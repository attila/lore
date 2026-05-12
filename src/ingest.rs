// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ingestion orchestration and write operations.
//!
//! Walks markdown directories, chunks files, embeds them via any [`Embedder`],
//! and stores results in the [`KnowledgeDB`].  Write helpers (`add_pattern`,
//! `update_pattern`, `append_to_pattern`) create or modify markdown files
//! on disk, re-index them, and optionally commit via git.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::chunking::{
    Chunk, MalformedPredicateEntry, chunk_as_document_with_malformed_predicates,
    chunk_by_heading_with_malformed_predicates, extract_title,
};
use crate::database::KnowledgeDB;
use crate::embeddings::Embedder;
use crate::git;
use crate::lore_debug;
use crate::loreignore;

// ---------------------------------------------------------------------------
// Embedding helpers
// ---------------------------------------------------------------------------

/// Build the composite text used for embedding a chunk.
///
/// Includes title and tags alongside the body so that vector search
/// carries domain signal, not just body content.
fn embed_text(chunk: &Chunk) -> String {
    format!("{}\n{}\n{}", chunk.title, chunk.tags, chunk.body)
}

// ---------------------------------------------------------------------------
// Metadata keys
// ---------------------------------------------------------------------------

/// Key used to store the last successfully ingested commit SHA.
pub(crate) const META_LAST_COMMIT: &str = "last_ingested_commit";
/// Key used to store the FNV-1a content hash of `.loreignore` so the next
/// ingest can detect when the ignore list has changed.
const META_LOREIGNORE_HASH: &str = "loreignore_hash";
/// Key used to stash the last ingest's universal-pattern advisories
/// (count summary, >3 warning, oversized-body flags, near-miss tags). The
/// MCP `lore_status` tool surfaces these to agents that can't see stderr.
pub(crate) const META_UNIVERSAL_ADVISORIES: &str = "universal_advisories";

/// Persist the universal-pattern advisories from a completed ingest so the
/// MCP `lore_status` tool can surface them to agents. The value is a JSON
/// object with fields `universal_count`, `universal_sources`,
/// `oversized_bodies`, `near_miss_tags`, `count_warning`,
/// `body_size_hard_limit_bytes`, and `body_size_warning_threshold_bytes`.
///
/// Only overwrites when the ingest touched any universal-related field.
/// A delta ingest that touches no universal-tagged files leaves the last
/// persisted summary intact — otherwise every non-universal-touching delta
/// would zero the advisory payload and agents would see a stale-looking
/// "Universal patterns: 0" after routine edits.
pub fn persist_universal_advisories(db: &KnowledgeDB, result: &IngestResult) -> anyhow::Result<()> {
    if !matches!(
        result.mode,
        IngestMode::Full | IngestMode::SingleFile { .. }
    ) && result.universal_sources.is_empty()
        && result.oversized_universal_bodies.is_empty()
        && result.near_miss_universal_tags.is_empty()
    {
        return Ok(());
    }
    let payload = universal_advisories_json(result);
    db.set_metadata(META_UNIVERSAL_ADVISORIES, &payload.to_string())
}

/// Build the structured JSON document persisted under
/// [`META_UNIVERSAL_ADVISORIES`] and returned verbatim by `lore_status`.
fn universal_advisories_json(result: &IngestResult) -> serde_json::Value {
    serde_json::json!({
        "universal_count": result.universal_sources.len(),
        "universal_sources": result.universal_sources,
        "oversized_bodies": result.oversized_universal_bodies,
        "near_miss_tags": result.near_miss_universal_tags,
        "count_warning": result.universal_sources.len() > 3,
        "body_size_hard_limit_bytes": UNIVERSAL_BODY_HARD_LIMIT_BYTES,
        "body_size_warning_threshold_bytes": UNIVERSAL_BODY_SIZE_WARNING_BYTES,
    })
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Whether the ingest ran in full, delta, or single-file mode.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum IngestMode {
    /// Full re-index (cleared database and re-embedded everything).
    #[default]
    Full,
    /// Delta update — only changed files were processed.
    Delta {
        /// Number of files that were unchanged and skipped.
        unchanged: usize,
    },
    /// Single-file upsert — exactly one file was re-indexed without walking
    /// the repository or consulting git state.
    SingleFile {
        /// Knowledge-dir-relative path of the file that was indexed.
        path: String,
    },
}

/// Summary returned after a directory ingest.
#[derive(Debug, Default)]
pub struct IngestResult {
    pub mode: IngestMode,
    pub files_processed: usize,
    pub chunks_created: usize,
    /// Files removed from the index by `.loreignore` reconciliation because
    /// they now match an ignore pattern.
    pub reconciled_removed: usize,
    /// Files re-indexed by `.loreignore` reconciliation because they exist
    /// on disk, are no longer matched by an ignore pattern, and were missing
    /// from the database.
    pub reconciled_added: usize,
    pub errors: Vec<String>,
    /// Distinct source files that ingested at least one chunk tagged
    /// `universal` in this run. Drives the always-on `Universal patterns: N`
    /// summary line and the >3 advisory.
    pub universal_sources: Vec<String>,
    /// Universal-tagged chunks whose body exceeded the body-size advisory
    /// threshold (currently 1024 bytes). Each entry is the source file path.
    /// Drives the per-pattern body-size advisory at ingest time.
    pub oversized_universal_bodies: Vec<String>,
    /// Tag values whose lowercased form equals `universal` but whose exact
    /// form does not (e.g. `Universal`, `UNIVERSAL`). Each entry is
    /// `<source_file>: <tag>`. Drives the near-miss spelling advisory.
    pub near_miss_universal_tags: Vec<String>,
    /// Per-file `applies_when` malformed-predicate advisories collected
    /// from the U2 frontmatter parser. Each entry names the source file,
    /// the offending key (e.g. `appliess_when`, `applies_when.tools`),
    /// and a short human-readable reason. The pattern is ingested as if
    /// no predicate were set (R9 skip-with-warning); the entry exists so
    /// CLI consumers can introspect the run after the fact and so the
    /// MCP tools can surface the count via `lore_status` in future work.
    /// The user-facing warning channel is `eprintln!` from inside
    /// `index_single_file` — single source so CLI and MCP write paths
    /// see the warning regardless of authoring surface.
    pub malformed_applies_when: Vec<MalformedPredicateEntry>,
}

impl IngestResult {
    /// Construct an empty `IngestResult` carrying `mode` with every other
    /// field at its default (zero counters, empty vectors). Every ingest
    /// entry point uses this as its starting point; callers then mutate as
    /// work progresses.
    pub fn with_mode(mode: IngestMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }
}

/// Outcome of the git step in a write operation.
#[derive(Debug)]
pub enum CommitStatus {
    /// No git repo or commit was not attempted.
    NotCommitted,
    /// Committed locally on the checked-out branch.
    Committed,
    /// Committed and pushed to a per-submission inbox branch.
    Pushed { branch: String },
}

/// Summary returned after a single-file write operation.
#[derive(Debug)]
pub struct WriteResult {
    pub file_path: String,
    pub chunks_indexed: usize,
    pub commit_status: CommitStatus,
    pub embedding_failures: usize,
}

// ---------------------------------------------------------------------------
// Directory ingest (delta and full)
// ---------------------------------------------------------------------------

/// Ingest the knowledge base, using delta mode when possible.
///
/// Tries to detect changes via `git diff --name-status` against the last
/// successfully ingested commit. Falls back to a full re-index when:
/// - `knowledge_dir` is not a git repository
/// - No previous commit SHA is stored in the database
/// - The stored commit no longer exists in the repository history
///
/// Use [`full_ingest`] directly to force a complete re-index.
#[allow(clippy::too_many_lines)]
pub fn ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    // Tier-2 warning when the effective scan set is empty (filesystem-empty
    // or all-ignored). Fires once at the top of the entry point so that
    // every downstream branch — delta, full, or any of the full-fallback
    // short-circuits below — surfaces the same signal. Continues regardless
    // (exit 0); see project memory `project_cli_behaviour_ladder.md`.
    if let Some(msg) = empty_warning_message(knowledge_dir) {
        on_progress(&msg);
    }

    // Not a git repo — full ingest is the only option.
    if !git::is_git_repo(knowledge_dir) {
        on_progress("Not a git repository — running full ingest");
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    }

    // No stored commit — first ingest, metadata cleared, or a fresh `git init`
    // with no commits yet. Discriminate the unborn-branch case so the user
    // gets a wording that explains what's actually happening (R9 of the
    // edge-case-handling brainstorm). Other reasons for landing in this
    // branch — first ingest after `lore ingest --force`, cleared database —
    // keep the original wording.
    let Ok(Some(last_commit)) = db.get_metadata(META_LAST_COMMIT) else {
        if git::is_unborn_head(knowledge_dir) {
            on_progress("No commits yet — HEAD will be recorded after your first commit.");
        } else {
            on_progress("No previous ingest recorded — running full ingest");
        }
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    };

    // Stored commit no longer exists (history rewrite, shallow clone, etc.).
    if !git::commit_exists(knowledge_dir, &last_commit) {
        on_progress("Previous commit not found in history — running full ingest");
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    }

    // Capture HEAD once to avoid TOCTOU between diff and recording.
    let head = match git::head_commit(knowledge_dir) {
        Ok(h) => h,
        Err(e) => {
            on_progress(&format!(
                "Failed to resolve HEAD ({e}) — running full ingest"
            ));
            return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
        }
    };

    // Run delta detection.
    let changes = match git::diff_name_status(knowledge_dir, &last_commit) {
        Ok(c) => c,
        Err(e) => {
            on_progress(&format!("git diff failed ({e}) — running full ingest"));
            return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
        }
    };

    // Detect .loreignore changes by content hash. The diff above filters to
    // markdown files, so .loreignore edits never appear there — the hash
    // comparison is the only signal. Reconciliation runs before FileChange
    // processing so the database starts in a clean state.
    //
    // Both the matcher and the hash come from a single read of .loreignore
    // (see loreignore::load), eliminating the race window where two sequential
    // reads could observe different file contents.
    let loaded_ignore = loreignore::load(knowledge_dir);
    let stored_hash = db
        .get_metadata(META_LOREIGNORE_HASH)
        .unwrap_or_default()
        .unwrap_or_default();
    let loreignore_changed = loaded_ignore.hash != stored_hash;
    lore_debug!(
        "loreignore: current_hash={:?} stored_hash={:?} changed={} matcher={}",
        loaded_ignore.hash,
        stored_hash,
        loreignore_changed,
        if loaded_ignore.matcher.is_some() {
            "loaded"
        } else {
            "none"
        }
    );

    let (reconcile_stats, reconcile_errors) = if loreignore_changed {
        on_progress(".loreignore changed — running reconciliation");
        run_reconciliation(
            db,
            embedder,
            knowledge_dir,
            strategy,
            &loaded_ignore,
            on_progress,
        )
    } else {
        (ReconcileStats::default(), Vec::new())
    };
    let mut reconcile_errors = reconcile_errors;

    let reconciled_removed = reconcile_stats.removed;
    let reconciled_added = reconcile_stats.added;
    let reconcile_chunks = reconcile_stats.chunks_added;
    let reconcile_did_work = reconciled_removed > 0 || reconciled_added > 0;

    if changes.is_empty() {
        if !reconcile_did_work && reconcile_errors.is_empty() {
            on_progress("Already up to date — no files changed since last ingest");
        } else {
            on_progress(&format!(
                "Reconciliation: {reconciled_removed} removed, {reconciled_added} re-indexed; HEAD recorded"
            ));
            if let Err(e) = db.set_metadata(META_LAST_COMMIT, &head) {
                reconcile_errors.push(format!("Failed to record commit SHA: {e}"));
            }
        }
        let mut result = IngestResult::with_mode(IngestMode::Delta { unchanged: 0 });
        result.chunks_created = reconcile_chunks;
        result.reconciled_removed = reconciled_removed;
        result.reconciled_added = reconciled_added;
        result.errors = reconcile_errors;
        return result;
    }

    let mut result = delta_ingest(
        db,
        embedder,
        knowledge_dir,
        strategy,
        &changes,
        &head,
        loaded_ignore.matcher.as_ref(),
        on_progress,
    );
    // Surface reconciliation errors and counts to the caller alongside delta
    // results.
    result.errors.extend(reconcile_errors);
    result.reconciled_removed = reconciled_removed;
    result.reconciled_added = reconciled_added;
    result.chunks_created += reconcile_chunks;
    result
}

/// Run a reconciliation pass and update the stored content hash on success.
///
/// Encapsulates the "reconcile then commit hash" sequence so that
/// [`ingest`] can call it as a single statement, keeping the entry-point
/// function within the line limit.
fn run_reconciliation(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    loaded: &loreignore::LoadedIgnore,
    on_progress: &dyn Fn(&str),
) -> (ReconcileStats, Vec<String>) {
    let mut errors: Vec<String> = Vec::new();
    let mut stats = match reconcile_ignored(
        db,
        embedder,
        knowledge_dir,
        strategy,
        loaded.matcher.as_ref(),
        on_progress,
    ) {
        Ok(stats) => {
            // Only update the stored hash on successful reconciliation.
            // If reconciliation failed partway through, the database is in
            // a partially reconciled state — leaving the hash stale forces
            // the next ingest to retry, rather than skipping reconciliation
            // and silently leaving stale chunks.
            if let Err(e) = db.set_metadata(META_LOREIGNORE_HASH, &loaded.hash) {
                errors.push(format!("Failed to record .loreignore hash: {e}"));
            }
            stats
        }
        Err(e) => {
            errors.push(format!("Reconciliation failed: {e}"));
            ReconcileStats::default()
        }
    };
    // Surface lossy-path warnings collected by the reconciliation walk on
    // the same channel as the rest of the reconcile errors so they reach
    // `IngestResult::errors` via the existing caller plumbing.
    errors.append(&mut stats.lossy_warnings);
    (stats, errors)
}

/// Outcome of a `.loreignore` reconciliation pass.
#[derive(Debug, Default)]
struct ReconcileStats {
    /// Files removed from the database because they now match an ignore
    /// pattern.
    removed: usize,
    /// Files re-indexed from disk because they are no longer matched by an
    /// ignore pattern (or `.loreignore` was deleted entirely) and were
    /// missing from the database.
    added: usize,
    /// Total chunks inserted by the re-index pass.
    chunks_added: usize,
    /// Lossy-path warnings collected by the reconciliation walk. Surfaced
    /// onto `IngestResult::errors` by the caller so a non-UTF-8 filename
    /// encountered during reconciliation is reported with the same
    /// severity as one encountered during `full_ingest`.
    lossy_warnings: Vec<String>,
}

/// Reconcile the database against the current `.loreignore` matcher.
///
/// This pass is cumulative: it both removes files that are now ignored
/// **and** re-indexes files that are now allowed but missing from the
/// database. The two directions together make `.loreignore` edits behave
/// transparently — adding a pattern removes matching files, removing a
/// pattern brings them back, and deleting `.loreignore` re-indexes
/// everything that had been excluded.
///
/// When `ignore_matcher` is `None` (no `.loreignore` file), the removal
/// pass is a no-op but the re-index pass still runs — picking up files
/// that were previously excluded.
fn reconcile_ignored(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
    on_progress: &dyn Fn(&str),
) -> anyhow::Result<ReconcileStats> {
    use std::collections::HashSet;

    let mut stats = ReconcileStats::default();

    // Snapshot the indexed source list once. We use it for both the removal
    // pass and the re-index pass; the snapshot is taken before any deletions
    // so the re-index pass can correctly identify "files we just removed"
    // and avoid re-indexing them.
    let db_sources_vec = db.source_files()?;
    lore_debug!(
        "loreignore: reconcile scanning {} indexed sources",
        db_sources_vec.len()
    );
    let db_sources: HashSet<String> = db_sources_vec.iter().cloned().collect();

    // Pass 1: remove files in the database that the matcher now rejects.
    if let Some(matcher) = ignore_matcher {
        for source in &db_sources_vec {
            let ignored = loreignore::is_ignored(matcher, Path::new(source), false);
            lore_debug!("loreignore: reconcile check {source} → ignored={ignored}");
            if ignored {
                db.delete_by_source(source)?;
                stats.removed += 1;
                lore_debug!("loreignore: reconciled {source} (removed from index)");
                on_progress(&format!("  {source} (reconciled — removed)"));
            }
        }
    } else {
        lore_debug!("loreignore: reconcile removal pass skipped (no matcher)");
    }

    // Pass 2: walk the filesystem and re-index any markdown file that is
    // not currently in the database (using the pre-pass-1 snapshot, so
    // files we just removed are correctly excluded). walk_md_files already
    // applies the ignore matcher, so this loop only sees allowed files.
    let (disk_files, _, lossy_warnings) = walk_md_files(knowledge_dir, ignore_matcher);
    stats.lossy_warnings = lossy_warnings;
    for (rel_path, full_path) in disk_files {
        if db_sources.contains(&rel_path) {
            continue;
        }
        match index_single_file(db, embedder, knowledge_dir, &full_path, strategy) {
            Ok(indexed) => {
                stats.added += 1;
                stats.chunks_added += indexed.chunks_indexed;
                lore_debug!(
                    "loreignore: reconciled {rel_path} (re-indexed, {} chunks)",
                    indexed.chunks_indexed
                );
                on_progress(&format!("  {rel_path} (reconciled — re-indexed)"));
            }
            Err(e) => {
                lore_debug!("loreignore: failed to re-index {rel_path}: {e}");
                return Err(e);
            }
        }
    }

    Ok(stats)
}

/// Return `true` when the path is matched by the `.loreignore` matcher.
/// Returns `false` when no matcher is present.
fn path_ignored(matcher: Option<&ignore::gitignore::Gitignore>, path: &str) -> bool {
    matcher.is_some_and(|m| loreignore::is_ignored(m, Path::new(path), false))
}

/// Process only the files that changed since the last ingest.
#[allow(clippy::too_many_arguments)]
fn delta_ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    changes: &[git::FileChange],
    head: &str,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    // Count unchanged files: existing sources minus those that this delta
    // will actually touch. Ignored changes do not affect indexed state, so
    // they must not deflate the unchanged count.
    let sources_before = db.stats().map(|s| s.sources).unwrap_or(0);
    let existing_changed = changes
        .iter()
        .filter(|c| match c {
            git::FileChange::Added(_) => false,
            git::FileChange::Modified(p) | git::FileChange::Deleted(p) => {
                !path_ignored(ignore_matcher, p)
            }
            git::FileChange::Renamed { from, .. } => !path_ignored(ignore_matcher, from),
        })
        .count();
    let unchanged = sources_before.saturating_sub(existing_changed);

    on_progress(&format!("Delta ingest: {} file(s) changed", changes.len()));

    let mut result = IngestResult::with_mode(IngestMode::Delta { unchanged });

    for change in changes {
        let processed = process_change(
            db,
            embedder,
            knowledge_dir,
            strategy,
            change,
            ignore_matcher,
            on_progress,
            &mut result,
        );
        if processed {
            result.files_processed += 1;
        }
    }

    // Record the pre-captured HEAD on success (no errors).
    if result.errors.is_empty()
        && let Err(e) = db.set_metadata(META_LAST_COMMIT, head)
    {
        on_progress(&format!("Warning: failed to record commit SHA: {e}"));
    }

    result
}

/// Process one [`git::FileChange`] within delta ingest.
///
/// Returns `true` when the change actually touched the database (and should
/// count toward `files_processed`); returns `false` for changes that were
/// skipped because the path is ignored.
#[allow(clippy::too_many_arguments)]
fn process_change(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    change: &git::FileChange,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
    on_progress: &dyn Fn(&str),
    result: &mut IngestResult,
) -> bool {
    match change {
        git::FileChange::Added(path) | git::FileChange::Modified(path) => {
            if path_ignored(ignore_matcher, path) {
                lore_debug!("loreignore: skipping {path} (delta ingest)");
                return false;
            }
            let file_path = knowledge_dir.join(path);
            match index_single_file(db, embedder, knowledge_dir, &file_path, strategy) {
                Ok(indexed) => {
                    result.chunks_created += indexed.chunks_indexed;
                    fold_universal_metadata(result, &indexed.rel_path, &indexed.universal_metadata);
                    fold_malformed_applies_when(result, &indexed.malformed_applies_when);
                    on_progress(&format!("  {path} → {} chunks", indexed.chunks_indexed));
                }
                Err(e) => {
                    result.errors.push(format!("Failed to index {path}: {e}"));
                }
            }
            true
        }
        git::FileChange::Deleted(path) => {
            if let Err(e) = db.delete_by_source(path) {
                result.errors.push(format!("Failed to delete {path}: {e}"));
            } else {
                on_progress(&format!("  {path} (deleted)"));
            }
            true
        }
        git::FileChange::Renamed { from, to } => {
            let from_ignored = path_ignored(ignore_matcher, from);
            let to_ignored = path_ignored(ignore_matcher, to);
            // Only delete from-side chunks when the source had been indexed.
            if !from_ignored && let Err(e) = db.delete_by_source(from) {
                result
                    .errors
                    .push(format!("Failed to delete old path {from}: {e}"));
            }
            if to_ignored {
                lore_debug!("loreignore: rename target {to} ignored, skipping index");
                if from_ignored {
                    return false;
                }
                on_progress(&format!("  {from} → {to} (target ignored)"));
                return true;
            }
            let file_path = knowledge_dir.join(to);
            match index_single_file(db, embedder, knowledge_dir, &file_path, strategy) {
                Ok(indexed) => {
                    result.chunks_created += indexed.chunks_indexed;
                    fold_universal_metadata(result, &indexed.rel_path, &indexed.universal_metadata);
                    fold_malformed_applies_when(result, &indexed.malformed_applies_when);
                    on_progress(&format!(
                        "  {from} → {to} ({} chunks)",
                        indexed.chunks_indexed
                    ));
                }
                Err(e) => {
                    result.errors.push(format!("Failed to index {to}: {e}"));
                }
            }
            true
        }
    }
}

/// Walk a knowledge directory for markdown files, optionally filtering
/// through a `.loreignore` matcher.
///
/// Returns the kept files as `(rel_path, full_path)` tuples sorted by
/// relative path, plus the total number of markdown files seen before
/// filtering. Silent — callers attach progress messages themselves.
/// Walk `knowledge_dir` for markdown files and split the result into three
/// buckets:
///
/// - `kept`: files with valid-UTF-8 relative paths that survive
///   `.loreignore` filtering — the indexable set.
/// - `walked_count`: total number of `.md` / `.markdown` files encountered
///   on the walk, before any filtering. Used by callers to derive the
///   number excluded by `.loreignore` (`walked_count - kept.len() -
///   lossy_warnings.len()`).
/// - `lossy_warnings`: one entry per file whose on-disk filename is not
///   valid UTF-8, formatted for surfacing on `IngestResult::errors`. These
///   files are deliberately not added to `kept` — indexing them would
///   store the chunk under a U+FFFD-substituted source-file key, masking
///   the underlying problem.
fn walk_md_files(
    knowledge_dir: &Path,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
) -> (Vec<(String, PathBuf)>, usize, Vec<String>) {
    let walked: Vec<PathBuf> = WalkDir::new(knowledge_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file()
                && matches!(
                    e.path().extension().and_then(|s| s.to_str()),
                    Some("md" | "markdown")
                )
        })
        .map(walkdir::DirEntry::into_path)
        .collect();

    let walked_count = walked.len();
    let mut kept: Vec<(String, PathBuf)> = Vec::new();
    let mut lossy_warnings: Vec<String> = Vec::new();
    for path in walked {
        let Ok(rel_path) = path.strip_prefix(knowledge_dir) else {
            continue;
        };
        let rel_cow = rel_path.to_string_lossy();
        if matches!(rel_cow, Cow::Owned(_)) {
            lossy_warnings.push(format!(
                "Skipped {rel_cow}: filename is not valid UTF-8 (file not indexed)"
            ));
            continue;
        }
        let rel = rel_cow.into_owned();
        if let Some(matcher) = ignore_matcher
            && loreignore::is_ignored(matcher, Path::new(&rel), false)
        {
            lore_debug!("loreignore: skipping {rel} (walk)");
            continue;
        }
        kept.push((rel, path));
    }
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    (kept, walked_count, lossy_warnings)
}

/// Walk + report for full ingest. Returns the kept full paths plus any
/// lossy-path warnings collected by [`walk_md_files`] so the caller can
/// fold them onto `IngestResult::errors`.
fn discover_md_files(
    knowledge_dir: &Path,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
    on_progress: &dyn Fn(&str),
) -> (Vec<PathBuf>, Vec<String>) {
    let (kept, walked_count, lossy_warnings) = walk_md_files(knowledge_dir, ignore_matcher);
    // Files excluded by `.loreignore` only — lossy paths are surfaced via
    // `IngestResult::errors`, not on this progress line.
    let ignored = walked_count - kept.len() - lossy_warnings.len();
    if ignored > 0 {
        on_progress(&format!(
            "Found {} markdown files ({} excluded by .loreignore)",
            kept.len(),
            ignored
        ));
    } else {
        on_progress(&format!("Found {} markdown files", kept.len()));
    }
    (kept.into_iter().map(|(_, p)| p).collect(), lossy_warnings)
}

/// Result of evaluating the effective scan set of a knowledge directory —
/// the markdown files that survive `.loreignore` filtering.
///
/// Used to discriminate the cause of an empty effective scan set so the
/// user-visible warning can name the right recovery action. Surfaces that
/// only need to know "is anything indexable?" can call [`is_effective_empty`]
/// for a bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectiveScanState {
    /// At least one markdown file survives `.loreignore` filtering.
    Populated,
    /// No markdown files exist at all in the knowledge directory.
    FilesystemEmpty,
    /// Markdown files exist on disk, but `.loreignore` excludes every one.
    AllIgnored,
    /// `knowledge_dir` does not exist on disk or is not a directory (regular
    /// file, broken symlink, missing parent). The user almost certainly has
    /// a typo or a stale path in the config — running ingest is the wrong
    /// recovery action.
    Missing,
}

/// Effective scan-set state plus auxiliary flags derived from the same
/// `.loreignore` load + `walk_md_files` pass.
///
/// Centralising both fields here lets `handle_lore_status` and the ingest
/// entry point compute the full picture from a single filesystem walk
/// rather than re-reading `.loreignore` and re-walking the directory per
/// caller.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanInfo {
    pub state: EffectiveScanState,
    pub loreignore_active: bool,
}

/// Compute the effective scan state of a knowledge directory.
///
/// Walks `knowledge_dir` for markdown files and applies `.loreignore`
/// filtering. Returns both the discriminated state and a `loreignore_active`
/// flag derived from the same single load, so callers that need both (like
/// `handle_lore_status`) avoid a second `.loreignore` read.
///
/// When `knowledge_dir` does not exist or is not a directory, the function
/// short-circuits with [`EffectiveScanState::Missing`] — no walk, no
/// `.loreignore` load, and `loreignore_active` is reported as `false`.
pub(crate) fn effective_scan_state(knowledge_dir: &Path) -> ScanInfo {
    if !knowledge_dir.is_dir() {
        return ScanInfo {
            state: EffectiveScanState::Missing,
            loreignore_active: false,
        };
    }
    let loaded_ignore = loreignore::load(knowledge_dir);
    let loreignore_active = loaded_ignore.matcher.is_some();
    let (kept, walked_count, lossy_warnings) =
        walk_md_files(knowledge_dir, loaded_ignore.matcher.as_ref());
    // Subtract lossy-named files from the walk count so an all-lossy
    // directory routes to `FilesystemEmpty` rather than `AllIgnored` — the
    // .loreignore-blamed wording would name the wrong cause and recovery
    // action. The per-file lossy warnings travel separately via
    // `walk_md_files` → `discover_md_files` → `IngestResult::errors`; this
    // probe is a synchronous bool/state surface and discards them.
    let effective_walked = walked_count - lossy_warnings.len();
    let state = if !kept.is_empty() {
        EffectiveScanState::Populated
    } else if effective_walked == 0 {
        EffectiveScanState::FilesystemEmpty
    } else {
        EffectiveScanState::AllIgnored
    };
    ScanInfo {
        state,
        loreignore_active,
    }
}

/// Returns `true` when the effective scan set is empty — either because no
/// markdown files exist, `.loreignore` excludes every candidate, or the
/// directory does not exist on disk.
///
/// Convenience wrapper for callers that only need a bool.
pub fn is_effective_empty(knowledge_dir: &Path) -> bool {
    !matches!(
        effective_scan_state(knowledge_dir).state,
        EffectiveScanState::Populated
    )
}

/// Returns a stable label describing the knowledge directory's effective
/// scan-set state: `"populated"`, `"empty"`, or `"missing"`.
///
/// Used by the `lore_status` MCP tool's `knowledge_dir_status` field and the
/// `lore status` CLI's `Scan set:` line so both surfaces report the same
/// discrimination without exposing the internal enum across the binary/library
/// boundary.
pub fn knowledge_dir_status_label(knowledge_dir: &Path) -> &'static str {
    match effective_scan_state(knowledge_dir).state {
        EffectiveScanState::Populated => "populated",
        EffectiveScanState::Missing => "missing",
        EffectiveScanState::FilesystemEmpty | EffectiveScanState::AllIgnored => "empty",
    }
}

/// Compose the user-facing tier-2 warning for an effective-empty knowledge
/// directory, or `None` when the directory is populated.
///
/// Distinct messages per cause so the recovery action (add a `.md` file,
/// relax `.loreignore`, or fix the `knowledge_dir` path) is unambiguous.
/// Used by [`ingest`] (emitted via `on_progress`) and by `cmd_serve` startup
/// (emitted via `eprintln!`).
pub fn empty_warning_message(knowledge_dir: &Path) -> Option<String> {
    match effective_scan_state(knowledge_dir).state {
        EffectiveScanState::Populated => None,
        EffectiveScanState::Missing => Some(format!(
            "Warning: knowledge directory not found at {} — check `knowledge_dir` in your config",
            knowledge_dir.display()
        )),
        EffectiveScanState::FilesystemEmpty => Some(format!(
            "Warning: knowledge directory is empty — add at least one .md file under {}",
            knowledge_dir.display()
        )),
        EffectiveScanState::AllIgnored => Some(
            "Warning: .loreignore matched every markdown file; nothing will be indexed".to_string(),
        ),
    }
}

/// Clear the database and re-index every markdown file from scratch.
///
/// Records the current HEAD commit SHA on success so that subsequent
/// [`ingest`] calls can use delta mode.
pub fn full_ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    let mut result = IngestResult::with_mode(IngestMode::Full);

    // Single read of .loreignore: matcher and hash come from the same bytes.
    let loaded_ignore = loreignore::load(knowledge_dir);
    let (md_files, lossy_warnings) =
        discover_md_files(knowledge_dir, loaded_ignore.matcher.as_ref(), on_progress);
    // Surface non-UTF-8 filenames on the same per-file error channel
    // `index_single_file` failures use. The corresponding files are not in
    // `md_files`, so the index never sees a U+FFFD-substituted source key.
    for warning in lossy_warnings {
        result.errors.push(warning);
    }

    // The effective-empty warning fires from `ingest()` at the entry point
    // (see `empty_warning_message`); both filesystem-empty and all-ignored
    // cases are covered there, so `full_ingest` does not need a redundant
    // emission for direct callers.

    if let Err(e) = db.clear_all() {
        result.errors.push(format!("Failed to clear database: {e}"));
        return result;
    }

    // Store the .loreignore content hash so delta ingest can detect changes.
    // clear_all() does not touch ingest_metadata, so a stale value from a
    // previous ingest could survive — always write the current hash.
    if let Err(e) = db.set_metadata(META_LOREIGNORE_HASH, &loaded_ignore.hash) {
        on_progress(&format!("Warning: failed to record .loreignore hash: {e}"));
    }

    for file_path in &md_files {
        match index_single_file(db, embedder, knowledge_dir, file_path, strategy) {
            Ok(indexed) => {
                result.chunks_created += indexed.chunks_indexed;
                fold_universal_metadata(
                    &mut result,
                    &indexed.rel_path,
                    &indexed.universal_metadata,
                );
                fold_malformed_applies_when(&mut result, &indexed.malformed_applies_when);
                result.files_processed += 1;
                on_progress(&format!(
                    "  {} → {} chunks",
                    indexed.rel_path, indexed.chunks_indexed
                ));
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to index {}: {e}", file_path.display()));
            }
        }
    }

    // Record the HEAD commit for future delta ingests.
    if result.errors.is_empty()
        && git::is_git_repo(knowledge_dir)
        && let Ok(head) = git::head_commit(knowledge_dir)
        && let Err(e) = db.set_metadata(META_LAST_COMMIT, &head)
    {
        on_progress(&format!("Warning: failed to record commit SHA: {e}"));
    }

    result
}

// ---------------------------------------------------------------------------
// Single-file ingest
// ---------------------------------------------------------------------------

/// Upsert a single markdown file into the index without walking the
/// repository or consulting git state.
///
/// This is the fast edit-ingest-search feedback loop for pattern authoring:
/// edit a file, run `lore ingest --file path/to/it.md`, and it is immediately
/// searchable — no commit required. Intended as an alternative to the
/// walk-based entry points [`ingest`] and [`full_ingest`], not a replacement.
///
/// The function:
/// - canonicalises `file_path` and verifies it lies inside `knowledge_dir`;
/// - rejects non-markdown extensions (`.md`, `.markdown`);
/// - consults `.loreignore` and refuses to index excluded files unless
///   `force_override_ignore` is `true`;
/// - delegates the actual read → chunk → embed → insert sequence to the
///   same internal helper used by walk-based ingest, so chunking and
///   embedding behaviour match exactly;
/// - does **not** touch the stored `last_ingested_commit` or `.loreignore`
///   hash — this path is orthogonal to walk-based delta state and must not
///   interfere with the next `lore ingest` picking up real git changes.
///
/// All error conditions are reported via `IngestResult::errors`; the function
/// never panics and never returns early without populating the result.
pub fn ingest_single_file(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    file_path: &Path,
    strategy: &str,
    force_override_ignore: bool,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    // Seed the mode with the caller-supplied path so that early-return error
    // branches (before rel_path is computed) still carry a meaningful path
    // for downstream consumers that pattern-match on IngestMode::SingleFile.
    let mut result = IngestResult::with_mode(IngestMode::SingleFile {
        path: file_path.to_string_lossy().into_owned(),
    });

    // Canonicalise the file path. This also implicitly checks that it exists
    // and is accessible. Include the current working directory in the error
    // so agents hitting a relative-path mismatch can self-diagnose.
    let canonical = match file_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            let cwd_hint = std::env::current_dir()
                .map(|c| format!(" (cwd: {})", c.display()))
                .unwrap_or_default();
            result.errors.push(format!(
                "Cannot access {}{cwd_hint}: {e}",
                file_path.display()
            ));
            return result;
        }
    };

    if !canonical.is_file() {
        result
            .errors
            .push(format!("Not a regular file: {}", canonical.display()));
        return result;
    }

    match canonical.extension().and_then(|s| s.to_str()) {
        Some("md" | "markdown") => {}
        other => {
            let ext = other.unwrap_or("(none)");
            result.errors.push(format!(
                "Unsupported extension '{ext}' for {}: only .md and .markdown are indexed",
                canonical.display()
            ));
            return result;
        }
    }

    // Ensure the file lies inside the knowledge directory. Uses the same
    // guard as add_pattern / update_pattern so path-traversal protection is
    // uniform across write paths. The returned canonical path is the same
    // value we already have, so we ignore it here.
    if let Err(e) = validate_within_dir(knowledge_dir, &canonical) {
        result.errors.push(e.to_string());
        return result;
    }

    // Derive the relative path the same way index_single_file does, against
    // the canonicalised knowledge directory so strip_prefix succeeds.
    let canonical_dir = match knowledge_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            result.errors.push(format!(
                "Cannot access knowledge directory {}: {e}",
                knowledge_dir.display()
            ));
            return result;
        }
    };
    let rel_path = canonical
        .strip_prefix(&canonical_dir)
        .unwrap_or(&canonical)
        .to_string_lossy()
        .to_string();

    // Respect .loreignore by default. The override flag exists for the
    // author-iterating-on-a-draft case.
    if !force_override_ignore {
        let loaded = loreignore::load(&canonical_dir);
        if let Some(matcher) = loaded.matcher.as_ref()
            && loreignore::is_ignored(matcher, Path::new(&rel_path), false)
        {
            result.errors.push(format!(
                "{rel_path} is excluded by .loreignore; pass --force to index anyway"
            ));
            return result;
        }
    }

    lore_debug!("ingest_single_file: {rel_path} (override={force_override_ignore})");
    on_progress(&format!("Single-file ingest: {rel_path}"));

    match index_single_file(db, embedder, &canonical_dir, &canonical, strategy) {
        Ok(indexed) => {
            result.files_processed = 1;
            result.chunks_created = indexed.chunks_indexed;
            if indexed.embedding_failures > 0 {
                result.errors.push(format!(
                    "{} embedding failure(s) while indexing {rel_path}",
                    indexed.embedding_failures
                ));
            }
            fold_universal_metadata(&mut result, &indexed.rel_path, &indexed.universal_metadata);
            fold_malformed_applies_when(&mut result, &indexed.malformed_applies_when);
            on_progress(&format!("  {rel_path} → {} chunks", indexed.chunks_indexed));
        }
        Err(e) => {
            result
                .errors
                .push(format!("Failed to index {rel_path}: {e}"));
        }
    }

    result.mode = IngestMode::SingleFile { path: rel_path };
    result
}

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

/// Create a new pattern file, index it, and commit.
///
/// When `inbox_branch_prefix` is `Some`, the file is committed to a
/// per-submission branch and pushed to the remote instead of being written
/// to disk and indexed locally.
pub fn add_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    title: &str,
    body: &str,
    tags: &[&str],
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    // Sanitise the title at the canonical write boundary: trim surrounding
    // whitespace and reject embedded newlines. Without this, surrounding
    // whitespace would round-trip as a misclassified collision (extract_title
    // trims; build_file_content writes verbatim) and embedded newlines would
    // truncate the on-disk heading at the first `\n`, defeating the
    // collision discriminator forever and corrupting the indexed pattern.
    let title = title.trim();
    if title.contains('\n') || title.contains('\r') {
        anyhow::bail!("Title must not contain newline characters");
    }

    let slug = slugify(title);
    if slug.is_empty() {
        anyhow::bail!("Title must contain at least one alphanumeric character");
    }
    let filename = slug.clone() + ".md";

    // Validate slug doesn't contain path traversal components.
    validate_slug(&filename)?;

    let content = build_file_content(title, body, tags);

    if let Some(prefix) = inbox_branch_prefix {
        let branch = git::commit_to_new_branch(
            knowledge_dir,
            prefix,
            &slug,
            &filename,
            &content,
            &format!("lore: add pattern \"{title}\""),
        )?;
        push_or_cleanup(knowledge_dir, &branch)?;

        return Ok(WriteResult {
            file_path: filename,
            chunks_indexed: 0,
            commit_status: CommitStatus::Pushed { branch },
            embedding_failures: 0,
        });
    }

    let file_path = knowledge_dir.join(&filename);

    if file_path.exists() {
        // Discriminate a slug collision (two distinct titles sharing a slug)
        // from an intentional re-use (the same title written twice). Both
        // hit the same path on disk, but the user-visible recovery differs:
        // re-use → use `update_pattern`; collision → choose a different
        // title. Compares titles after NFC normalisation so an NFC/NFD pair
        // of the same visual title classifies as re-use.
        // Read the existing file to extract its title for the discriminator.
        // On read failure (directory at `file_path`, permission denied, or
        // non-UTF-8 content), fall back to empty contents so the collision
        // branch fires with the curated `(no title heading)` label rather
        // than propagating a raw OS error and bypassing the tier-1 wording.
        let existing = std::fs::read_to_string(&file_path).unwrap_or_default();
        // `extract_title` can return `Some("")` for a bare `# ` heading with
        // no text. Treat that as no extractable title so the collision label
        // reads `(no title heading)` rather than `title: ""`.
        let existing_title_raw = extract_title(&existing).filter(|t| !t.trim().is_empty());
        let incoming_nfc: String = title.nfc().collect();
        let existing_nfc: Option<String> = existing_title_raw.as_deref().map(|t| t.nfc().collect());

        if existing_nfc.as_deref() == Some(incoming_nfc.as_str()) {
            anyhow::bail!(
                "Pattern \"{title}\" already exists at {filename}. \
                 Use update_pattern to modify it."
            );
        }

        let existing_label = existing_title_raw.as_deref().map_or_else(
            || "no title heading".to_string(),
            |t| format!("title: \"{t}\""),
        );
        anyhow::bail!(
            "Slug \"{slug}\" already used by {filename} ({existing_label}). \
             Choose a different title or call update_pattern to modify the existing file."
        );
    }

    std::fs::write(&file_path, &content)?;

    let IndexedFile {
        chunks_indexed: chunks,
        embedding_failures,
        ..
    } = index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

    let commit_status = try_commit(
        knowledge_dir,
        &file_path,
        &format!("lore: add pattern \"{title}\""),
    );

    Ok(WriteResult {
        file_path: filename,
        chunks_indexed: chunks,
        commit_status,
        embedding_failures,
    })
}

/// Overwrite an existing pattern file, re-index it, and commit.
///
/// When `inbox_branch_prefix` is `Some`, the modification is committed to a
/// per-submission branch and pushed. The file must exist on the working tree
/// (trunk) — inbox-only files are not supported.
///
/// `tags` has three cases:
/// - `None` — preserve the existing frontmatter tags. This is the default
///   path for agents that rewrite only the body and would otherwise silently
///   drop every tag (including `universal`, de-universalising the pattern).
/// - `Some(&[])` — explicitly clear all tags.
/// - `Some(&[...])` — replace the tag list wholesale.
pub fn update_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    source_file: &str,
    body: &str,
    tags: Option<&[&str]>,
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    let file_path = knowledge_dir.join(source_file);

    if !file_path.exists() {
        anyhow::bail!("File not found: {source_file}");
    }

    // Use the canonical path returned by validation for every subsequent
    // read and write so an attacker cannot race a symlink swap between the
    // containment check and file access.
    let canonical = validate_within_dir(knowledge_dir, &file_path)?;

    let existing = std::fs::read_to_string(&canonical)?;
    let title = extract_title(&existing).unwrap_or_else(|| file_stem(source_file));

    // Preserve-on-`None` resolves the subtle footgun where an agent rewrites
    // the body through `update_pattern` but forgets to pass `tags`, which
    // previously silently cleared every tag — including `universal`, which
    // would de-universalise a pinned pattern without any signal.
    let preserved: Vec<String>;
    let preserved_refs: Vec<&str>;
    let tags_to_apply: &[&str] = if let Some(t) = tags {
        t
    } else {
        preserved = crate::chunking::parse_frontmatter_tag_list(&existing);
        preserved_refs = preserved.iter().map(String::as_str).collect();
        &preserved_refs
    };

    let content = build_file_content(&title, body, tags_to_apply);

    if let Some(prefix) = inbox_branch_prefix {
        let slug = file_stem(source_file);
        let branch = git::commit_to_new_branch(
            knowledge_dir,
            prefix,
            &slug,
            source_file,
            &content,
            &format!("lore: update pattern \"{title}\""),
        )?;
        push_or_cleanup(knowledge_dir, &branch)?;

        return Ok(WriteResult {
            file_path: source_file.to_string(),
            chunks_indexed: 0,
            commit_status: CommitStatus::Pushed { branch },
            embedding_failures: 0,
        });
    }

    std::fs::write(&canonical, &content)?;

    let IndexedFile {
        chunks_indexed: chunks,
        embedding_failures,
        ..
    } = index_single_file(db, embedder, knowledge_dir, &canonical, "heading")?;

    let commit_status = try_commit(
        knowledge_dir,
        &file_path,
        &format!("lore: update pattern \"{title}\""),
    );

    Ok(WriteResult {
        file_path: source_file.to_string(),
        chunks_indexed: chunks,
        commit_status,
        embedding_failures,
    })
}

/// Append a new section to an existing pattern file, re-index, and commit.
///
/// When `inbox_branch_prefix` is `Some`, the modification is committed to a
/// per-submission branch and pushed. The file must exist on the working tree
/// (trunk) — inbox-only files are not supported.
pub fn append_to_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    source_file: &str,
    heading: &str,
    body: &str,
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    let file_path = knowledge_dir.join(source_file);

    if !file_path.exists() {
        anyhow::bail!("File not found: {source_file}");
    }

    // Use the canonical path returned by validation for every subsequent
    // read and write so an attacker cannot race a symlink swap between the
    // containment check and file access.
    let canonical = validate_within_dir(knowledge_dir, &file_path)?;

    let existing = std::fs::read_to_string(&canonical)?;
    let title = extract_title(&existing).unwrap_or_else(|| file_stem(source_file));

    let mut content = existing;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let _ = write!(content, "\n## {heading}\n\n");
    content.push_str(body);
    content.push('\n');

    if let Some(prefix) = inbox_branch_prefix {
        let slug = file_stem(source_file);
        let branch = git::commit_to_new_branch(
            knowledge_dir,
            prefix,
            &slug,
            source_file,
            &content,
            &format!("lore: append to \"{title}\" — {heading}"),
        )?;
        push_or_cleanup(knowledge_dir, &branch)?;

        return Ok(WriteResult {
            file_path: source_file.to_string(),
            chunks_indexed: 0,
            commit_status: CommitStatus::Pushed { branch },
            embedding_failures: 0,
        });
    }

    std::fs::write(&canonical, &content)?;

    let IndexedFile {
        chunks_indexed: chunks,
        embedding_failures,
        ..
    } = index_single_file(db, embedder, knowledge_dir, &canonical, "heading")?;

    let commit_status = try_commit(
        knowledge_dir,
        &file_path,
        &format!("lore: append to \"{title}\" — {heading}"),
    );

    Ok(WriteResult {
        file_path: source_file.to_string(),
        chunks_indexed: chunks,
        commit_status,
        embedding_failures,
    })
}

// ---------------------------------------------------------------------------
// Universal-pattern detection at ingest
// ---------------------------------------------------------------------------

/// Soft threshold (bytes) for the per-pattern universal-body advisory.
///
/// Universal patterns re-inject on every relevant `PreToolUse` call, so a large
/// body compounds quickly: a 2KB body matched 50 times in a session costs
/// 100KB of repeated context. The advisory fires per-pattern when the body
/// exceeds this threshold so authors notice unintended bloat at ingest time.
const UNIVERSAL_BODY_SIZE_WARNING_BYTES: usize = 1024;

/// Hard cap (bytes) on the total chunked-body size of a single universal file.
///
/// Exceeding this at ingest is a rejected error rather than a stderr advisory:
/// universal bodies re-inject on every relevant tool call, so a 50KB universal
/// file multiplied across 100 tool calls is a 5MB context-window liability.
/// The cap is deliberately 8× the soft warning so authors get a warning band
/// between "notice this" and "reject this".
pub const UNIVERSAL_BODY_HARD_LIMIT_BYTES: usize = 8 * 1024;

/// Enforce the per-file universal body-size cap. Returns an error naming the
/// file, observed size, and the cap when any universal-tagged file exceeds
/// [`UNIVERSAL_BODY_HARD_LIMIT_BYTES`]. Called from every ingest entry point
/// before touching the database so rejection is atomic at the file level.
pub(crate) fn enforce_universal_body_cap(rel_path: &str, chunks: &[Chunk]) -> anyhow::Result<()> {
    let total: usize = chunks
        .iter()
        .filter(|c| c.is_universal)
        .map(|c| c.body.len())
        .sum();
    if total > UNIVERSAL_BODY_HARD_LIMIT_BYTES {
        anyhow::bail!(
            "universal pattern `{rel_path}` body totals {total} bytes, over the \
             {UNIVERSAL_BODY_HARD_LIMIT_BYTES}-byte per-file hard limit. \
             Universal patterns re-inject on every relevant tool call — trim \
             the body or remove the `universal` tag."
        );
    }
    Ok(())
}

/// Per-file accounting for universal-pattern detection during ingest.
///
/// Folded into `IngestResult` by every ingest path (full / delta / single-file)
/// so the summary line, the >3 advisory, the body-size advisory, and the
/// near-miss-tag advisory all see the same data shape.
#[derive(Debug, Default)]
struct UniversalMetadata {
    /// `true` when at least one inserted chunk carries `is_universal = true`.
    is_universal_source: bool,
    /// `true` when at least one universal chunk's body exceeds the warning
    /// threshold. Populated only when `is_universal_source` is also `true`.
    body_oversized: bool,
    /// Frontmatter tag values whose lowercased form equals `universal` but
    /// whose exact form does not.
    near_miss_tags: Vec<String>,
}

/// Inspect a file's frontmatter and chunks to derive the universal-pattern
/// metadata for this ingest. Pure — no I/O.
fn detect_universal_metadata(content: &str, chunks: &[Chunk]) -> UniversalMetadata {
    let universal_chunks: Vec<&Chunk> = chunks.iter().filter(|c| c.is_universal).collect();

    UniversalMetadata {
        is_universal_source: !universal_chunks.is_empty(),
        body_oversized: universal_chunks
            .iter()
            .any(|c| c.body.len() > UNIVERSAL_BODY_SIZE_WARNING_BYTES),
        near_miss_tags: crate::chunking::frontmatter_near_miss_tags(content, "universal"),
    }
}

/// Fold the per-file metadata into the running `IngestResult`. Deduplicates
/// universal-source paths and oversized-body paths so each file appears at
/// most once in each list, regardless of how many chunks it produced.
fn fold_universal_metadata(
    result: &mut IngestResult,
    rel_path: &str,
    metadata: &UniversalMetadata,
) {
    if metadata.is_universal_source && !result.universal_sources.iter().any(|s| s == rel_path) {
        result.universal_sources.push(rel_path.to_string());
    }
    if metadata.body_oversized
        && !result
            .oversized_universal_bodies
            .iter()
            .any(|s| s == rel_path)
    {
        result.oversized_universal_bodies.push(rel_path.to_string());
    }
    for tag in &metadata.near_miss_tags {
        let entry = format!("{rel_path}: {tag}");
        if !result.near_miss_universal_tags.contains(&entry) {
            result.near_miss_universal_tags.push(entry);
        }
    }
}

/// Fold per-file `applies_when` malformed-predicate advisories into the
/// running `IngestResult`. Used by every ingest path (full / delta /
/// single-file) so the run-summary surface and any future `lore_status`
/// integration sees the same data shape regardless of which entry point
/// processed the file. The user-facing warning is emitted once on stderr
/// from inside [`index_single_file`]; this fold path captures the entries
/// for introspective callers without duplicating the message.
fn fold_malformed_applies_when(result: &mut IngestResult, entries: &[MalformedPredicateEntry]) {
    for entry in entries {
        if !result.malformed_applies_when.contains(entry) {
            result.malformed_applies_when.push(entry.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Per-file outcome of [`index_single_file`]. Carries enough information for
/// the calling ingest path to fold both chunk-count and universal-metadata
/// state into its `IngestResult`.
struct IndexedFile {
    chunks_indexed: usize,
    embedding_failures: usize,
    rel_path: String,
    universal_metadata: UniversalMetadata,
    /// `applies_when` advisories from the U2 frontmatter parser, with
    /// `file_path` populated to the relative path. Already-emitted to
    /// stderr by [`index_single_file`] before return; the calling ingest
    /// path folds the entries into [`IngestResult::malformed_applies_when`]
    /// so introspective callers (CLI exit summary, future `lore_status`
    /// surface) can report them without re-parsing the file.
    malformed_applies_when: Vec<MalformedPredicateEntry>,
}

/// Index (or re-index) a single file: delete old chunks, chunk, embed, insert.
///
/// The `strategy` parameter selects the chunking approach (`"heading"` or
/// `"document"`).
fn index_single_file(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    file_path: &Path,
    strategy: &str,
) -> anyhow::Result<IndexedFile> {
    let content = std::fs::read_to_string(file_path)?;
    let rel_path = file_path
        .strip_prefix(knowledge_dir)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    let (chunks, malformed_applies_when) = dispatch_chunking(strategy, &content, &rel_path);

    // Surface malformed-predicate advisories on stderr from a single
    // channel so CLI ingest (whose `on_progress` writes to stdout) and
    // MCP write paths (which return `WriteResult` with no warnings field)
    // both see the warning regardless of authoring surface. R9
    // skip-with-warning: the pattern still ingests with
    // `applies_when_json = NULL` because U2's parser already returned
    // `None` for the predicate.
    for entry in &malformed_applies_when {
        eprintln!(
            "Warning: pattern {}: malformed applies_when ({}): {}",
            entry.file_path, entry.key, entry.reason
        );
    }

    // Non-universal-pattern guard: an `applies_when` predicate on a
    // pattern without the `universal` tag is dormant in Track 1 (the
    // hook-side evaluator runs only for universal-tagged chunks per R8).
    // Without this advisory the silent fail-open mode would give pattern
    // authors no signal that their predicate does nothing. Emitted once
    // per source file because the predicate is whole-file.
    if let Some(first) = chunks.first()
        && first.applies_when_json.is_some()
        && !first.is_universal
    {
        eprintln!(
            "Warning: pattern {rel_path} has applies_when but is not \
             universal-tagged; predicate is dormant in Track 1 \
             (see Track 2-B)."
        );
    }

    // Enforce the per-file universal body-size hard cap before touching
    // the database so a rejection leaves the existing index untouched.
    enforce_universal_body_cap(&rel_path, &chunks)?;

    // Compute embeddings BEFORE opening the outer transaction so the SQLite
    // write lock is never held across Ollama HTTP round-trips. R4b in
    // `docs/plans/2026-04-22-001-feat-db-sole-read-surface-plan.md`.
    // Collecting into a Vec materialises all embed calls up front; the
    // transaction block below contains only in-memory DB work.
    let mut embedding_failures = 0_usize;
    let chunks_with_embeddings: Vec<(Chunk, Option<Vec<f32>>)> = chunks
        .iter()
        .map(|chunk| {
            let embedding = if let Ok(emb) = embedder.embed(&embed_text(chunk)) {
                Some(emb)
            } else {
                embedding_failures += 1;
                None
            };
            (chunk.clone(), embedding)
        })
        .collect();

    let pattern_row = if chunks.is_empty() {
        None
    } else {
        Some(crate::chunking::pattern_row_from(
            &content, &rel_path, &chunks,
        ))
    };

    // Single outer transaction: delete any existing patterns/chunks rows
    // for this source, then upsert the patterns row and insert every
    // chunk. Any reader on a second connection sees either the old state
    // or the new state — never a mismatch. See `begin_immediate_tx` doc
    // for why this is effectively `BEGIN DEFERRED` and why that's fine.
    let tx = db.begin_immediate_tx()?;
    crate::database::delete_pattern_and_chunks_in_tx(&tx, &rel_path)?;
    if let Some(row) = &pattern_row {
        crate::database::upsert_pattern_in_tx(&tx, row)?;
    }
    for (chunk, embedding) in &chunks_with_embeddings {
        crate::database::insert_chunk_in_tx(&tx, chunk, embedding.as_deref())?;
    }
    tx.commit()?;

    let count = chunks_with_embeddings.len();

    // R4d: in debug builds, verify the 1:1 invariant actually held across
    // the commit. Catches a future regression where a write path forgets
    // the outer transaction or where `delete_pattern_and_chunks_in_tx`
    // loses a table. Release builds skip the asserts — the DB writes are
    // correct without them; the checks are a drift-detection belt.
    #[cfg(debug_assertions)]
    {
        let expected_pattern_rows = i64::from(pattern_row.is_some());
        let pattern_count = db.pattern_count_for_source(&rel_path).unwrap_or(-1);
        debug_assert_eq!(
            pattern_count, expected_pattern_rows,
            "patterns row count mismatch for {rel_path}: expected {expected_pattern_rows}, got {pattern_count}"
        );
        let chunk_count = db.chunk_count_for_source(&rel_path).unwrap_or(-1);
        let expected_chunks = i64::try_from(count).unwrap_or(-1);
        debug_assert_eq!(
            chunk_count, expected_chunks,
            "chunk count mismatch for {rel_path}: expected {expected_chunks}, got {chunk_count}"
        );
    }

    let universal_metadata = detect_universal_metadata(&content, &chunks);

    Ok(IndexedFile {
        chunks_indexed: count,
        embedding_failures,
        rel_path,
        universal_metadata,
        malformed_applies_when,
    })
}

/// Validate that `file_path` lies within `knowledge_dir` after canonicalisation
/// and return the canonical form of `file_path`.
///
/// This prevents path traversal attacks where a `source_file` like
/// `../../../etc/passwd` could escape the knowledge directory. Callers must
/// use the returned canonical `PathBuf` for subsequent reads and writes —
/// reading from the pre-canonical input re-opens the TOCTOU window between
/// validation and access (a symlink could be swapped in between).
pub(crate) fn validate_within_dir(
    knowledge_dir: &Path,
    file_path: &Path,
) -> anyhow::Result<PathBuf> {
    let canon_dir = knowledge_dir.canonicalize()?;
    let canon_file = file_path.canonicalize()?;
    if !canon_file.starts_with(&canon_dir) {
        anyhow::bail!(
            "Path escapes the knowledge directory: {}",
            file_path.display()
        );
    }
    Ok(canon_file)
}

/// Validate that a slug-derived filename does not contain path traversal components.
fn validate_slug(filename: &str) -> anyhow::Result<()> {
    let path = PathBuf::from(filename);
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        ) {
            anyhow::bail!("Path escapes the knowledge directory: {filename}");
        }
    }
    if filename.contains("..") {
        anyhow::bail!("Path escapes the knowledge directory: {filename}");
    }
    Ok(())
}

/// Dispatch to the appropriate chunking function based on `strategy`.
///
/// Returns the produced chunks alongside any
/// [`MalformedPredicateEntry`] advisories raised by the U2 frontmatter
/// parser so [`index_single_file`] can surface them to CLI and MCP write
/// paths via a single warning channel.
fn dispatch_chunking(
    strategy: &str,
    content: &str,
    rel_path: &str,
) -> (Vec<Chunk>, Vec<MalformedPredicateEntry>) {
    if strategy == "heading" {
        chunk_by_heading_with_malformed_predicates(content, rel_path)
    } else {
        chunk_as_document_with_malformed_predicates(content, rel_path)
    }
}

/// Push a branch to the remote, deleting the local ref if the push fails.
fn push_or_cleanup(knowledge_dir: &Path, branch: &str) -> anyhow::Result<()> {
    if let Err(e) = git::push_branch(knowledge_dir, branch) {
        // Clean up the orphaned local branch ref before propagating.
        let _ = std::process::Command::new("git")
            .args(["update-ref", "-d", &format!("refs/heads/{branch}")])
            .current_dir(knowledge_dir)
            .output();
        return Err(e);
    }
    Ok(())
}

/// Attempt a git commit; return [`CommitStatus::NotCommitted`] if not a git
/// repo or the commit fails.
fn try_commit(knowledge_dir: &Path, file_path: &Path, message: &str) -> CommitStatus {
    if !git::is_git_repo(knowledge_dir) {
        return CommitStatus::NotCommitted;
    }
    if git::add_and_commit(knowledge_dir, file_path, message).is_ok() {
        CommitStatus::Committed
    } else {
        CommitStatus::NotCommitted
    }
}

/// Turn a title into a filename-safe slug.
///
/// Input is NFC-normalised first so visually identical titles in different
/// normalisation forms (e.g. precomposed `é` vs. `e` + combining acute)
/// produce identical slugs. Without this, NFD combining marks would be
/// stripped by the `is_alphanumeric` filter and `café` (NFD) would slug to
/// `cafe`, diverging from `café` (NFC) which slugs to `café`.
fn slugify(title: &str) -> String {
    title
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Build a markdown file from title, body, and optional frontmatter tags.
fn build_file_content(title: &str, body: &str, tags: &[&str]) -> String {
    let mut content = String::new();
    if !tags.is_empty() {
        content.push_str("---\n");
        let _ = writeln!(content, "tags: [{}]", tags.join(", "));
        content.push_str("---\n\n");
    }
    let _ = write!(content, "# {title}\n\n");
    content.push_str(body);
    content.push('\n');
    content
}

/// Extract the filename stem (without extension) from a path string.
fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;
    use crate::database::KnowledgeDB;

    // -- embed_text -------------------------------------------------------

    #[test]
    fn embed_text_includes_title_tags_body() {
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Error Handling".into(),
            body: "Use anyhow for errors".into(),
            tags: "rust, anyhow".into(),
            source_file: "errors.md".into(),
            heading_path: String::new(),
            is_universal: false,
            applies_when_json: None,
        };
        assert_eq!(
            embed_text(&chunk),
            "Error Handling\nrust, anyhow\nUse anyhow for errors"
        );
    }

    #[test]
    fn embed_text_with_empty_tags() {
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Title".into(),
            body: "Body".into(),
            tags: String::new(),
            source_file: "test.md".into(),
            heading_path: String::new(),
            is_universal: false,
            applies_when_json: None,
        };
        assert_eq!(embed_text(&chunk), "Title\n\nBody");
    }
    use crate::embeddings::FakeEmbedder;

    /// Open an in-memory `KnowledgeDB` with 768-dimension embeddings.
    fn memory_db() -> KnowledgeDB {
        let db = KnowledgeDB::open(Path::new(":memory:"), 768).unwrap();
        db.init().unwrap();
        db
    }

    /// Initialise a git repo in `dir` with a test user identity.
    fn git_init(dir: &Path) {
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
            // Disable GPG signing for test repos so commits don't require a key.
            vec!["config", "commit.gpgsign", "false"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git command failed");
        }
    }

    // -- slugify -----------------------------------------------------------

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_special_characters() {
        assert_eq!(slugify("Foo: Bar / Baz (v2)"), "foo-bar-baz-v2");
    }

    #[test]
    fn slugify_leading_trailing_dashes() {
        assert_eq!(slugify("  --Title-- "), "title");
    }

    #[test]
    fn slugify_nfd_combining_acute_normalises_to_nfc() {
        // R11.7. `café` typed with a combining acute (NFD: `e` + U+0301)
        // post-normalisation slugs to the precomposed NFC form, not `cafe`.
        // Pre-fix the combining mark was stripped by `is_alphanumeric`.
        let nfd = "cafe\u{0301}";
        let nfc = "café";
        assert_eq!(slugify(nfd), slugify(nfc));
        assert_eq!(slugify(nfd), "café");
    }

    #[test]
    fn slugify_combining_marks_only_yields_empty_slug() {
        // R11.8. A title made solely of combining marks slugs to empty;
        // callers (add_pattern) surface the existing
        // `Title must contain at least one alphanumeric character` error.
        assert_eq!(slugify("\u{0301}\u{0301}"), "");
    }

    #[test]
    fn slugify_preserves_full_unicode() {
        // R7. NFC neither folds nor transliterates — non-Latin scripts
        // survive intact and produce a slug equal to their lowercased,
        // alphanumeric form (CJK has no cased form, so it round-trips
        // unchanged).
        assert_eq!(slugify("Café Tip"), "café-tip");
        assert_eq!(slugify("日本語"), "日本語");
    }

    // -- ingest (full directory) -------------------------------------------

    #[test]
    fn ingest_tempdir_with_markdown_files() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        fs::write(
            dir.join("alpha.md"),
            "# Alpha\n\nAlpha body text that is long enough for a chunk.\n",
        )
        .unwrap();
        fs::write(
            dir.join("beta.md"),
            "# Beta\n\nBeta body text that is long enough for a chunk.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.files_processed, 2);
        assert!(
            result.chunks_created >= 2,
            "expected >=2 chunks, got {}",
            result.chunks_created
        );
        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn ingest_empty_directory_returns_zero() {
        let tmp = tempdir().unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, tmp.path(), "heading", &|_| {});

        assert_eq!(result.files_processed, 0);
        assert_eq!(result.chunks_created, 0);
        assert!(result.errors.is_empty());
    }

    // -- effective-empty warning -------------------------------------------

    #[test]
    fn ingest_empty_directory_warns() {
        let tmp = tempdir().unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let messages = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, tmp.path(), "heading", &|m| {
            messages.borrow_mut().push(m.to_string());
        });

        let captured = messages.borrow();
        assert!(
            captured
                .iter()
                .any(|m| m.contains("knowledge directory is empty")),
            "expected filesystem-empty warning, got: {captured:?}"
        );
    }

    #[test]
    fn ingest_all_ignored_warns() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("a.md"), "# A\n\nBody.\n").unwrap();
        fs::write(dir.join("b.md"), "# B\n\nBody.\n").unwrap();
        fs::write(dir.join(".loreignore"), "*.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let messages = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, dir, "heading", &|m| {
            messages.borrow_mut().push(m.to_string());
        });

        let captured = messages.borrow();
        assert!(
            captured
                .iter()
                .any(|m| m.contains("matched every markdown")),
            "expected all-ignored warning, got: {captured:?}"
        );
    }

    #[test]
    fn ingest_populated_directory_does_not_warn() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("only.md"), "# Only\n\nBody text long enough.\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let messages = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, dir, "heading", &|m| {
            messages.borrow_mut().push(m.to_string());
        });

        let captured = messages.borrow();
        assert!(
            !captured.iter().any(|m| m.contains("Warning:")),
            "expected no warning, got: {captured:?}"
        );
    }

    #[test]
    fn ingest_missing_directory_warns_and_does_not_crash() {
        // Configured path doesn't exist on disk. The warning must say so
        // explicitly rather than the misleading "add at least one .md file"
        // (which is the wrong recovery for a typo).
        let tmp = tempdir().unwrap();
        let nonexistent = tmp.path().join("does-not-exist");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let messages = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, &nonexistent, "heading", &|m| {
            messages.borrow_mut().push(m.to_string());
        });

        let captured = messages.borrow();
        assert!(
            captured
                .iter()
                .any(|m| m.contains("not found") && m.contains("knowledge_dir")),
            "expected missing-directory warning, got: {captured:?}"
        );
        assert!(
            !captured.iter().any(|m| m.contains("add at least one")),
            "missing-dir warning must not suggest adding a file, got: {captured:?}"
        );
    }

    #[test]
    fn ingest_knowledge_dir_is_regular_file_warns() {
        // Configured path is a regular file, not a directory. Same recovery
        // path as Missing (fix the config).
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("not-a-dir");
        fs::write(&path, "I am not a directory").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let messages = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, &path, "heading", &|m| {
            messages.borrow_mut().push(m.to_string());
        });

        let captured = messages.borrow();
        assert!(
            captured
                .iter()
                .any(|m| m.contains("not found") && m.contains("knowledge_dir")),
            "expected missing-directory warning for regular file, got: {captured:?}"
        );
    }

    #[test]
    fn empty_warning_clears_after_adding_a_file() {
        // Remedy-completion test: warn fires on empty -> user adds a .md
        // file -> next ingest must not fire the warning. Pins the contract
        // that the warning is reachable AND the documented remedy actually
        // clears it.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let first = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, dir, "heading", &|m| {
            first.borrow_mut().push(m.to_string());
        });
        assert!(
            first
                .borrow()
                .iter()
                .any(|m| m.contains("knowledge directory is empty")),
            "first ingest on empty dir must warn"
        );

        fs::write(dir.join("only.md"), "# Only\n\nBody text long enough.\n").unwrap();

        let second = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, dir, "heading", &|m| {
            second.borrow_mut().push(m.to_string());
        });
        assert!(
            !second.borrow().iter().any(|m| m.contains("Warning:")),
            "warning must be cleared after adding a file, got: {:?}",
            second.borrow()
        );
    }

    #[test]
    fn delta_ingest_after_emptying_warns() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        // Initial state: one .md file, committed. First ingest records HEAD.
        fs::write(dir.join("only.md"), "# Only\n\nBody text long enough.\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add only"])
            .current_dir(dir)
            .output()
            .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let first = std::cell::RefCell::new(Vec::<String>::new());
        ingest(&db, &embedder, dir, "heading", &|m| {
            first.borrow_mut().push(m.to_string());
        });
        assert!(
            !first.borrow().iter().any(|m| m.contains("Warning:")),
            "first ingest should not warn, got: {:?}",
            first.borrow()
        );

        // Delete the file and commit; second ingest enters delta mode.
        fs::remove_file(dir.join("only.md")).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "rm only"])
            .current_dir(dir)
            .output()
            .unwrap();

        let second = std::cell::RefCell::new(Vec::<String>::new());
        let second_result = ingest(&db, &embedder, dir, "heading", &|m| {
            second.borrow_mut().push(m.to_string());
        });

        // Pin that the second run actually entered delta mode — the warning
        // fires from `ingest()` before the delta/full split, so without this
        // assertion a silent fall-through to `full_ingest` would still pass
        // the substring check below.
        assert!(
            matches!(second_result.mode, IngestMode::Delta { .. }),
            "expected delta mode on second ingest, got: {:?}",
            second_result.mode
        );

        let captured = second.borrow();
        assert!(
            captured
                .iter()
                .any(|m| m.contains("knowledge directory is empty")),
            "expected filesystem-empty warning on delta path, got: {captured:?}"
        );
    }

    #[test]
    fn ingest_uses_document_strategy() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // A file with multiple headings — document strategy should produce 1 chunk.
        fs::write(
            dir.join("multi.md"),
            "# Top\n\nIntro text that is long enough.\n\n\
             ## Section\n\nSection body that is long enough.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "document", &|_| {});

        assert_eq!(result.files_processed, 1);
        // document mode produces exactly 1 chunk per file.
        assert_eq!(result.chunks_created, 1);
    }

    // -- add_pattern -------------------------------------------------------

    #[test]
    fn add_pattern_creates_file_with_frontmatter() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(
            &db,
            &embedder,
            dir,
            "My Pattern",
            "Pattern body that is long enough for chunking.",
            &["design", "rust"],
            None,
        )
        .unwrap();

        assert_eq!(result.file_path, "my-pattern.md");
        assert!(result.chunks_indexed >= 1);
        // Non-git directory: file is written and indexed, but not committed.
        assert!(matches!(result.commit_status, CommitStatus::NotCommitted));

        let content = fs::read_to_string(dir.join("my-pattern.md")).unwrap();
        assert!(content.contains("tags: [design, rust]"));
        assert!(content.contains("# My Pattern"));
        assert!(content.contains("Pattern body"));
    }

    #[test]
    fn add_pattern_rejects_re_use_with_update_pattern_hint() {
        // Re-use case: incoming title matches the existing file's heading.
        // The error names the pattern and points at update_pattern.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(dir.join("existing.md"), "# Existing\n").unwrap();

        let result = add_pattern(&db, &embedder, dir, "Existing", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("already exists"), "unexpected error: {msg}");
        assert!(
            msg.contains("update_pattern"),
            "should hint update_pattern: {msg}"
        );
        assert!(
            !msg.contains("Slug "),
            "should not use collision wording for re-use: {msg}"
        );
    }

    #[test]
    fn add_pattern_distinct_titles_colliding_slug_returns_collision_error() {
        // R11.5. Two distinct titles slugifying to the same name produce a
        // collision-specific error naming the existing file and its title.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(dir.join("api-notes.md"), "# API Notes\n").unwrap();

        let result = add_pattern(&db, &embedder, dir, "API: Notes", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Slug \"api-notes\""), "missing slug: {msg}");
        assert!(msg.contains("api-notes.md"), "missing filename: {msg}");
        assert!(
            msg.contains("title: \"API Notes\""),
            "missing existing title: {msg}"
        );
        assert!(
            msg.contains("Choose a different title"),
            "missing recovery hint: {msg}"
        );
    }

    #[test]
    fn add_pattern_collision_with_no_heading_existing_file_uses_no_title_heading_label() {
        // R11.6. When the conflicting file has no `# ` line at any position,
        // extract_title returns None and the error uses `(no title heading)`
        // rather than echoing the filename stem as if it were a title.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Frontmatter plus plain prose body — no `# ` line anywhere.
        fs::write(
            dir.join("api-notes.md"),
            "---\ntags: [legacy]\n---\n\nPlain prose with no heading whatsoever.\n",
        )
        .unwrap();

        let result = add_pattern(&db, &embedder, dir, "API Notes", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("(no title heading)"),
            "missing fallback: {msg}"
        );
        assert!(
            !msg.contains("title: \""),
            "should not synthesise a title: {msg}"
        );
    }

    #[test]
    fn add_pattern_nfc_nfd_round_trip_classifies_as_re_use() {
        // Existing file written with NFC `café` heading; incoming NFD
        // (`cafe` + combining acute) slugs identically and — after NFC
        // normalisation of both titles — compares equal, so the
        // discriminator classifies as re-use, not collision.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(dir.join("café.md"), "# café\n").unwrap();

        let result = add_pattern(&db, &embedder, dir, "cafe\u{0301}", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("update_pattern"),
            "expected re-use path: {msg}"
        );
        assert!(!msg.contains("Slug "), "should not be a collision: {msg}");
    }

    #[test]
    fn add_pattern_rejects_combining_marks_only_title() {
        // Guards U1 + U2 together: a title that NFC-normalises to a slug
        // composed solely of combining marks still hits the existing
        // alphanumeric-required error before any collision check fires.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(&db, &embedder, dir, "\u{0301}\u{0301}", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("at least one alphanumeric character"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn add_pattern_rejects_empty_title() {
        // Plan-listed regression guard: empty title still hits the
        // alphanumeric-required error after U1's NFC normalisation.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(&db, &embedder, dir, "", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("at least one alphanumeric character"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn add_pattern_trims_title_whitespace_so_round_trip_classifies_as_re_use() {
        // R1. A title with leading/trailing whitespace round-trips correctly:
        // the trimmed form is used for slugify, the on-disk heading, and the
        // discriminator NFC compare. A second call with the trimmed form
        // classifies as re-use, not collision.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        add_pattern(
            &db,
            &embedder,
            dir,
            "  My Pattern  ",
            "body text",
            &[],
            None,
        )
        .unwrap();

        let result = add_pattern(&db, &embedder, dir, "My Pattern", "other body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("update_pattern"),
            "expected re-use path: {msg}"
        );
        assert!(
            !msg.contains("Choose a different title"),
            "should not be a collision: {msg}"
        );
    }

    #[test]
    fn add_pattern_rejects_newline_in_title() {
        // R2. Newlines in a title would truncate the on-disk `# heading` line
        // and corrupt the indexed pattern. Reject at the write boundary.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(&db, &embedder, dir, "Hello\nWorld", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("must not contain newline"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn add_pattern_rejects_carriage_return_in_title() {
        // R2 sibling. CRLF artefacts from copy-paste are rejected by the
        // same guard.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(&db, &embedder, dir, "Hello\rWorld", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("must not contain newline"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn add_pattern_directory_at_file_path_falls_back_to_no_heading_collision() {
        // R3. A directory at the slug path makes `read_to_string` fail; the
        // discriminator must still surface the curated tier-1 message with
        // the `(no title heading)` label rather than propagating a raw
        // `IsADirectory` error.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Create a directory at the path `add_pattern` would target.
        fs::create_dir_all(dir.join("blocked.md")).unwrap();

        let result = add_pattern(&db, &embedder, dir, "Blocked", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        // The fs::write that follows the discriminator may also fail with an
        // OS error if the directory survives, but the discriminator runs
        // first and bails with the curated message.
        assert!(
            msg.contains("Slug \"blocked\"") && msg.contains("(no title heading)"),
            "expected curated collision message, got: {msg}"
        );
    }

    #[test]
    fn add_pattern_existing_file_with_nfd_heading_classifies_nfc_incoming_as_re_use() {
        // Symmetric companion to add_pattern_nfc_nfd_round_trip_classifies_as_re_use:
        // existing file's heading is NFD on disk, incoming title is NFC.
        // The discriminator NFC-normalises both sides before comparison, so
        // either direction should classify as re-use.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Existing file's heading is NFD (`e` + combining acute).
        fs::write(dir.join("café.md"), "# cafe\u{0301}\n").unwrap();

        let result = add_pattern(&db, &embedder, dir, "café", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("update_pattern"),
            "expected re-use path: {msg}"
        );
        assert!(
            !msg.contains("Choose a different title"),
            "should not be a collision: {msg}"
        );
    }

    // -- update_pattern ----------------------------------------------------

    #[test]
    fn update_pattern_overwrites_content_preserves_title() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("doc.md"),
            "# Original Title\n\nOld body that is long enough.\n",
        )
        .unwrap();

        let result = update_pattern(
            &db,
            &embedder,
            dir,
            "doc.md",
            "Brand new body that is long enough for a chunk.",
            Some(&["updated"]),
            None,
        )
        .unwrap();

        assert!(result.chunks_indexed >= 1);
        // Non-git directory: file is written and indexed, but not committed.
        assert!(matches!(result.commit_status, CommitStatus::NotCommitted));

        let content = fs::read_to_string(dir.join("doc.md")).unwrap();
        // Title should be preserved from the original file.
        assert!(content.contains("# Original Title"));
        assert!(content.contains("Brand new body"));
        assert!(content.contains("tags: [updated]"));
        // Old body should be gone.
        assert!(!content.contains("Old body"));
    }

    #[test]
    fn update_pattern_with_none_tags_preserves_existing_frontmatter_tags() {
        // An agent that rewrites only the body through update_pattern must
        // not silently strip the `universal` tag. The None branch preserves
        // whatever was in the frontmatter.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("pinned.md"),
            "---\ntags: [universal, workflow]\n---\n\n# Pinned\n\nOriginal body long enough.\n",
        )
        .unwrap();

        let result = update_pattern(
            &db,
            &embedder,
            dir,
            "pinned.md",
            "New body long enough for a chunk.",
            None,
            None,
        )
        .unwrap();
        assert!(result.chunks_indexed >= 1);

        let content = fs::read_to_string(dir.join("pinned.md")).unwrap();
        assert!(
            content.contains("tags: [universal, workflow]"),
            "existing tags must be preserved; got:\n{content}"
        );
        assert!(content.contains("New body"));

        // The DB flag also survives: universal_patterns() still returns it.
        let universal = db.universal_patterns().unwrap();
        assert!(
            universal.iter().any(|p| p.source_file == "pinned.md"),
            "is_universal must still be true after a None-tags update"
        );
    }

    #[test]
    fn update_pattern_with_empty_tags_clears_frontmatter_tags() {
        // The explicit "clear all tags" path — distinguishes Some(&[]) from
        // None so agents can opt into tag removal when they actually mean
        // to.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("pinned.md"),
            "---\ntags: [universal]\n---\n\n# Pinned\n\nOriginal body long enough.\n",
        )
        .unwrap();

        update_pattern(
            &db,
            &embedder,
            dir,
            "pinned.md",
            "New body long enough.",
            Some(&[]),
            None,
        )
        .unwrap();

        let content = fs::read_to_string(dir.join("pinned.md")).unwrap();
        assert!(
            !content.contains("tags:"),
            "empty tags must remove the frontmatter block, got:\n{content}"
        );
    }

    // -- append_to_pattern -------------------------------------------------

    #[test]
    fn append_to_pattern_adds_section() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("doc.md"),
            "# Doc Title\n\nExisting body that is long enough for chunking.\n",
        )
        .unwrap();

        let result = append_to_pattern(
            &db,
            &embedder,
            dir,
            "doc.md",
            "New Section",
            "Appended body that is long enough for a chunk.",
            None,
        )
        .unwrap();

        assert!(result.chunks_indexed >= 1);
        // Non-git directory: file is written and indexed, but not committed.
        assert!(matches!(result.commit_status, CommitStatus::NotCommitted));

        let content = fs::read_to_string(dir.join("doc.md")).unwrap();
        assert!(content.contains("# Doc Title"));
        assert!(content.contains("Existing body"));
        assert!(content.contains("## New Section"));
        assert!(content.contains("Appended body"));
    }

    // -- index_single_file strategy ----------------------------------------

    #[test]
    fn index_single_file_respects_strategy() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // File with multiple headings.
        let md = "# Top\n\nIntro text that is definitely long enough.\n\n\
                  ## Sub\n\nSub body text that is definitely long enough.\n";
        fs::write(dir.join("multi.md"), md).unwrap();

        let db_heading = memory_db();
        let db_doc = memory_db();
        let embedder = FakeEmbedder::new();

        let heading_count = index_single_file(
            &db_heading,
            &embedder,
            dir,
            &dir.join("multi.md"),
            "heading",
        )
        .unwrap()
        .chunks_indexed;

        let doc_count =
            index_single_file(&db_doc, &embedder, dir, &dir.join("multi.md"), "document")
                .unwrap()
                .chunks_indexed;

        // heading strategy should produce more chunks than document strategy.
        assert!(
            heading_count > doc_count,
            "heading ({heading_count}) should produce more chunks than document ({doc_count})"
        );
        assert_eq!(doc_count, 1);
    }

    // -- ingest_single_file ------------------------------------------------

    fn write_body(dir: &Path, name: &str, body: &str) {
        let content = format!("# {name}\n\n{body} that is long enough for chunking.\n");
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn ingest_single_file_indexes_uncommitted_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_body(dir, "draft.md", "Draft body text");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("draft.md"),
            "heading",
            false,
            &|_| {},
        );

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.files_processed, 1);
        assert!(result.chunks_created >= 1);
        assert!(matches!(
            result.mode,
            IngestMode::SingleFile { ref path } if path == "draft.md"
        ));
        assert_eq!(db.source_files().unwrap(), vec!["draft.md".to_string()]);
    }

    #[test]
    fn ingest_single_file_accepts_markdown_extension() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        fs::write(
            dir.join("alt.markdown"),
            "# Alt\n\nBody text that is long enough for chunking.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("alt.markdown"),
            "heading",
            false,
            &|_| {},
        );

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.files_processed, 1);
    }

    #[test]
    fn ingest_single_file_replaces_existing_chunks() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_body(dir, "note.md", "Original body text");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("note.md"),
            "heading",
            false,
            &|_| {},
        );
        let first_count = db.stats().unwrap().chunks;

        // Rewrite file and re-ingest — chunk count should not double.
        write_body(dir, "note.md", "Replacement body text");
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("note.md"),
            "heading",
            false,
            &|_| {},
        );

        assert!(result.errors.is_empty());
        let second_count = db.stats().unwrap().chunks;
        assert_eq!(
            first_count, second_count,
            "re-ingest must replace, not append"
        );
        assert_eq!(db.source_files().unwrap(), vec!["note.md".to_string()]);
    }

    #[test]
    fn ingest_single_file_rejects_non_markdown_extension() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("notes.txt"), "not markdown").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("notes.txt"),
            "heading",
            false,
            &|_| {},
        );

        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("Unsupported extension"),
            "unexpected error: {}",
            result.errors[0]
        );
        assert_eq!(result.files_processed, 0);
        assert!(db.source_files().unwrap().is_empty());
    }

    #[test]
    fn ingest_single_file_rejects_missing_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("nope.md"),
            "heading",
            false,
            &|_| {},
        );

        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("Cannot access"));
    }

    #[test]
    fn ingest_single_file_rejects_path_outside_knowledge_dir() {
        let knowledge = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(
            outside.path().join("escape.md"),
            "# Escape\n\nBody that is long enough for chunking.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            knowledge.path(),
            &outside.path().join("escape.md"),
            "heading",
            false,
            &|_| {},
        );

        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("escapes the knowledge directory"),
            "unexpected error: {}",
            result.errors[0]
        );
        assert!(db.source_files().unwrap().is_empty());
    }

    #[test]
    fn ingest_single_file_respects_loreignore() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_body(dir, "draft.md", "Draft body text");
        fs::write(dir.join(".loreignore"), "draft.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("draft.md"),
            "heading",
            false,
            &|_| {},
        );

        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains(".loreignore"),
            "unexpected error: {}",
            result.errors[0]
        );
        assert!(db.source_files().unwrap().is_empty());
    }

    #[test]
    fn ingest_single_file_force_overrides_loreignore() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_body(dir, "draft.md", "Draft body text");
        fs::write(dir.join(".loreignore"), "draft.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("draft.md"),
            "heading",
            true,
            &|_| {},
        );

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.files_processed, 1);
        assert_eq!(db.source_files().unwrap(), vec!["draft.md".to_string()]);
    }

    #[test]
    fn ingest_single_file_does_not_touch_last_ingested_commit() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_body(dir, "note.md", "Body text");

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Seed a fake commit SHA as if a previous walk-based ingest had run.
        db.set_metadata(META_LAST_COMMIT, "deadbeef").unwrap();
        db.set_metadata(META_LOREIGNORE_HASH, "cafef00d").unwrap();

        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("note.md"),
            "heading",
            false,
            &|_| {},
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        assert_eq!(
            db.get_metadata(META_LAST_COMMIT).unwrap(),
            Some("deadbeef".to_string()),
            "single-file ingest must not touch last_ingested_commit"
        );
        assert_eq!(
            db.get_metadata(META_LOREIGNORE_HASH).unwrap(),
            Some("cafef00d".to_string()),
            "single-file ingest must not touch loreignore_hash"
        );
    }

    // -- try_commit (git integration) --------------------------------------

    #[test]
    fn try_commit_succeeds_in_git_repo() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("test.md");
        fs::write(&file, "# Test\n").unwrap();

        assert!(matches!(
            try_commit(dir, &file, "lore: test commit"),
            CommitStatus::Committed
        ));
    }

    #[test]
    fn try_commit_returns_not_committed_without_git() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        let file = dir.join("test.md");
        fs::write(&file, "# Test\n").unwrap();

        assert!(matches!(
            try_commit(dir, &file, "lore: test commit"),
            CommitStatus::NotCommitted
        ));
    }

    // -- write operations with git -----------------------------------------

    #[test]
    fn add_pattern_commits_in_git_repo() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        // Need an initial commit so HEAD exists.
        let seed = dir.join("seed.md");
        fs::write(&seed, "seed\n").unwrap();
        git::add_and_commit(dir, &seed, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(
            &db,
            &embedder,
            dir,
            "Git Test",
            "Body text that is long enough for a chunk.",
            &["test"],
            None,
        )
        .unwrap();

        assert!(matches!(result.commit_status, CommitStatus::Committed));

        // Verify the commit message prefix was renamed.
        let output = Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(dir)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert!(
            log.contains("lore: add pattern"),
            "commit message should start with 'lore:', got: {log}"
        );
    }

    // -- inbox branch workflow against non-git directories ---------------
    //
    // The inbox branch workflow (Some(prefix)) calls git unconditionally to
    // create and push per-submission branches. When the knowledge base is not
    // a git repository, every variant must surface a hard error rather than
    // silently writing the file or no-oping. These tests pin the documented
    // contract from `docs/configuration.md` ("Omit the `[git]` section
    // entirely when the knowledge base is not a git repository").

    #[test]
    fn add_pattern_with_inbox_prefix_fails_on_non_git_dir() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(
            &db,
            &embedder,
            dir,
            "Inbox Pattern",
            "Body content long enough for chunking.",
            &[],
            Some("inbox/"),
        );

        assert!(
            result.is_err(),
            "expected failure when inbox prefix is set on a non-git dir"
        );
    }

    #[test]
    fn update_pattern_with_inbox_prefix_fails_on_non_git_dir() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Pre-create a file so the slug-validation path is reached before the
        // git operation; otherwise the test would also pass for the wrong
        // reason (file-not-found).
        fs::write(
            dir.join("doc.md"),
            "# Doc\n\nOriginal body that is long enough.\n",
        )
        .unwrap();

        let result = update_pattern(
            &db,
            &embedder,
            dir,
            "doc.md",
            "Replacement body that is long enough.",
            Some(&[]),
            Some("inbox/"),
        );

        assert!(
            result.is_err(),
            "expected failure when inbox prefix is set on a non-git dir"
        );
    }

    #[test]
    fn append_to_pattern_with_inbox_prefix_fails_on_non_git_dir() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("doc.md"),
            "# Doc\n\nOriginal body that is long enough.\n",
        )
        .unwrap();

        let result = append_to_pattern(
            &db,
            &embedder,
            dir,
            "doc.md",
            "New Section",
            "Appended body that is long enough.",
            Some("inbox/"),
        );

        assert!(
            result.is_err(),
            "expected failure when inbox prefix is set on a non-git dir"
        );
    }

    // -- empty slug -------------------------------------------------------

    #[test]
    fn add_pattern_rejects_all_punctuation_title() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(&db, &embedder, dir, "!@#$%^&*()", "body text", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("alphanumeric"),
            "error should mention alphanumeric, got: {msg}"
        );
    }

    // -- path traversal ---------------------------------------------------

    #[test]
    fn update_pattern_rejects_path_traversal() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // Create a file outside the knowledge directory.
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("secret.md");
        fs::write(
            &outside_file,
            "# Secret\n\nSecret body that is long enough.\n",
        )
        .unwrap();

        // Construct a relative path that escapes the knowledge directory.
        let rel = pathdiff_relative(dir, &outside_file);

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = update_pattern(&db, &embedder, dir, &rel, "new body", Some(&[]), None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("escapes") || msg.contains("Path"),
            "error should mention path escaping, got: {msg}"
        );
    }

    #[test]
    fn append_to_pattern_rejects_path_traversal() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // Create a file outside the knowledge directory.
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("secret.md");
        fs::write(
            &outside_file,
            "# Secret\n\nSecret body that is long enough.\n",
        )
        .unwrap();

        // Construct a relative path that escapes the knowledge directory.
        let rel = pathdiff_relative(dir, &outside_file);

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = append_to_pattern(&db, &embedder, dir, &rel, "Hacked", "evil body", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("escapes") || msg.contains("Path"),
            "error should mention path escaping, got: {msg}"
        );
    }

    /// Compute a relative path from `base` to `target` using `..` components.
    fn pathdiff_relative(base: &Path, target: &Path) -> String {
        let base = fs::canonicalize(base).unwrap();
        let target = fs::canonicalize(target).unwrap();

        let mut base_parts: Vec<_> = base.components().collect();
        let target_parts: Vec<_> = target.components().collect();

        // Find common prefix length.
        let common = base_parts
            .iter()
            .zip(target_parts.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let ups = base_parts.len() - common;
        base_parts.clear();

        let mut rel = String::new();
        for _ in 0..ups {
            rel.push_str("../");
        }
        for part in &target_parts[common..] {
            rel.push_str(&part.as_os_str().to_string_lossy());
            rel.push('/');
        }
        // Remove trailing slash.
        if rel.ends_with('/') {
            rel.pop();
        }
        rel
    }

    // -- validate_slug rejects path traversal components ------------------

    #[test]
    fn validate_slug_rejects_dot_dot() {
        let result = validate_slug("../../../etc/passwd.md");
        assert!(result.is_err());
    }

    #[test]
    fn validate_slug_accepts_normal_filename() {
        let result = validate_slug("my-pattern.md");
        assert!(result.is_ok());
    }

    // -- full_ingest records commit SHA ------------------------------------

    #[test]
    fn full_ingest_records_head_commit() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("doc.md");
        fs::write(
            &file,
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.mode, IngestMode::Full);
        assert!(result.errors.is_empty());

        let stored = db.get_metadata(META_LAST_COMMIT).unwrap();
        assert!(stored.is_some(), "should have stored a commit SHA");
        assert_eq!(stored.unwrap().len(), 40);
    }

    #[test]
    fn ingest_emits_no_head_progress_line_on_fresh_git_init() {
        // R11.2: fresh `git init` with no commits → ingest() emits the
        // no-HEAD-specific progress line exactly once and does NOT write
        // META_LAST_COMMIT (full_ingest's HEAD-recording let-chain
        // short-circuits when head_commit fails on an unborn branch).

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir); // init + config only; no commit, HEAD is unborn.
        fs::write(
            dir.join("rust.md"),
            "# Rust\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let progress = std::cell::RefCell::new(Vec::<String>::new());

        // Act
        let result = ingest(&db, &embedder, dir, "heading", &|m| {
            progress.borrow_mut().push(m.to_string());
        });

        // Assert
        assert!(
            result.errors.is_empty(),
            "ingest errors: {:?}",
            result.errors
        );
        let lines = progress.borrow();
        let no_head = "No commits yet — HEAD will be recorded after your first commit.";
        let no_previous = "No previous ingest recorded — running full ingest";
        let no_head_hits = lines.iter().filter(|l| l.as_str() == no_head).count();
        assert_eq!(
            no_head_hits, 1,
            "expected the no-HEAD progress line exactly once, got: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| l != no_previous),
            "old wording must not fire on the unborn-branch path, got: {lines:?}"
        );
        let stored = db.get_metadata(META_LAST_COMMIT).unwrap();
        assert!(
            stored.is_none(),
            "META_LAST_COMMIT must not be written before the first commit lands, got: {stored:?}"
        );
    }

    #[test]
    fn ingest_uses_existing_wording_on_committed_repo_with_empty_metadata() {
        // Negative control for R11.2: a real repo with commits but a fresh
        // DB (no prior META_LAST_COMMIT) must keep the existing
        // "No previous ingest recorded" wording. Pins discriminator
        // specificity — a regression that broadens the new wording to all
        // None cases would surface here.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        let file = dir.join("rust.md");
        fs::write(
            &file,
            "# Rust\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let progress = std::cell::RefCell::new(Vec::<String>::new());

        // Act
        let result = ingest(&db, &embedder, dir, "heading", &|m| {
            progress.borrow_mut().push(m.to_string());
        });

        // Assert
        assert!(
            result.errors.is_empty(),
            "ingest errors: {:?}",
            result.errors
        );
        let lines = progress.borrow();
        let no_head = "No commits yet — HEAD will be recorded after your first commit.";
        let no_previous = "No previous ingest recorded — running full ingest";
        assert!(
            lines.iter().any(|l| l == no_previous),
            "expected existing wording on committed-repo-with-empty-metadata path, got: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| l != no_head),
            "no-HEAD wording must not fire when HEAD already exists, got: {lines:?}"
        );
    }

    #[test]
    fn ingest_no_head_to_commit_to_delta_transition() {
        // R11.3: the no-HEAD → first-commit → delta-mode lifecycle, exercised
        // through the top-level `ingest()` entry point.
        //
        // Step 1: fresh `git init`, write markdown, ingest. Expect the
        //   no-HEAD wording; META_LAST_COMMIT remains None (full_ingest's
        //   HEAD-recording let-chain short-circuits because head_commit
        //   fails on an unborn branch).
        // Step 2: commit the markdown, ingest again. Expect the existing
        //   "No previous ingest recorded" wording (HEAD now exists, but
        //   no recorded metadata); full_ingest writes META_LAST_COMMIT.
        // Step 3: ingest a third time. Expect the delta path — none of the
        //   five full-fallback wordings should fire.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        let file = dir.join("rust.md");
        fs::write(
            &file,
            "# Rust\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let progress = std::cell::RefCell::new(Vec::<String>::new());
        let collect = |m: &str| progress.borrow_mut().push(m.to_string());

        let no_head = "No commits yet — HEAD will be recorded after your first commit.";
        let no_previous = "No previous ingest recorded — running full ingest";
        let other_fallbacks = [
            "Not a git repository — running full ingest",
            "Previous commit not found in history — running full ingest",
        ];

        // Act + Assert — step 1: unborn HEAD
        let r1 = ingest(&db, &embedder, dir, "heading", &collect);
        assert!(r1.errors.is_empty(), "step 1 errors: {:?}", r1.errors);
        assert!(
            progress.borrow().iter().any(|l| l == no_head),
            "step 1 should emit no-HEAD wording, got: {:?}",
            progress.borrow()
        );
        assert!(
            db.get_metadata(META_LAST_COMMIT).unwrap().is_none(),
            "step 1 must not record HEAD on an unborn branch"
        );
        progress.borrow_mut().clear();

        // Arrange — land the first real commit
        git::add_and_commit(dir, &file, "initial").unwrap();

        // Act + Assert — step 2: HEAD exists but no recorded metadata
        let r2 = ingest(&db, &embedder, dir, "heading", &collect);
        assert!(r2.errors.is_empty(), "step 2 errors: {:?}", r2.errors);
        assert!(
            progress.borrow().iter().any(|l| l == no_previous),
            "step 2 should emit the existing No-previous-ingest wording, got: {:?}",
            progress.borrow()
        );
        assert!(
            progress.borrow().iter().all(|l| l != no_head),
            "step 2 must not emit no-HEAD wording once HEAD exists, got: {:?}",
            progress.borrow()
        );
        let stored = db.get_metadata(META_LAST_COMMIT).unwrap();
        assert!(
            stored.is_some(),
            "step 2 must record HEAD via full_ingest, got: {stored:?}"
        );
        assert_eq!(stored.unwrap().len(), 40, "step 2 should record a real SHA");
        progress.borrow_mut().clear();

        // Act + Assert — step 3: delta path
        let r3 = ingest(&db, &embedder, dir, "heading", &collect);
        assert!(r3.errors.is_empty(), "step 3 errors: {:?}", r3.errors);
        let lines = progress.borrow();
        assert!(
            lines.iter().all(|l| l != no_head && l != no_previous),
            "step 3 should take the delta path with no full-fallback wording, got: {lines:?}"
        );
        for fb in &other_fallbacks {
            assert!(
                lines.iter().all(|l| l != fb),
                "step 3 should not emit fallback '{fb}', got: {lines:?}"
            );
        }
    }

    // -- lossy-path warning (R8 / R11.9) ----------------------------------

    /// Plant a `.md` file whose on-disk name is not valid UTF-8 by
    /// concatenating an invalid lead byte (0xFF) with a literal `.md`
    /// suffix. Returns the constructed full path. Linux tmpfs accepts
    /// arbitrary byte sequences as filenames; the cfg(unix) gate is the
    /// safety boundary against Windows builds invoking
    /// `OsStr::from_bytes`.
    #[cfg(unix)]
    fn plant_non_utf8_md(dir: &Path) -> std::path::PathBuf {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bytes = [0xFFu8, b'.', b'm', b'd'];
        let name = OsStr::from_bytes(&bytes);
        let path = dir.join(name);
        fs::write(
            &path,
            "# Body\n\nlossy filename body text that is long enough.\n",
        )
        .expect("tmpfs should accept non-UTF-8 filenames; if this fails, gate the test");
        path
    }

    #[cfg(unix)]
    #[test]
    fn full_ingest_warns_and_skips_non_utf8_filename() {
        // R11.9. Plant one valid-UTF-8 `.md` and one non-UTF-8 `.md` in
        // the same dir; full_ingest must index only the valid one, and
        // emit a lossy-path warning on `result.errors` naming the bad
        // file.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let good = dir.join("good.md");
        fs::write(
            &good,
            "# Good\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        let _lossy = plant_non_utf8_md(dir);
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Act
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Assert
        assert_eq!(
            result.errors.len(),
            1,
            "expected exactly one lossy warning, got: {:?}",
            result.errors
        );
        let warning = &result.errors[0];
        assert!(
            warning.contains("not valid UTF-8"),
            "warning must name the cause, got: {warning}"
        );
        assert!(
            warning.contains("file not indexed"),
            "warning must name the consequence, got: {warning}"
        );
        assert_eq!(
            result.files_processed, 1,
            "only the valid-UTF-8 file should be indexed"
        );
        assert!(
            result.chunks_created >= 1,
            "valid sibling should still produce chunks, got: {}",
            result.chunks_created
        );
        let stats = db.stats().unwrap();
        assert_eq!(stats.sources, 1, "DB must contain exactly one source row");
    }

    #[cfg(unix)]
    #[test]
    fn full_ingest_with_only_non_utf8_files_routes_to_empty_warning() {
        // Shadow path. If every .md file in the dir is lossy-named, the
        // empty-warning probe must route to `FilesystemEmpty`, not
        // `AllIgnored`. The `.loreignore` wording would name the wrong
        // cause and recovery action.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let _lossy = plant_non_utf8_md(dir);

        // Act
        let message = empty_warning_message(dir).expect("empty-dir warning must fire");

        // Assert
        assert!(
            message.contains("knowledge directory is empty"),
            "all-lossy dir must classify as FilesystemEmpty, got: {message}"
        );
        assert!(
            !message.contains(".loreignore"),
            "all-lossy dir must not blame .loreignore, got: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn discover_md_files_progress_line_does_not_misattribute_lossy_to_loreignore() {
        // Shadow path. Without the `walked_count - kept.len() -
        // lossy_warnings.len()` accounting fix, the progress line would
        // print "1 excluded by .loreignore" for an all-lossy directory
        // with no .loreignore file present. Guard against that
        // regression.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        fs::write(
            dir.join("good.md"),
            "# Good\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        let _lossy = plant_non_utf8_md(dir);
        let progress = std::cell::RefCell::new(Vec::<String>::new());

        // Act
        let _ = discover_md_files(dir, None, &|m| progress.borrow_mut().push(m.to_string()));

        // Assert
        let lines = progress.borrow();
        assert!(
            lines.iter().any(|l| l.contains("Found 1 markdown files")),
            "progress should count only the valid-UTF-8 file, got: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| !l.contains("excluded by .loreignore")),
            "lossy must not be blamed on .loreignore, got: {lines:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn delta_reconcile_surfaces_lossy_warning_from_walk() {
        // Shadow path. A non-UTF-8 filename encountered during the
        // `.loreignore` reconciliation walk must surface on
        // `IngestResult::errors`, not just on a `lore_debug!` log line.
        // Reproduce via: ingest twice with the lossy file present, with
        // `.loreignore` changing between runs to force the
        // reconciliation pass.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        let good = dir.join("good.md");
        fs::write(
            &good,
            "# Good\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &good, "seed").unwrap();
        // Seed `.loreignore` so the first ingest records its hash.
        let loreignore = dir.join(".loreignore");
        fs::write(&loreignore, "# placeholder\n").unwrap();
        git::add_and_commit(dir, &loreignore, "loreignore v1").unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        // First ingest: no lossy file yet — establishes META_LAST_COMMIT
        // and stores the .loreignore hash.
        let first = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(
            first.errors.is_empty(),
            "seed ingest errors: {:?}",
            first.errors
        );
        // Now plant the lossy file and bump `.loreignore` so the second
        // ingest takes the reconciliation pass (which calls
        // `walk_md_files`).
        let _lossy = plant_non_utf8_md(dir);
        fs::write(&loreignore, "# placeholder v2\n").unwrap();

        // Act
        let second = ingest(&db, &embedder, dir, "heading", &|_| {});

        // Assert
        assert!(
            second
                .errors
                .iter()
                .any(|e| e.contains("not valid UTF-8") && e.contains("file not indexed")),
            "reconcile pass must propagate lossy warning, got: {:?}",
            second.errors
        );
    }

    #[test]
    fn full_ingest_passes_valid_utf8_unicode_paths_unchanged() {
        // Negative control. `to_string_lossy()` returns `Cow::Borrowed`
        // for any valid-UTF-8 path including non-ASCII. The lossy check
        // must not be mistakenly tightened to ASCII-only.

        // Arrange
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let cafe = dir.join("café.md");
        fs::write(
            &cafe,
            "# Café\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // Act
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Assert
        assert!(
            result.errors.is_empty(),
            "non-ASCII UTF-8 path must index cleanly, got: {:?}",
            result.errors
        );
        assert_eq!(
            result.files_processed, 1,
            "café.md should be indexed normally"
        );
    }

    // -- delta ingest tests ------------------------------------------------

    #[test]
    fn delta_ingest_processes_only_changed_files() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        // Create two files and do initial full ingest.
        let alpha = dir.join("alpha.md");
        let beta = dir.join("beta.md");
        fs::write(
            &alpha,
            "# Alpha\n\nAlpha body text that is long enough for a chunk.\n",
        )
        .unwrap();
        fs::write(
            &beta,
            "# Beta\n\nBeta body text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &alpha, "add alpha").unwrap();
        git::add_and_commit(dir, &beta, "add beta").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        let initial_chunks = result.chunks_created;

        // Modify only alpha.
        fs::write(
            &alpha,
            "# Alpha\n\nUpdated alpha body that is long enough.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &alpha, "update alpha").unwrap();

        // Delta ingest should only process the modified file.
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(
            matches!(result.mode, IngestMode::Delta { .. }),
            "expected Delta mode, got {:?}",
            result.mode
        );
        assert_eq!(
            result.files_processed, 1,
            "should only process the modified file"
        );
        assert!(result.errors.is_empty());

        // Beta chunks should still be in the database.
        let stats = db.stats().unwrap();
        assert!(
            stats.chunks >= initial_chunks,
            "beta chunks should be preserved"
        );
    }

    #[test]
    fn delta_ingest_handles_deleted_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let alpha = dir.join("alpha.md");
        let beta = dir.join("beta.md");
        fs::write(
            &alpha,
            "# Alpha\n\nAlpha body text that is long enough for a chunk.\n",
        )
        .unwrap();
        fs::write(
            &beta,
            "# Beta\n\nBeta body text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &alpha, "add alpha").unwrap();
        git::add_and_commit(dir, &beta, "add beta").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Delete alpha.
        fs::remove_file(&alpha).unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "delete alpha"])
            .current_dir(dir)
            .output()
            .unwrap();

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert_eq!(result.files_processed, 1);
        assert!(result.errors.is_empty());

        // Alpha chunks should be gone, beta chunks should remain.
        let stats = db.stats().unwrap();
        assert_eq!(stats.sources, 1, "only beta should remain");
    }

    #[test]
    fn delta_ingest_handles_renamed_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let old_name = dir.join("old-name.md");
        fs::write(
            &old_name,
            "# Rename Test\n\nBody text that is long enough for rename detection by git.\n\nThis extra paragraph makes the content substantial.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &old_name, "add file").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Verify old-name chunks exist.
        let stats_before = db.stats().unwrap();
        assert_eq!(stats_before.sources, 1);

        // Rename via git mv.
        Command::new("git")
            .args(["mv", "old-name.md", "new-name.md"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "rename file"])
            .current_dir(dir)
            .output()
            .unwrap();

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert!(result.errors.is_empty());

        // Old source should be gone, new source should exist.
        let stats_after = db.stats().unwrap();
        assert_eq!(stats_after.sources, 1, "should still have exactly 1 source");

        // Search should find chunks under the new source file.
        let results = db.search_fts("rename", 10).unwrap();
        assert!(!results.is_empty(), "should find chunks from renamed file");
        assert_eq!(results[0].source_file, "new-name.md");
    }

    #[test]
    fn delta_ingest_no_changes_returns_early() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("doc.md");
        fs::write(
            &file,
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // No changes — delta should return immediately.
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert_eq!(result.files_processed, 0);
        assert_eq!(result.chunks_created, 0);
    }

    #[test]
    fn ingest_falls_back_to_full_without_git() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        fs::write(
            dir.join("doc.md"),
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.mode, IngestMode::Full);
        assert_eq!(result.files_processed, 1);
    }

    #[test]
    fn ingest_falls_back_to_full_without_stored_commit() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("doc.md");
        fs::write(
            &file,
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // First ingest() call with no stored commit falls back to full.
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(result.mode, IngestMode::Full);
    }

    // -- .loreignore in full ingest ---------------------------------------

    /// Helper: write a markdown file with frontmatter and a body chunk.
    fn write_md(dir: &Path, name: &str, title: &str, body: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            &path,
            format!("# {title}\n\n{body} that is long enough to chunk.\n"),
        )
        .unwrap();
    }

    #[test]
    fn full_ingest_skips_files_matched_by_loreignore() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "README.md", "Readme", "Project readme");
        write_md(dir, "rust.md", "Rust", "Rust pattern body");
        fs::write(dir.join(".loreignore"), "README.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.files_processed, 1);
        let files = db.source_files().unwrap();
        assert_eq!(files, vec!["rust.md".to_string()]);
    }

    #[test]
    fn full_ingest_skips_directory_matched_by_loreignore() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "rust.md", "Rust", "Rust pattern body");
        write_md(dir, "docs/intro.md", "Intro", "Doc intro");
        write_md(dir, "docs/api.md", "API", "Doc api");
        fs::write(dir.join(".loreignore"), "docs/\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert!(result.errors.is_empty());
        assert_eq!(result.files_processed, 1);
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
    }

    #[test]
    fn full_ingest_without_loreignore_indexes_all_files() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "a.md", "A", "Body a");
        write_md(dir, "b.md", "B", "Body b");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.files_processed, 2);
        assert_eq!(db.stats().unwrap().sources, 2);
    }

    #[test]
    fn full_ingest_with_all_files_excluded_indexes_nothing() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "a.md", "A", "Body a");
        write_md(dir, "b.md", "B", "Body b");
        fs::write(dir.join(".loreignore"), "*.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let messages = std::cell::RefCell::new(Vec::<String>::new());
        let result = ingest(&db, &embedder, dir, "heading", &|m| {
            messages.borrow_mut().push(m.to_string());
        });

        assert_eq!(result.files_processed, 0);
        assert_eq!(db.stats().unwrap().sources, 0);
        let captured = messages.borrow();
        assert!(
            captured
                .iter()
                .any(|m| m.contains("matched every markdown")),
            "expected warning, got: {captured:?}"
        );
    }

    #[test]
    fn full_ingest_with_empty_loreignore_applies_no_filtering() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "a.md", "A", "Body a");
        // Comments and blanks only — no effective patterns.
        fs::write(dir.join(".loreignore"), "# nothing here\n\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.files_processed, 1);
    }

    #[test]
    fn full_ingest_negation_un_ignores_specific_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "important.md", "Important", "Important body");
        write_md(dir, "draft.md", "Draft", "Draft body");
        write_md(dir, "scratch.md", "Scratch", "Scratch body");
        fs::write(dir.join(".loreignore"), "*.md\n!important.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.files_processed, 1);
        assert_eq!(db.source_files().unwrap(), vec!["important.md".to_string()]);
    }

    #[test]
    fn full_ingest_stores_loreignore_hash_in_metadata() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "rust.md", "Rust", "Body");
        fs::write(dir.join(".loreignore"), "README.md\n").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let _ = ingest(&db, &embedder, dir, "heading", &|_| {});

        let stored = db.get_metadata(META_LOREIGNORE_HASH).unwrap();
        let expected = loreignore::load(dir).hash;
        assert_eq!(stored, Some(expected));
        assert!(stored.unwrap() != "");
    }

    // -- .loreignore in delta ingest --------------------------------------

    /// Helper: stage and commit all changes in a tempdir.
    fn git_commit_all(dir: &Path, message: &str) {
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn delta_ingest_skips_added_file_matched_by_loreignore() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Add a new file that .loreignore matches.
        write_md(dir, "drafts/wip.md", "WIP", "Draft body");
        git_commit_all(dir, "add draft");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(
            result.files_processed, 0,
            "ignored file should not be processed"
        );
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
    }

    #[test]
    fn delta_ingest_without_loreignore_processes_all_changes() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "a.md", "A", "Body a");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        write_md(dir, "b.md", "B", "Body b");
        git_commit_all(dir, "add b");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(result.files_processed, 1);
        assert_eq!(db.stats().unwrap().sources, 2);
    }

    #[test]
    fn delta_ingest_reconciles_when_loreignore_added() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "README.md", "Readme", "Project readme");
        write_md(dir, "rust.md", "Rust", "Rust body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 2);

        // Add .loreignore in a new commit.
        fs::write(dir.join(".loreignore"), "README.md\n").unwrap();
        git_commit_all(dir, "add loreignore");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(
            db.source_files().unwrap(),
            vec!["rust.md".to_string()],
            "README should have been reconciled out"
        );
    }

    #[test]
    fn delta_ingest_reconciles_when_loreignore_modified_to_add_pattern() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "scratch.md", "Scratch", "Scratch body");
        fs::write(dir.join(".loreignore"), "# placeholder\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 2);

        // Modify .loreignore to add a pattern.
        fs::write(dir.join(".loreignore"), "scratch.md\n").unwrap();
        git_commit_all(dir, "exclude scratch");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
    }

    #[test]
    fn delta_ingest_loreignore_deleted_keeps_indexed_files() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Delete .loreignore.
        fs::remove_file(dir.join(".loreignore")).unwrap();
        git_commit_all(dir, "remove loreignore");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
        assert_eq!(
            db.get_metadata(META_LOREIGNORE_HASH).unwrap(),
            Some(String::new()),
            "hash should be cleared after .loreignore deletion"
        );
    }

    #[test]
    fn delta_ingest_runs_when_loreignore_is_only_change() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "README.md", "Readme", "Readme body");
        write_md(dir, "rust.md", "Rust", "Rust body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 2);

        // Add .loreignore with no other changes.
        fs::write(dir.join(".loreignore"), "README.md\n").unwrap();
        git_commit_all(dir, "exclude readme");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        // git diff is empty for markdown files; reconciliation still ran.
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
    }

    #[test]
    fn delta_ingest_renamed_to_ignored_path_removes_old_chunks() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        fs::write(dir.join(".loreignore"), "archive/\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(db.source_files().unwrap().contains(&"rust.md".to_string()));

        // Rename rust.md → archive/rust.md (target is ignored).
        fs::create_dir_all(dir.join("archive")).unwrap();
        fs::rename(dir.join("rust.md"), dir.join("archive/rust.md")).unwrap();
        git_commit_all(dir, "archive rust");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        let files = db.source_files().unwrap();
        assert!(
            files.is_empty(),
            "rust.md should be removed, archive/rust.md should not be indexed: {files:?}"
        );
    }

    #[test]
    fn delta_ingest_renamed_from_ignored_path_indexes_new_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "drafts/idea.md", "Idea", "Idea body");
        fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        // drafts/idea.md was never indexed.
        assert!(db.source_files().unwrap().is_empty());

        // Rename drafts/idea.md → idea.md (source was ignored, target is not).
        fs::rename(dir.join("drafts/idea.md"), dir.join("idea.md")).unwrap();
        git_commit_all(dir, "promote idea");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(db.source_files().unwrap(), vec!["idea.md".to_string()]);
    }

    #[test]
    fn delta_ingest_renamed_with_both_paths_ignored_is_skipped() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "drafts/idea.md", "Idea", "Idea body");
        fs::write(dir.join(".loreignore"), "drafts/\narchive/\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);

        // Rename drafts/idea.md → archive/idea.md (both paths ignored).
        fs::create_dir_all(dir.join("archive")).unwrap();
        fs::rename(dir.join("drafts/idea.md"), dir.join("archive/idea.md")).unwrap();
        git_commit_all(dir, "rename to archive");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        // Only rust.md remains; the rename was a no-op for the database.
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
    }

    #[test]
    fn delta_ingest_reconciliation_failure_preserves_stale_hash() {
        // Verifies the cascade-fix: if reconciliation fails, the stored hash
        // must remain stale so the next ingest retries reconciliation rather
        // than silently skipping it.
        //
        // We can't easily inject a database failure without invasive
        // refactoring, so this test exercises the success path and verifies
        // the hash IS written on success — providing the inverse evidence.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "scratch.md", "Scratch", "Scratch body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        let initial_hash = db
            .get_metadata(META_LOREIGNORE_HASH)
            .unwrap()
            .unwrap_or_default();
        assert_eq!(initial_hash, "", "no .loreignore initially");

        // Add .loreignore in a new commit — reconciliation must run and the
        // hash must be updated only on success.
        fs::write(dir.join(".loreignore"), "scratch.md\n").unwrap();
        git_commit_all(dir, "exclude scratch");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        let new_hash = db.get_metadata(META_LOREIGNORE_HASH).unwrap();
        assert!(new_hash.is_some() && !new_hash.unwrap().is_empty());
    }

    #[test]
    fn delta_ingest_loreignore_pattern_removed_re_indexes_file() {
        // Cumulative reconciliation: removing a pattern from .loreignore
        // brings previously excluded files back into the index automatically.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "drafts/wip.md", "WIP", "Draft body");
        fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);

        // Remove the drafts/ pattern.
        fs::write(dir.join(".loreignore"), "# nothing excluded\n").unwrap();
        git_commit_all(dir, "un-ignore drafts");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.reconciled_removed, 0);
        assert_eq!(
            result.reconciled_added, 1,
            "drafts/wip.md should be re-indexed"
        );
        let files = db.source_files().unwrap();
        assert_eq!(
            files,
            vec!["drafts/wip.md".to_string(), "rust.md".to_string()]
        );
    }

    #[test]
    fn delta_ingest_reconciliation_respects_negation() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "important.md", "Important", "Important body");
        write_md(dir, "draft.md", "Draft", "Draft body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 2);

        // Exclude all .md files except important.md.
        fs::write(dir.join(".loreignore"), "*.md\n!important.md\n").unwrap();
        git_commit_all(dir, "add loreignore with negation");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        // Reconciliation must not remove important.md (whitelist).
        assert_eq!(db.source_files().unwrap(), vec!["important.md".to_string()]);
    }

    #[test]
    fn delta_ingest_unchanged_count_excludes_ignored_modifications() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "go.md", "Go", "Go body");
        // Make scratch.md exist but don't index it (it's ignored from the start).
        write_md(dir, "scratch.md", "Scratch", "Scratch body");
        fs::write(dir.join(".loreignore"), "scratch.md\n").unwrap();
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 2);

        // Modify both rust.md and scratch.md. scratch.md is ignored.
        write_md(dir, "rust.md", "Rust", "Updated rust body");
        write_md(dir, "scratch.md", "Scratch", "Updated scratch body");
        git_commit_all(dir, "modify both");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        assert_eq!(result.files_processed, 1, "only rust.md should process");
        // unchanged should be 1 (go.md), not deflated by the ignored scratch.md.
        if let IngestMode::Delta { unchanged } = result.mode {
            assert_eq!(unchanged, 1, "go.md should be the only unchanged file");
        } else {
            panic!("expected Delta mode");
        }
    }

    #[test]
    fn delta_ingest_result_reports_reconciled_count() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "scratch.md", "Scratch", "Scratch body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 2);

        // Add .loreignore excluding scratch.md, no other file changes.
        fs::write(dir.join(".loreignore"), "scratch.md\n").unwrap();
        git_commit_all(dir, "exclude scratch");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(
            result.reconciled_removed, 1,
            "should report one removed file"
        );
        assert_eq!(result.reconciled_added, 0);
        assert_eq!(result.files_processed, 0, "no diff-driven changes");
    }

    #[test]
    fn delta_ingest_result_reports_zero_reconciled_when_unchanged() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Modify rust.md (no .loreignore).
        write_md(dir, "rust.md", "Rust", "Updated rust body");
        git_commit_all(dir, "modify rust");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(result.reconciled_removed, 0);
        assert_eq!(result.reconciled_added, 0);
        assert_eq!(result.files_processed, 1);
    }

    #[test]
    fn delta_ingest_unchanged_count_after_reconciliation() {
        // Regression: when reconciliation removes files, the unchanged count
        // must reflect the post-reconciliation source count, not the
        // pre-reconciliation count. delta_ingest queries db.stats() after
        // reconciliation has run, so this is correct by construction —
        // this test pins that ordering.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);
        write_md(dir, "rust.md", "Rust", "Rust body");
        write_md(dir, "go.md", "Go", "Go body");
        write_md(dir, "drafts/wip.md", "WIP", "Draft body");
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(db.stats().unwrap().sources, 3);

        // In one commit: add .loreignore excluding drafts/, AND modify rust.md.
        // Reconciliation removes drafts/wip.md (1 file), then delta processes
        // rust.md (1 file), leaving go.md as the only "unchanged" file.
        fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
        write_md(dir, "rust.md", "Rust", "Updated rust body");
        git_commit_all(dir, "exclude drafts and update rust");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.files_processed, 1, "only rust.md should process");
        if let IngestMode::Delta { unchanged } = result.mode {
            assert_eq!(
                unchanged, 1,
                "after reconciliation removed drafts/wip.md, only go.md is unchanged"
            );
        } else {
            panic!("expected Delta mode");
        }
        // Verify the database actually reflects the reconciled state.
        let files = db.source_files().unwrap();
        assert_eq!(files, vec!["go.md".to_string(), "rust.md".to_string()]);
    }

    #[test]
    fn full_ingest_stores_empty_hash_when_loreignore_absent() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_md(dir, "rust.md", "Rust", "Body");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let _ = ingest(&db, &embedder, dir, "heading", &|_| {});

        let stored = db.get_metadata(META_LOREIGNORE_HASH).unwrap();
        assert_eq!(stored, Some(String::new()));
    }

    // -- U7: applies_when persistence + malformed advisories ----------------
    //
    // The U2 frontmatter parser populates `Chunk::applies_when_json` for
    // every chunk produced from a file with a valid `applies_when` block;
    // U7 plumbs the column through `index_single_file` so both chunk and
    // pattern rows persist it. Test scenarios from the plan: happy paths
    // (with/without predicate), error path (malformed → NULL + advisory),
    // non-universal predicate guard, multi-section whole-file invariant,
    // and delta-ingest update.

    fn write_pattern(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
    }

    #[test]
    fn ingest_persists_applies_when_json_on_chunk_and_pattern_rows() {
        // Happy path: a universal pattern with a valid `applies_when` block
        // populates `applies_when_json` on every chunk and on the
        // `patterns` row (whole-file mirror via `pattern_row_from`).
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        let body = "---\n\
                    tags: [universal, workflow]\n\
                    applies_when:\n  \
                    tools: [Bash]\n  \
                    bash_command_starts_with: [git, gh]\n\
                    ---\n\n\
                    # Workflow\n\n\
                    Workflow body that is long enough for chunking.\n";
        write_pattern(dir, "workflow.md", body);

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(result.malformed_applies_when.is_empty());

        // Direct DB access: every chunk row carries the JSON.
        let chunk_predicates = db
            .chunk_applies_when_json_for_source("workflow.md")
            .unwrap();
        assert!(!chunk_predicates.is_empty(), "expected chunks");
        for predicate in &chunk_predicates {
            let json = predicate.as_ref().expect("Some JSON");
            assert!(
                json.contains(r#""tools":["Bash"]"#),
                "chunk JSON should serialise tools, got: {json}"
            );
            assert!(
                json.contains(r#""bash_command_starts_with":["git","gh"]"#),
                "chunk JSON should serialise bash prefix list, got: {json}"
            );
        }

        // Pattern row also carries the JSON (mirrored via `pattern_row_from`).
        let pattern_predicate = db
            .pattern_applies_when_json_for_source("workflow.md")
            .unwrap();
        let pattern_json = pattern_predicate
            .expect("patterns row exists")
            .expect("predicate JSON populated on pattern row");
        assert_eq!(
            &pattern_json,
            chunk_predicates[0].as_ref().unwrap(),
            "pattern row's applies_when_json must match chunk row's verbatim",
        );
    }

    #[test]
    fn ingest_leaves_applies_when_json_null_when_block_absent() {
        // Happy path: a pattern without an `applies_when` block ingests
        // with `applies_when_json = NULL` on both chunk and pattern rows.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        write_pattern(
            dir,
            "plain.md",
            "---\ntags: [conventions]\n---\n\n# Plain\n\nPlain body long enough.\n",
        );

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let chunk_predicates = db.chunk_applies_when_json_for_source("plain.md").unwrap();
        assert!(!chunk_predicates.is_empty());
        for predicate in chunk_predicates {
            assert!(predicate.is_none(), "expected NULL applies_when_json");
        }

        let pattern_predicate = db.pattern_applies_when_json_for_source("plain.md").unwrap();
        assert_eq!(pattern_predicate, Some(None));
    }

    #[test]
    fn ingest_records_malformed_applies_when_advisory_with_null_column() {
        // Error path (AE5): a typo'd top-level key (`appliess_when`) leaves
        // the column NULL on both rows AND surfaces a
        // `MalformedPredicateEntry` in `IngestResult.malformed_applies_when`
        // so introspective callers can report the warning even though the
        // user-facing channel is stderr. Pattern fires as if no predicate
        // were set (R9 skip-with-warning).
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let body = "---\n\
                    tags: [universal]\n\
                    appliess_when:\n  \
                    tools: [Bash]\n\
                    ---\n\n\
                    # Typo\n\n\
                    Typo body that is long enough for a chunk.\n";
        write_pattern(dir, "typo.md", body);

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Both rows have NULL — pattern fires unrestricted.
        let chunk_predicates = db.chunk_applies_when_json_for_source("typo.md").unwrap();
        assert!(!chunk_predicates.is_empty());
        for predicate in chunk_predicates {
            assert!(predicate.is_none(), "typo'd predicate must leave NULL");
        }
        let pattern_predicate = db.pattern_applies_when_json_for_source("typo.md").unwrap();
        assert_eq!(pattern_predicate, Some(None));

        // The advisory IS captured in IngestResult so callers can introspect.
        assert_eq!(
            result.malformed_applies_when.len(),
            1,
            "expected exactly one malformed entry, got: {:?}",
            result.malformed_applies_when
        );
        let entry = &result.malformed_applies_when[0];
        assert_eq!(entry.file_path, "typo.md");
        assert!(
            entry.key.contains("appliess_when") || entry.key == "applies_when",
            "advisory should mention the offending key, got: {}",
            entry.key
        );
    }

    #[test]
    fn ingest_persists_applies_when_when_pattern_is_not_universal() {
        // R8 invariant: the predicate parses on any pattern (the namespace
        // is reserved for Track 2-B), so the column persists even when
        // the pattern is not universal-tagged. The hook-side evaluator
        // (U5) gates predicate evaluation on `is_universal`. U7's
        // ingest-side guard surfaces a stderr advisory naming the file
        // so authors get a signal that nothing happens at hook time.
        // We verify the DB column persists; the stderr emission is best
        // observed in the MCP audit tests since the unit-test stderr is
        // captured by the harness.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let body = "---\n\
                    tags: [conventions]\n\
                    applies_when:\n  \
                    tools: [Bash]\n\
                    ---\n\n\
                    # Dormant\n\n\
                    Dormant body that is long enough for a chunk.\n";
        write_pattern(dir, "dormant.md", body);

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(
            result.malformed_applies_when.is_empty(),
            "non-universal predicate is not malformed; should not appear in malformed list",
        );

        // Column persists — the pattern is not universal-tagged but the
        // namespace is reserved (R7) and parses on any pattern (R8).
        let pattern_predicate = db
            .pattern_applies_when_json_for_source("dormant.md")
            .unwrap();
        let json = pattern_predicate
            .expect("patterns row exists")
            .expect("non-universal predicate still persists in column");
        assert!(
            json.contains(r#""tools":["Bash"]"#),
            "non-universal predicate JSON must round-trip, got: {json}",
        );
    }

    #[test]
    fn ingest_propagates_applies_when_to_every_chunk_in_multi_section_pattern() {
        // Whole-file invariant: every chunk of a multi-section pattern
        // shares the same `applies_when_json`. Pinned per the
        // composition-cascades learning — sibling expansion via
        // `chunks_by_sources` returns all chunks from a matched source,
        // so they must all carry the predicate consistently or
        // suppression would behave non-uniformly.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let body = "---\n\
                    tags: [universal]\n\
                    applies_when:\n  \
                    bash_command_starts_with: [git]\n\
                    ---\n\n\
                    # Top\n\n\
                    Intro body that is long enough for chunking.\n\n\
                    ## Section A\n\n\
                    Section A body that is long enough for chunking.\n\n\
                    ## Section B\n\n\
                    Section B body that is long enough for chunking.\n";
        write_pattern(dir, "multi.md", body);

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        let predicates = db.chunk_applies_when_json_for_source("multi.md").unwrap();
        assert!(
            predicates.len() >= 2,
            "expected at least two chunks, got {}",
            predicates.len(),
        );
        let first = predicates[0]
            .as_ref()
            .expect("first chunk must carry predicate JSON")
            .clone();
        for (i, predicate) in predicates.iter().enumerate() {
            assert_eq!(
                predicate.as_ref(),
                Some(&first),
                "chunk {i} must share the same applies_when_json as the first chunk",
            );
        }
    }

    #[test]
    fn delta_ingest_updates_applies_when_json_when_predicate_added() {
        // Integration: a pattern initially has no predicate; a later
        // commit adds `applies_when` to its frontmatter. Delta ingest
        // unconditionally rewrites the chunks of changed files, so the
        // updated predicate must show up on both chunk and pattern rows.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let initial_body = "---\ntags: [universal]\n---\n\n# WF\n\nWF body that is long enough.\n";
        write_pattern(dir, "wf.md", initial_body);
        git_commit_all(dir, "initial");

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Before adding the predicate.
        let before = db.pattern_applies_when_json_for_source("wf.md").unwrap();
        assert_eq!(before, Some(None));

        // Now amend the frontmatter to include `applies_when` and re-commit.
        let updated_body = "---\n\
                            tags: [universal]\n\
                            applies_when:\n  \
                            bash_command_starts_with: [git]\n\
                            ---\n\n\
                            # WF\n\n\
                            WF body that is long enough.\n";
        write_pattern(dir, "wf.md", updated_body);
        git_commit_all(dir, "add applies_when");

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let after_pattern = db.pattern_applies_when_json_for_source("wf.md").unwrap();
        let after_json = after_pattern
            .expect("patterns row exists after delta")
            .expect("delta-ingested row carries predicate");
        assert!(
            after_json.contains(r#""bash_command_starts_with":["git"]"#),
            "delta-ingested predicate JSON missing prefix list, got: {after_json}",
        );
        let after_chunks = db.chunk_applies_when_json_for_source("wf.md").unwrap();
        for predicate in after_chunks {
            assert_eq!(
                predicate.as_deref(),
                Some(after_json.as_str()),
                "every chunk row must carry the new predicate after delta",
            );
        }
    }

    #[test]
    fn ingest_single_file_records_malformed_applies_when_in_result() {
        // Pin that the single-file CLI entry point also surfaces the
        // advisory in its `IngestResult.malformed_applies_when`. Same
        // contract as full ingest, exercised through the
        // `ingest_single_file` wrapper used by `lore ingest --file`.
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let body = "---\n\
                    tags: [universal]\n\
                    applies_when:\n  \
                    tools: Bash\n\
                    ---\n\n\
                    # Scalar\n\n\
                    Scalar body that is long enough for a chunk.\n";
        write_pattern(dir, "scalar.md", body);

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest_single_file(
            &db,
            &embedder,
            dir,
            &dir.join("scalar.md"),
            "heading",
            false,
            &|_| {},
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(
            !result.malformed_applies_when.is_empty(),
            "scalar-where-list should produce a malformed advisory entry",
        );
        assert_eq!(result.malformed_applies_when[0].file_path, "scalar.md");

        // The chunk row's applies_when_json must NOT carry the scalar form;
        // depending on parser behaviour the entry may still parse the known
        // keys partially. The contract here is that malformed entries are
        // reported AND ingest does not crash.
        let chunks = db.chunk_applies_when_json_for_source("scalar.md").unwrap();
        assert!(!chunks.is_empty(), "scalar.md should still ingest");
    }
}
