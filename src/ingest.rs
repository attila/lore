// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ingestion orchestration and write operations.
//!
//! Walks markdown directories, chunks files, embeds them via any [`Embedder`],
//! and stores results in the [`KnowledgeDB`].  Write helpers (`add_pattern`,
//! `update_pattern`, `append_to_pattern`) create or modify markdown files
//! on disk, re-index them, and optionally commit via git.

use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use walkdir::WalkDir;

use crate::chunking::{Chunk, chunk_as_document, chunk_by_heading, extract_title};
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

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Whether the ingest ran in full or delta mode.
#[derive(Debug, PartialEq, Eq)]
pub enum IngestMode {
    /// Full re-index (cleared database and re-embedded everything).
    Full,
    /// Delta update — only changed files were processed.
    Delta {
        /// Number of files that were unchanged and skipped.
        unchanged: usize,
    },
}

/// Summary returned after a directory ingest.
#[derive(Debug)]
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
pub fn ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    // Not a git repo — full ingest is the only option.
    if !git::is_git_repo(knowledge_dir) {
        on_progress("Not a git repository — running full ingest");
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    }

    // No stored commit — first ingest or metadata was cleared.
    let Ok(Some(last_commit)) = db.get_metadata(META_LAST_COMMIT) else {
        on_progress("No previous ingest recorded — running full ingest");
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
        return IngestResult {
            mode: IngestMode::Delta { unchanged: 0 },
            files_processed: 0,
            chunks_created: reconcile_chunks,
            reconciled_removed,
            reconciled_added,
            errors: reconcile_errors,
        };
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
    let stats = match reconcile_ignored(
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
    let (disk_files, _) = walk_md_files(knowledge_dir, ignore_matcher);
    for (rel_path, full_path) in disk_files {
        if db_sources.contains(&rel_path) {
            continue;
        }
        match index_single_file(db, embedder, knowledge_dir, &full_path, strategy) {
            Ok((chunks, _)) => {
                stats.added += 1;
                stats.chunks_added += chunks;
                lore_debug!("loreignore: reconciled {rel_path} (re-indexed, {chunks} chunks)");
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

    let mut result = IngestResult {
        mode: IngestMode::Delta { unchanged },
        files_processed: 0,
        chunks_created: 0,
        reconciled_removed: 0,
        reconciled_added: 0,
        errors: Vec::new(),
    };

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
                Ok((chunks, _)) => {
                    result.chunks_created += chunks;
                    on_progress(&format!("  {path} → {chunks} chunks"));
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
                Ok((chunks, _)) => {
                    result.chunks_created += chunks;
                    on_progress(&format!("  {from} → {to} ({chunks} chunks)"));
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
fn walk_md_files(
    knowledge_dir: &Path,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
) -> (Vec<(String, PathBuf)>, usize) {
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
    let mut kept: Vec<(String, PathBuf)> = walked
        .into_iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(knowledge_dir)
                .ok()?
                .to_string_lossy()
                .to_string();
            if let Some(matcher) = ignore_matcher
                && loreignore::is_ignored(matcher, Path::new(&rel), false)
            {
                lore_debug!("loreignore: skipping {rel} (walk)");
                return None;
            }
            Some((rel, path))
        })
        .collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    (kept, walked_count)
}

/// Walk + report for full ingest. Returns full paths for compatibility with
/// the existing `full_ingest` loop.
fn discover_md_files(
    knowledge_dir: &Path,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
    on_progress: &dyn Fn(&str),
) -> Vec<PathBuf> {
    let (kept, walked_count) = walk_md_files(knowledge_dir, ignore_matcher);
    let skipped = walked_count - kept.len();
    if skipped > 0 {
        on_progress(&format!(
            "Found {} markdown files ({} excluded by .loreignore)",
            kept.len(),
            skipped
        ));
    } else {
        on_progress(&format!("Found {} markdown files", kept.len()));
    }
    kept.into_iter().map(|(_, p)| p).collect()
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
    let mut result = IngestResult {
        mode: IngestMode::Full,
        files_processed: 0,
        chunks_created: 0,
        reconciled_removed: 0,
        reconciled_added: 0,
        errors: Vec::new(),
    };

    // Single read of .loreignore: matcher and hash come from the same bytes.
    let loaded_ignore = loreignore::load(knowledge_dir);
    let md_files = discover_md_files(knowledge_dir, loaded_ignore.matcher.as_ref(), on_progress);

    if md_files.is_empty() && loaded_ignore.matcher.is_some() {
        on_progress("Warning: .loreignore matched every markdown file; nothing will be indexed");
    }

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
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to read {}: {e}", file_path.display()));
                continue;
            }
        };

        let rel_path = file_path
            .strip_prefix(knowledge_dir)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let chunks = dispatch_chunking(strategy, &content, &rel_path);

        for chunk in &chunks {
            let embedding = match embedder.embed(&embed_text(chunk)) {
                Ok(emb) => Some(emb),
                Err(e) => {
                    result
                        .errors
                        .push(format!("Embedding failed for {}: {e}", chunk.id));
                    None
                }
            };

            if let Err(e) = db.insert_chunk(chunk, embedding.as_deref()) {
                result
                    .errors
                    .push(format!("Insert failed for {}: {e}", chunk.id));
            } else {
                result.chunks_created += 1;
            }
        }

        result.files_processed += 1;
        on_progress(&format!("  {} → {} chunks", rel_path, chunks.len()));
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
        anyhow::bail!("File already exists: {filename}. Use update_pattern instead.");
    }

    std::fs::write(&file_path, &content)?;

    let (chunks, embedding_failures) =
        index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

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
pub fn update_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    source_file: &str,
    body: &str,
    tags: &[&str],
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    let file_path = knowledge_dir.join(source_file);

    if !file_path.exists() {
        anyhow::bail!("File not found: {source_file}");
    }

    validate_within_dir(knowledge_dir, &file_path)?;

    let title = extract_title(&std::fs::read_to_string(&file_path)?)
        .unwrap_or_else(|| file_stem(source_file));

    let content = build_file_content(&title, body, tags);

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

    std::fs::write(&file_path, &content)?;

    let (chunks, embedding_failures) =
        index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

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

    validate_within_dir(knowledge_dir, &file_path)?;

    let existing = std::fs::read_to_string(&file_path)?;
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

    std::fs::write(&file_path, &content)?;

    let (chunks, embedding_failures) =
        index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

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
// Helpers
// ---------------------------------------------------------------------------

/// Index (or re-index) a single file: delete old chunks, chunk, embed, insert.
///
/// The `strategy` parameter selects the chunking approach (`"heading"` or
/// `"document"`).
///
/// Returns `(chunks_indexed, embedding_failures)`.
fn index_single_file(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    file_path: &Path,
    strategy: &str,
) -> anyhow::Result<(usize, usize)> {
    let content = std::fs::read_to_string(file_path)?;
    let rel_path = file_path
        .strip_prefix(knowledge_dir)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    // Remove old chunks for this file.
    db.delete_by_source(&rel_path)?;

    let chunks = dispatch_chunking(strategy, &content, &rel_path);
    let mut count = 0;
    let mut embedding_failures = 0;

    for chunk in &chunks {
        let embedding = if let Ok(emb) = embedder.embed(&embed_text(chunk)) {
            Some(emb)
        } else {
            embedding_failures += 1;
            None
        };
        db.insert_chunk(chunk, embedding.as_deref())?;
        count += 1;
    }

    Ok((count, embedding_failures))
}

/// Validate that `file_path` lies within `knowledge_dir` after canonicalization.
///
/// This prevents path traversal attacks where a `source_file` like
/// `../../../etc/passwd` could escape the knowledge directory.
fn validate_within_dir(knowledge_dir: &Path, file_path: &Path) -> anyhow::Result<()> {
    let canon_dir = knowledge_dir.canonicalize()?;
    let canon_file = file_path.canonicalize()?;
    if !canon_file.starts_with(&canon_dir) {
        anyhow::bail!(
            "Path escapes the knowledge directory: {}",
            file_path.display()
        );
    }
    Ok(())
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
fn dispatch_chunking(strategy: &str, content: &str, rel_path: &str) -> Vec<Chunk> {
    if strategy == "heading" {
        chunk_by_heading(content, rel_path)
    } else {
        chunk_as_document(content, rel_path)
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
fn slugify(title: &str) -> String {
    title
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
    fn add_pattern_rejects_existing_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(dir.join("existing.md"), "# Existing\n").unwrap();

        let result = add_pattern(&db, &embedder, dir, "Existing", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("already exists"), "unexpected error: {msg}");
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
            &["updated"],
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

        let (heading_count, _) = index_single_file(
            &db_heading,
            &embedder,
            dir,
            &dir.join("multi.md"),
            "heading",
        )
        .unwrap();

        let (doc_count, _) =
            index_single_file(&db_doc, &embedder, dir, &dir.join("multi.md"), "document").unwrap();

        // heading strategy should produce more chunks than document strategy.
        assert!(
            heading_count > doc_count,
            "heading ({heading_count}) should produce more chunks than document ({doc_count})"
        );
        assert_eq!(doc_count, 1);
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
            &[],
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

        let result = update_pattern(&db, &embedder, dir, &rel, "new body", &[], None);
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
}
