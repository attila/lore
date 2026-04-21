// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hook pipeline for Claude Code lifecycle events.
//!
//! Reads JSON from stdin, dispatches on `hook_event_name`, and handles:
//! - `SessionStart`: creates a dedup file, returns meta-instruction + pattern index
//! - `PreToolUse`: extracts a search query, searches, dedup-filters, formats imperatives
//! - `PostToolUse`: on Bash errors, searches with stderr, returns relevant patterns
//! - `PostCompact`: resets dedup, re-emits `SessionStart` content

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::database::{KnowledgeDB, SearchResult};
use crate::embeddings::Embedder;
use crate::lore_debug;

// ---------------------------------------------------------------------------
// Stop words
// ---------------------------------------------------------------------------

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "that", "this", "then", "when", "will", "has",
    "have", "was", "are", "not", "but", "can", "all", "its", "our", "use", "new", "let", "set",
    "get", "add", "run", "see", "how", "may", "per", "via", "yet", "also", "just", "some", "been",
    "were", "what", "they", "each", "which", "their", "there", "about", "would", "could", "should",
    "these", "those", "other", "than", "them", "your", "does", "here",
];

// ---------------------------------------------------------------------------
// Input / output types
// ---------------------------------------------------------------------------

/// Deserialized from stdin JSON. All fields optional except `hook_event_name`.
#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub hook_event_name: String,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub agent_type: Option<String>,
    pub transcript_path: Option<String>,
    pub tool_response: Option<serde_json::Value>,
}

/// Written to stdout as JSON.
///
/// Two variants:
/// - `HookSpecific` — for events that support `hookSpecificOutput`
///   (`PreToolUse`, `PostToolUse`).
/// - `SystemMessage` — for events where Claude Code only accepts a top-level
///   `systemMessage` field (`SessionStart`, `PostCompact`).
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum HookOutput {
    HookSpecific {
        #[serde(rename = "hookSpecificOutput")]
        hook_specific_output: HookSpecificOutput,
    },
    SystemMessage {
        #[serde(rename = "systemMessage")]
        system_message: String,
    },
}

/// The payload nested inside `HookOutput::HookSpecific`.
#[derive(Debug, Serialize)]
pub struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read stdin and parse as `HookInput`.
pub fn read_input() -> anyhow::Result<HookInput> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let input: HookInput = serde_json::from_str(&buf)?;
    Ok(input)
}

/// Main dispatcher. Returns `Some(HookOutput)` when context should be
/// injected, or `None` when the hook should produce no output.
pub fn handle_hook(
    input: &HookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    lore_debug!(
        "hook event={} session={} tool={}",
        input.hook_event_name,
        input.session_id.as_deref().unwrap_or("none"),
        input.tool_name.as_deref().unwrap_or("none"),
    );

    let result = match input.hook_event_name.as_str() {
        "SessionStart" => handle_session_start(input, db, config),
        "PreToolUse" => handle_pre_tool_use(input, db, embedder, config),
        "PostToolUse" => handle_post_tool_use(input, db, embedder, config),
        "PostCompact" => handle_post_compact(input, db, config),
        _ => {
            lore_debug!("unknown event, producing no output");
            Ok(None)
        }
    };

    match &result {
        Ok(Some(_)) => lore_debug!("hook producing output"),
        Ok(None) => lore_debug!("hook producing no output"),
        Err(e) => lore_debug!("hook error: {e}"),
    }

    result
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

/// Handle `SessionStart`: create dedup file, return meta-instruction + pattern index.
fn handle_session_start(
    input: &HookInput,
    db: &KnowledgeDB,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    let dedup_path = session_dedup_path(input);
    if let Some(ref path) = dedup_path
        && let Err(e) = reset_dedup(path)
    {
        eprintln!("lore hook: failed to create dedup file: {e}");
        lore_debug!("SessionStart dedup reset error: {e}");
    }

    let context = format_session_context(db, &config.knowledge_dir)?;
    Ok(Some(HookOutput::SystemMessage {
        system_message: context,
    }))
}

/// Handle `PreToolUse`: extract query, search, dedup-filter, format imperatives.
fn handle_pre_tool_use(
    input: &HookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    if skip_agent(input) {
        lore_debug!("skipping subagent");
        return Ok(None);
    }

    let Some(query) = extract_query(input) else {
        lore_debug!("no query extracted from tool input");
        return Ok(None);
    };

    lore_debug!("extracted query: {query}");

    let partitioned = search_with_threshold(db, embedder, config, &query)?;

    if partitioned.is_empty() {
        lore_debug!("search returned no results");
        return Ok(None);
    }

    // Expand each slice independently to its sibling chunks. is_universal
    // is a file-level frontmatter tag, so universal seeds expand only to
    // universal siblings and non-universal seeds only to non-universal —
    // no cross-contamination at the chunk level.
    let universal = expand_to_siblings(db, &partitioned.universal);
    let ranked = expand_to_siblings(db, &partitioned.ranked);
    lore_debug!(
        "expand: {} universal + {} ranked after sibling expansion",
        universal.len(),
        ranked.len(),
    );

    // Combine universal first, then ranked, and route the full set through
    // dedup. dedup_filter_and_record bypasses the `seen.contains` check
    // for universal chunks (read-side filter) and still appends every
    // surfaced chunk to the dedup file (write side). The dedup file
    // therefore remains a faithful "what was injected this session" log
    // — defensive consistency per the session-dedup-lifecycle learning.
    let mut combined = universal;
    let universal_count = combined.len();
    combined.extend(ranked);

    let dedup_path = session_dedup_path(input);
    let combined = if let Some(ref path) = dedup_path
        && path.exists()
    {
        let pre_count = combined.len();
        match dedup_filter_and_record(path, &combined) {
            Ok(filtered) => {
                let kept_universal = filtered.iter().filter(|r| r.is_universal).count();
                lore_debug!(
                    "dedup: {} before -> {} after ({} universal kept by read-bypass) ({})",
                    pre_count,
                    filtered.len(),
                    kept_universal,
                    path.display()
                );
                filtered
            }
            Err(e) => {
                eprintln!("lore hook: dedup filter error: {e}");
                lore_debug!("dedup filter error (continuing without dedup): {e}");
                combined
            }
        }
    } else {
        lore_debug!("dedup inactive (no session file)");
        combined
    };

    if combined.is_empty() {
        lore_debug!("nothing to inject after partition + dedup");
        return Ok(None);
    }

    let kept_universal = combined.iter().filter(|r| r.is_universal).count();
    let kept_ranked = combined.len() - kept_universal;
    let sources: HashSet<&str> = combined.iter().map(|r| r.source_file.as_str()).collect();
    lore_debug!(
        "injecting {} chunks ({} universal + {} ranked, {} initially universal) from {} sources",
        combined.len(),
        kept_universal,
        kept_ranked,
        universal_count,
        sources.len()
    );

    let context = format_imperative(&combined);

    Ok(Some(HookOutput::HookSpecific {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PreToolUse".to_string(),
            additional_context: context,
        },
    }))
}

/// Expand a result slice to include all sibling chunks from the matched
/// source files (e.g. if Error Handling matched, also inject Functions and
/// Naming from the same document). Falls back to the original slice when
/// the database query fails.
fn expand_to_siblings(db: &KnowledgeDB, seeds: &[SearchResult]) -> Vec<SearchResult> {
    if seeds.is_empty() {
        return Vec::new();
    }
    let source_files: Vec<&str> = {
        let mut seen = HashSet::new();
        seeds
            .iter()
            .filter_map(|r| {
                if seen.insert(r.source_file.as_str()) {
                    Some(r.source_file.as_str())
                } else {
                    None
                }
            })
            .collect()
    };
    db.chunks_by_sources(&source_files)
        .unwrap_or_else(|_| seeds.to_vec())
}

/// Handle `PostCompact`: truncate dedup, re-emit `SessionStart` content.
fn handle_post_compact(
    input: &HookInput,
    db: &KnowledgeDB,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    let dedup_path = session_dedup_path(input);
    if let Some(ref path) = dedup_path
        && let Err(e) = reset_dedup(path)
    {
        eprintln!("lore hook: failed to truncate dedup file: {e}");
        lore_debug!("PostCompact dedup reset error: {e}");
    }

    let context = format_session_context(db, &config.knowledge_dir)?;
    Ok(Some(HookOutput::SystemMessage {
        system_message: context,
    }))
}

/// Handle `PostToolUse`: on Bash errors, search with stderr and return patterns.
fn handle_post_tool_use(
    input: &HookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    // Only handle Bash tool errors.
    if input.tool_name.as_deref() != Some("Bash") {
        return Ok(None);
    }

    let Some(ref response) = input.tool_response else {
        return Ok(None);
    };

    // Check for non-zero exit code. Handle both `exit_code` and `exitCode`.
    let exit_code = response
        .get("exit_code")
        .or_else(|| response.get("exitCode"))
        .and_then(serde_json::Value::as_i64);

    match exit_code {
        Some(0) | None => return Ok(None),
        Some(_) => {} // non-zero — proceed
    }

    // Extract stderr. Try top-level `stderr`, then nested under `result`.
    let stderr = response
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            response
                .get("result")
                .and_then(|r| r.get("stderr"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("");

    if stderr.is_empty() {
        lore_debug!("PostToolUse: empty stderr, skipping");
        return Ok(None);
    }

    // Use stderr as a search query (clean it into terms).
    let terms = split_into_words(stderr);
    let cleaned = clean_terms(&terms);
    if cleaned.is_empty() {
        return Ok(None);
    }

    let query = cleaned.join(" OR ");
    lore_debug!("PostToolUse: error query: {query}");
    let results = search_with_threshold(db, embedder, config, &query)?.flatten();

    if results.is_empty() {
        lore_debug!("PostToolUse: no results for error query");
        return Ok(None);
    }

    lore_debug!(
        "PostToolUse: injecting {} error-context chunks",
        results.len()
    );
    let context = format_imperative(&results);
    Ok(Some(HookOutput::HookSpecific {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PostToolUse".to_string(),
            additional_context: context,
        },
    }))
}

/// Search results split by injection policy.
///
/// `universal` carries chunks from `is_universal = 1` patterns above
/// `min_relevance` with no `top_k` cap — they are additive, re-injected on
/// every relevant `PreToolUse` call.
///
/// `ranked` carries non-universal chunks above `min_relevance`, capped at
/// `config.search.top_k`. They flow through the existing dedup pipeline.
#[derive(Debug, Default, Clone)]
pub struct PartitionedResults {
    pub universal: Vec<SearchResult>,
    pub ranked: Vec<SearchResult>,
}

impl PartitionedResults {
    /// Flatten into a single list with universal results first, then ranked.
    /// Used by `cmd_search` and the `PostToolUse` handler where the consumer
    /// does not need to know which chunks bypassed dedup.
    #[must_use]
    pub fn flatten(self) -> Vec<SearchResult> {
        let Self {
            mut universal,
            mut ranked,
        } = self;
        universal.append(&mut ranked);
        universal
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.universal.is_empty() && self.ranked.is_empty()
    }
}

/// Multiplier applied to `top_k` when querying the database for the raw
/// result set. Picked generously so a knowledge base with universal chunks
/// scattered across the ranking still returns them — the additive R5
/// promise can only deliver universal chunks the search actually sees.
const SEARCH_OVERFETCH_MULTIPLIER: usize = 10;

/// Shared search pipeline: embed, hybrid search, threshold filter, partition.
///
/// Returns universal chunks (uncapped) and ranked non-universal chunks (capped
/// at `config.search.top_k`) as separate slices. Callers that don't care
/// about the split can call `.flatten()`.
///
/// Extracted so that `cmd_search`, the `PreToolUse` handler, and the
/// `PostToolUse` handler all call the same function, avoiding drift between
/// the code paths.
pub fn search_with_threshold(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
    query: &str,
) -> anyhow::Result<PartitionedResults> {
    lore_debug!(
        "search: query={query:?} hybrid={} top_k={} min_relevance={:.4}",
        config.search.hybrid,
        config.search.top_k,
        config.search.min_relevance,
    );

    let mut embed_failed = false;

    let query_embedding = if config.search.hybrid {
        match embedder.embed(query) {
            Ok(v) => {
                lore_debug!("search: embedding succeeded ({} dims)", v.len());
                Some(v)
            }
            Err(e) => {
                eprintln!("Warning: Ollama unreachable ({e}), falling back to text search.");
                lore_debug!("search: embedding failed: {e}");
                embed_failed = true;
                None
            }
        }
    } else {
        None
    };

    // Over-fetch so universal chunks scattered in the ranking still surface.
    // Total cost is bounded — the multiplier ensures we don't over-fetch
    // pathologically when top_k is already large.
    let overfetch_limit = config
        .search
        .top_k
        .saturating_mul(SEARCH_OVERFETCH_MULTIPLIER);
    let results = db.search_hybrid(query, query_embedding.as_deref(), overfetch_limit)?;
    lore_debug!("search: {} raw results", results.len());

    let apply_threshold =
        config.search.hybrid && !embed_failed && config.search.min_relevance > 0.0;
    let results: Vec<_> = if apply_threshold {
        let before = results.len();
        let filtered: Vec<_> = results
            .into_iter()
            .filter(|r| r.score >= config.search.min_relevance)
            .collect();
        lore_debug!(
            "search: threshold={:.4} filtered {} -> {}",
            config.search.min_relevance,
            before,
            filtered.len(),
        );
        for r in &filtered {
            lore_debug!("  {:.4} {}", r.score, r.title);
        }
        filtered
    } else {
        for r in &results {
            lore_debug!("  {:.4} {}", r.score, r.title);
        }
        results
    };

    let (universal, mut ranked): (Vec<_>, Vec<_>) =
        results.into_iter().partition(|r| r.is_universal);
    ranked.truncate(config.search.top_k);

    lore_debug!(
        "search: partitioned {} universal + {} ranked (top_k={})",
        universal.len(),
        ranked.len(),
        config.search.top_k,
    );

    Ok(PartitionedResults { universal, ranked })
}

// ---------------------------------------------------------------------------
// Session context formatting
// ---------------------------------------------------------------------------

/// Format the meta-instruction + compact pattern index returned at session
/// start and after compaction.
///
/// Invokes `git rev-parse` against `knowledge_dir` (via [`crate::git::is_git_repo`])
/// to decide whether to inject the git advisory paragraph. `SessionStart` and
/// `PostCompact` are infrequent events, so the per-call subprocess cost is
/// acceptable.
fn format_session_context(db: &KnowledgeDB, knowledge_dir: &Path) -> anyhow::Result<String> {
    let patterns = db.list_patterns()?;

    let mut out = String::from(
        "This project uses lore for the author's strong coding preferences \
         and workflow conventions. Patterns are injected automatically via \
         additionalContext before your edits. Apply them as default \
         conventions — they take precedence over your training defaults but \
         yield to explicit project-level instructions (CLAUDE.md, AGENTS.md) \
         when they conflict.\n",
    );

    if !crate::git::is_git_repo(knowledge_dir) {
        out.push_str(
            "\nNote: this knowledge base is not a git repository. Pattern \
             writes via add_pattern, update_pattern, and append_to_pattern \
             will not be committed, delta ingest is unavailable, and there \
             is no version history. Run `git init` in the knowledge base \
             directory to enable these features. Use the lore_status tool \
             to inspect this state at any time.\n",
        );
    }

    let pinned = render_pinned_conventions(db, knowledge_dir)?;
    if !pinned.is_empty() {
        out.push_str(&pinned);
    }

    out.push_str("\nAvailable patterns:\n");

    for p in &patterns {
        if p.tags.is_empty() {
            let _ = writeln!(out, "- {}", p.title);
        } else {
            let _ = writeln!(out, "- {} [{}]", p.title, p.tags);
        }
    }

    Ok(out)
}

/// Escape control characters (ANSI escapes, newlines, tabs) in strings that
/// originate from the database or other semi-trusted sources before writing
/// them to stderr or `lore_debug!`. A tampered chunk row whose `source_file`
/// contains `\x1b[2J` must not clear the operator's terminal, and a row with
/// embedded newlines must not spoof structured output downstream.
pub fn sanitize_for_log(s: &str) -> String {
    s.chars().flat_map(char::escape_debug).collect()
}

/// Total-body cap (bytes) across all rendered universal patterns in a single
/// `## Pinned conventions` section.
///
/// Complements the per-file `UNIVERSAL_BODY_HARD_LIMIT_BYTES` ingest-time
/// reject. Even a single oversized file should never reach the agent context,
/// but this is the belt-and-braces guard against a tampered DB bypassing the
/// ingest-time check: once cumulative bytes exceed this cap, render emits a
/// visible truncation marker and stops.
pub const PINNED_SECTION_TOTAL_LIMIT_BYTES: usize = 32 * 1024;

/// Build the `## Pinned conventions` block for `SessionStart` and `PostCompact`.
///
/// Returns an empty string when no universal patterns exist (the section
/// header is then omitted entirely from the `SessionStart` payload). For each
/// universal pattern, validates the `source_file` against `knowledge_dir`
/// via `validate_within_dir` before reading it from disk — defends against
/// DB-tampering attacks where a `source_file` like `../../../etc/passwd`
/// could otherwise leak arbitrary file contents into the agent context.
///
/// Pattern files that fail validation, are missing, or fail to read are
/// individually skipped with a stderr log; the rest of the section still
/// renders. The hook's broader "never break the agent" contract means
/// any error here degrades to an empty pinned section rather than a hard
/// failure.
fn render_pinned_conventions(db: &KnowledgeDB, knowledge_dir: &Path) -> anyhow::Result<String> {
    let universal = db.universal_patterns()?;
    if universal.is_empty() {
        return Ok(String::new());
    }

    let header = "\n## Pinned conventions\n\n\
                  These patterns are tagged `universal` and apply across every \
                  tool call in this session. Treat them as always-on conventions.\n";
    let mut out = String::from(header);
    let budget_start = out.len();

    for pattern in &universal {
        let candidate = knowledge_dir.join(&pattern.source_file);
        let safe_source = sanitize_for_log(&pattern.source_file);
        let canonical = match crate::ingest::validate_within_dir(knowledge_dir, &candidate) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("lore hook: skipping pinned `{safe_source}`: {e}");
                lore_debug!("pinned containment check failed for {safe_source}: {e}");
                continue;
            }
        };
        // Read from the canonical path returned by validation. Reading from
        // the pre-canonical `candidate` re-opens the TOCTOU window — a
        // symlink swap between validation and open could redirect the read
        // to a file outside `knowledge_dir`.
        match std::fs::read_to_string(&canonical) {
            Ok(body) => {
                let body = body.trim_end();
                let consumed = out.len() - budget_start;
                let remaining = PINNED_SECTION_TOTAL_LIMIT_BYTES.saturating_sub(consumed);
                let heading = format!("\n### {}\n\n", pattern.title);
                if heading.len() + body.len() + 1 > remaining {
                    // The next pattern body would push the section over the
                    // render-time cap. Emit a visible truncation marker and
                    // stop. Ingest already rejects oversized universal
                    // patterns per file (UNIVERSAL_BODY_HARD_LIMIT_BYTES);
                    // hitting this guard means either a DB tamper bypassed
                    // the ingest check or the operator has so many universal
                    // files that the aggregate pushes over budget.
                    let _ = writeln!(
                        out,
                        "\n_[pinned conventions truncated at {PINNED_SECTION_TOTAL_LIMIT_BYTES} bytes — trim or retag universal patterns]_",
                    );
                    lore_debug!(
                        "pinned render truncated at {} bytes (next pattern: {safe_source})",
                        PINNED_SECTION_TOTAL_LIMIT_BYTES,
                    );
                    break;
                }
                out.push_str(&heading);
                out.push_str(body);
                out.push('\n');
            }
            Err(e) => {
                eprintln!("lore hook: skipping pinned `{safe_source}`: read failed ({e})");
                lore_debug!("pinned read failed for {safe_source}: {e}");
            }
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Dedup file helpers
// ---------------------------------------------------------------------------

/// Derive the dedup file path from the session ID in the input.
/// Returns `None` if no session ID is present.
fn session_dedup_path(input: &HookInput) -> Option<PathBuf> {
    input.session_id.as_deref().map(dedup_file_path)
}

/// Return the dedup file path for a given session ID.
///
/// Uses FNV-1a to hash the session ID into a deterministic 16-hex-char
/// filename, avoiding collision from character-level sanitisation and
/// preventing raw session IDs from leaking into `/tmp` filenames.
pub fn dedup_file_path(session_id: &str) -> PathBuf {
    let hash = crate::hash::fnv1a(session_id.as_bytes());
    std::env::temp_dir().join(format!("lore-session-{hash:016x}"))
}

/// Read chunk IDs from the dedup file. Returns an empty set on any error
/// (missing file, permission denied, etc.).
pub fn read_dedup(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|contents| {
            contents
                .lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Append chunk IDs to the dedup file (one per line).
pub fn write_dedup(path: &Path, ids: &[&str]) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for id in ids {
        writeln!(file, "{id}")?;
    }
    Ok(())
}

/// Create or truncate the dedup file under an exclusive advisory lock.
pub fn reset_dedup(path: &Path) -> anyhow::Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write().map_err(|e| anyhow::anyhow!("{e}"))?;
    // File is already truncated by OpenOptions; lock ensures no concurrent
    // reader sees a partial state.
    Ok(())
}

/// Read seen chunk IDs, filter results, and record newly seen IDs — all
/// under a single exclusive file lock to prevent TOCTOU races between
/// concurrent hook invocations.
///
/// Takes results by reference so the caller retains ownership and can fall
/// back to the unfiltered set on error.
///
/// Universal-tagged chunks bypass the read-side `seen.contains` check so
/// they re-inject on every relevant tool call regardless of dedup state.
/// They are still recorded on the write side — the dedup file remains a
/// faithful "what was injected this session" log, and the read-side
/// exemption is the defensive choice per
/// `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`.
fn dedup_filter_and_record(
    path: &Path,
    results: &[SearchResult],
) -> anyhow::Result<Vec<SearchResult>> {
    use std::io::Write as _;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)?;
    let mut lock = fd_lock::RwLock::new(file);
    let mut guard = lock.write().map_err(|e| anyhow::anyhow!("{e}"))?;

    // Read seen chunk IDs.
    let mut contents = String::new();
    guard.read_to_string(&mut contents)?;
    let seen: HashSet<String> = contents
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    // Filter out already-injected chunks. Universal chunks bypass the
    // `seen.contains` check entirely.
    let filtered: Vec<SearchResult> = results
        .iter()
        .filter(|r| r.is_universal || !seen.contains(&r.id))
        .cloned()
        .collect();

    // Record newly seen chunk IDs while still holding the lock. Universal
    // chunks are recorded too — the dedup file is the canonical injection
    // log and must reflect every chunk we surfaced this session.
    for r in &filtered {
        writeln!(&mut *guard, "{}", r.id)?;
    }

    Ok(filtered)
}

// ---------------------------------------------------------------------------
// Query extraction
// ---------------------------------------------------------------------------

/// Build an FTS5 query from tool input signals.
///
/// Returns `None` when no meaningful terms can be extracted.
pub fn extract_query(input: &HookInput) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut language: Option<String> = None;

    // 1. File path signals (Edit, Write, Read, etc.)
    if let Some(file_path) = tool_input_str(input, "file_path") {
        if let Some(lang) = language_from_extension(&file_path) {
            language = Some(lang);
        }
        terms.extend(filename_terms(&file_path));
    }

    // 2. Bash signals
    if input.tool_name.as_deref() == Some("Bash") {
        let text = tool_input_str(input, "description")
            .or_else(|| tool_input_str(input, "command"))
            .unwrap_or_default();

        if language.is_none() {
            language = language_from_bash(&text);
        }

        terms.extend(split_into_words(&text));
    }

    // 3. Transcript tail (last user message).
    // Validate that the transcript path is under $HOME before reading.
    // Use the canonical path returned from validation to prevent symlink
    // TOCTOU between validation and file open.
    if let Some(ref path) = input.transcript_path
        && let Some(canonical) = validate_transcript_path(Path::new(path))
        && let Some(msg) = last_user_message(&canonical)
    {
        let truncated = truncate_str(&msg, 200);
        terms.extend(split_into_words(truncated));
    }

    // 4. Clean terms
    let cleaned = clean_terms(&terms);

    // 5. Assemble FTS5 query
    match (language, cleaned.is_empty()) {
        // Language anchor + enrichment terms: `lang AND (term1 OR term2 OR ...)`
        (Some(lang), false) => {
            let or_clause = cleaned.join(" OR ");
            Some(format!("{lang} AND ({or_clause})"))
        }
        // Language anchor only (no enrichment survived cleaning): just the language
        (Some(lang), true) => Some(lang),
        // No language anchor, but enrichment terms: OR-only query
        (None, false) => Some(cleaned.join(" OR ")),
        // Nothing useful extracted
        (None, true) => None,
    }
}

/// Returns `true` if the agent type is read-only and should not receive
/// pattern injection (e.g. Explore, Plan subagents).
fn skip_agent(input: &HookInput) -> bool {
    matches!(input.agent_type.as_deref(), Some("Explore" | "Plan"))
}

/// Extract a string field from `tool_input` by key.
fn tool_input_str(input: &HookInput, key: &str) -> Option<String> {
    input
        .tool_input
        .as_ref()?
        .get(key)?
        .as_str()
        .map(String::from)
}

/// Map file extension to a language keyword for FTS anchor.
fn language_from_extension(path: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext.to_lowercase().as_str() {
        "ts" | "tsx" => Some("typescript".to_string()),
        "rs" => Some("rust".to_string()),
        "js" | "jsx" => Some("javascript".to_string()),
        "yml" | "yaml" => Some("yaml".to_string()),
        "py" => Some("python".to_string()),
        "go" => Some("golang".to_string()),
        _ => None,
    }
}

/// Infer language from a Bash command string.
fn language_from_bash(command: &str) -> Option<String> {
    let lower = command.to_lowercase();
    if lower.contains("npm")
        || lower.contains("npx")
        || lower.contains("yarn")
        || lower.contains("bun")
    {
        return Some("typescript".to_string());
    }
    if lower.contains("cargo") {
        return Some("rust".to_string());
    }
    if lower.contains("pip") || lower.contains("python") {
        return Some("python".to_string());
    }
    None
}

/// Extract terms from a filename: take the basename (without extension),
/// split `camelCase` and `PascalCase`, lowercase everything.
fn filename_terms(path: &str) -> Vec<String> {
    let basename = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    split_camel_case(basename)
        .into_iter()
        .flat_map(|w| split_into_words(&w))
        .collect()
}

/// Split a `camelCase` or `PascalCase` string into individual words.
fn split_camel_case(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in s.chars() {
        if ch.is_uppercase() && !current.is_empty() {
            words.push(current.clone());
            current.clear();
        }
        if ch.is_alphanumeric() {
            current.push(ch.to_lowercase().next().unwrap_or(ch));
        } else if !current.is_empty() {
            // Non-alphanumeric boundary (hyphens, underscores, dots, etc.)
            words.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Split a string on whitespace and non-alphabetic boundaries, lowercase.
fn split_into_words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Validate that a transcript path is under `$HOME`.
///
/// Returns `Some(canonical_path)` if valid, `None` if the path is outside
/// `$HOME`, doesn't exist, or `$HOME` is not set. Consistent with the
/// existing fallthrough where `last_user_message` returns `None`.
fn validate_transcript_path(path: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let home = PathBuf::from(home);
    let canonical = path.canonicalize().ok()?;
    if canonical.starts_with(&home) {
        Some(canonical)
    } else {
        lore_debug!(
            "transcript path outside $HOME, skipping: {}",
            path.display()
        );
        None
    }
}

/// Maximum bytes to read from the tail of a transcript file.
const TRANSCRIPT_TAIL_BYTES: usize = 32_768;

/// Read the last ~32KB of a transcript JSONL file in reverse to find the
/// last user message. Bounds the read to prevent OOM on large transcripts.
fn last_user_message(path: &Path) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let file_len = file.metadata().ok()?.len() as usize;

    let buf = if file_len > TRANSCRIPT_TAIL_BYTES {
        #[allow(clippy::cast_possible_wrap)]
        file.seek(SeekFrom::End(-(TRANSCRIPT_TAIL_BYTES as i64)))
            .ok()?;
        let mut buf = Vec::with_capacity(TRANSCRIPT_TAIL_BYTES);
        file.read_to_end(&mut buf).ok()?;
        buf
    } else {
        let mut buf = Vec::with_capacity(file_len);
        file.read_to_end(&mut buf).ok()?;
        buf
    };

    let contents = String::from_utf8_lossy(&buf);

    // If we seeked into the middle, discard the first partial JSONL line.
    let contents = if file_len > TRANSCRIPT_TAIL_BYTES {
        match contents.find('\n') {
            Some(pos) => &contents[pos + 1..],
            None => return None, // entire buffer is one partial line
        }
    } else {
        &contents
    };

    // Walk lines in reverse, find the last one with `"type":"user"`.
    for line in contents.lines().rev() {
        if !line.contains("\"type\":\"user\"") {
            continue;
        }
        // Try parsing as JSON and extracting the message content.
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
        {
            return Some(content.to_string());
        }
    }
    None
}

/// Truncate a string to at most `max_bytes` bytes (on a valid UTF-8 char
/// boundary).
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the largest byte offset that is both <= max_bytes and a valid
    // char boundary.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Clean terms: strip non-alpha, filter short, filter hex-like, filter stop
/// words, deduplicate while preserving order.
fn clean_terms(raw: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for term in raw {
        // Strip non-alphabetic characters.
        let cleaned: String = term.chars().filter(|c| c.is_alphabetic()).collect();
        let lower = cleaned.to_lowercase();

        // Filter terms shorter than 3 chars.
        if lower.len() < 3 {
            continue;
        }

        // Filter hex-like strings (6+ hex characters).
        if is_hex_like(&lower) {
            continue;
        }

        // Filter stop words.
        if STOP_WORDS.contains(&lower.as_str()) {
            continue;
        }

        // Deduplicate.
        if seen.insert(lower.clone()) {
            result.push(lower);
        }
    }

    result
}

/// Returns `true` if the string looks like a hex fragment (>= 6 chars,
/// all `[0-9a-f]`).
fn is_hex_like(s: &str) -> bool {
    s.len() >= 6 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// Imperative formatting
// ---------------------------------------------------------------------------

/// Format search results as imperative directives for agent context.
///
/// Groups results by source file and concatenates all bodies.
pub fn format_imperative(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }

    // Group results by source_file, preserving order of first appearance.
    let mut groups: BTreeMap<&str, Vec<&SearchResult>> = BTreeMap::new();
    for r in results {
        groups.entry(&r.source_file).or_default().push(r);
    }

    let mut out = String::new();

    for (source, items) in &groups {
        let _ = writeln!(out, "PROJECT CONVENTIONS (source: {source})");
        out.push_str("Apply these patterns when writing this code:\n\n");

        for item in items {
            out.push_str(&item.body);
            if !item.body.ends_with('\n') {
                out.push('\n');
            }
        }
    }

    // Trim trailing whitespace.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- extract_query -------------------------------------------------------

    #[test]
    fn extract_query_rs_file_path() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Edit".to_string()),
            tool_input: Some(serde_json::json!({"file_path": "src/validate_email.rs"})),
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };

        let query = extract_query(&input).unwrap();
        assert!(
            query.contains("rust"),
            "should have language anchor: {query}"
        );
        assert!(
            query.contains("validate"),
            "should have filename term: {query}"
        );
        assert!(
            query.contains("email"),
            "should have filename term: {query}"
        );
    }

    #[test]
    fn extract_query_ts_file_path() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Edit".to_string()),
            tool_input: Some(serde_json::json!({"file_path": "src/components/UserProfile.tsx"})),
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };

        let query = extract_query(&input).unwrap();
        assert!(
            query.contains("typescript"),
            "should have language anchor: {query}"
        );
        assert!(query.contains("user"), "should have filename term: {query}");
        assert!(
            query.contains("profile"),
            "should have filename term: {query}"
        );
    }

    #[test]
    fn extract_query_bash_with_cargo() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(
                serde_json::json!({"description": "Run cargo test for error handling"}),
            ),
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };

        let query = extract_query(&input).unwrap();
        assert!(
            query.contains("rust"),
            "should infer rust from cargo: {query}"
        );
        assert!(
            query.contains("error"),
            "should extract term from description: {query}"
        );
    }

    #[test]
    fn extract_query_bash_command_fallback() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "npm test authentication"})),
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };

        let query = extract_query(&input).unwrap();
        assert!(
            query.contains("typescript"),
            "should infer typescript from npm: {query}"
        );
        assert!(
            query.contains("authentication"),
            "should extract term: {query}"
        );
    }

    #[test]
    fn extract_query_no_signals_returns_none() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Read".to_string()),
            tool_input: Some(serde_json::json!({"file_path": "a.txt"})),
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };

        // .txt has no language anchor, and "a" is too short after cleaning.
        assert!(extract_query(&input).is_none());
    }

    // -- skip_agent ----------------------------------------------------------

    #[test]
    fn skip_agent_explore() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: None,
            tool_input: None,
            agent_type: Some("Explore".to_string()),
            transcript_path: None,
            tool_response: None,
        };
        assert!(skip_agent(&input));
    }

    #[test]
    fn skip_agent_plan() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: None,
            tool_input: None,
            agent_type: Some("Plan".to_string()),
            transcript_path: None,
            tool_response: None,
        };
        assert!(skip_agent(&input));
    }

    #[test]
    fn skip_agent_normal() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: None,
            tool_input: None,
            agent_type: Some("Main".to_string()),
            transcript_path: None,
            tool_response: None,
        };
        assert!(!skip_agent(&input));
    }

    #[test]
    fn skip_agent_none() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: None,
            tool_input: None,
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };
        assert!(!skip_agent(&input));
    }

    // -- split_camel_case ----------------------------------------------------

    #[test]
    fn split_camel_validate_email() {
        let parts = split_camel_case("validateEmail");
        assert_eq!(parts, vec!["validate", "email"]);
    }

    #[test]
    fn split_camel_pascal_case() {
        let parts = split_camel_case("UserProfile");
        assert_eq!(parts, vec!["user", "profile"]);
    }

    #[test]
    fn split_camel_snake_case() {
        let parts = split_camel_case("error_handling");
        assert_eq!(parts, vec!["error", "handling"]);
    }

    // -- is_hex_like ---------------------------------------------------------

    #[test]
    fn hex_like_true() {
        assert!(is_hex_like("abcdef"));
        assert!(is_hex_like("1a2b3c4d"));
        assert!(is_hex_like("deadbeef"));
    }

    #[test]
    fn hex_like_false() {
        assert!(!is_hex_like("abcde")); // too short
        assert!(!is_hex_like("abcxyz")); // non-hex chars
        assert!(!is_hex_like("rust")); // not hex
    }

    // -- clean_terms ---------------------------------------------------------

    #[test]
    fn clean_removes_stop_words_and_short() {
        let terms: Vec<String> = vec![
            "the".into(),
            "a".into(),
            "rust".into(),
            "error".into(),
            "handling".into(),
        ];
        let cleaned = clean_terms(&terms);
        assert!(!cleaned.contains(&"the".to_string()));
        assert!(!cleaned.contains(&"a".to_string()));
        assert!(cleaned.contains(&"rust".to_string()));
        assert!(cleaned.contains(&"error".to_string()));
    }

    #[test]
    fn clean_deduplicates() {
        let terms: Vec<String> = vec!["error".into(), "error".into(), "handling".into()];
        let cleaned = clean_terms(&terms);
        assert_eq!(
            cleaned.iter().filter(|t| *t == "error").count(),
            1,
            "should deduplicate"
        );
    }

    #[test]
    fn clean_filters_hex() {
        let terms: Vec<String> = vec!["deadbeef".into(), "rust".into()];
        let cleaned = clean_terms(&terms);
        assert!(!cleaned.contains(&"deadbeef".to_string()));
        assert!(cleaned.contains(&"rust".to_string()));
    }

    // -- format_imperative ---------------------------------------------------

    #[test]
    fn format_imperative_single_source() {
        let results = vec![SearchResult {
            id: "c1".into(),
            title: "Error Handling".into(),
            body: "Use anyhow for errors.".into(),
            tags: String::new(),
            source_file: "errors.md".into(),
            heading_path: String::new(),
            score: 0.8,
            is_universal: false,
        }];

        let formatted = format_imperative(&results);
        assert!(formatted.contains("PROJECT CONVENTIONS (source: errors.md)"));
        assert!(formatted.contains("Apply these patterns"));
        assert!(formatted.contains("Use anyhow for errors."));
    }

    #[test]
    fn format_imperative_multiple_sources() {
        let results = vec![
            SearchResult {
                id: "c1".into(),
                title: "Error Handling".into(),
                body: "Use anyhow for errors.".into(),
                tags: String::new(),
                source_file: "errors.md".into(),
                heading_path: String::new(),
                score: 0.8,
                is_universal: false,
            },
            SearchResult {
                id: "c2".into(),
                title: "Naming".into(),
                body: "Use snake_case.".into(),
                tags: String::new(),
                source_file: "naming.md".into(),
                heading_path: String::new(),
                score: 0.7,
                is_universal: false,
            },
        ];

        let formatted = format_imperative(&results);
        assert!(formatted.contains("PROJECT CONVENTIONS (source: errors.md)"));
        assert!(formatted.contains("PROJECT CONVENTIONS (source: naming.md)"));
    }

    #[test]
    fn format_imperative_empty_results() {
        let formatted = format_imperative(&[]);
        assert!(formatted.is_empty());
    }

    // -- language_from_extension ---------------------------------------------

    #[test]
    fn language_extension_rs() {
        assert_eq!(
            language_from_extension("src/main.rs"),
            Some("rust".to_string())
        );
    }

    #[test]
    fn language_extension_tsx() {
        assert_eq!(
            language_from_extension("App.tsx"),
            Some("typescript".to_string())
        );
    }

    #[test]
    fn language_extension_unknown() {
        assert_eq!(language_from_extension("notes.txt"), None);
    }

    // -- language_from_bash --------------------------------------------------

    #[test]
    fn bash_npm_is_typescript() {
        assert_eq!(
            language_from_bash("npm install express"),
            Some("typescript".to_string())
        );
    }

    #[test]
    fn bash_cargo_is_rust() {
        assert_eq!(
            language_from_bash("cargo build --release"),
            Some("rust".to_string())
        );
    }

    #[test]
    fn bash_pip_is_python() {
        assert_eq!(
            language_from_bash("pip install requests"),
            Some("python".to_string())
        );
    }

    #[test]
    fn bash_unknown_is_none() {
        assert_eq!(language_from_bash("ls -la"), None);
    }

    // -- transcript_path ---------------------------------------------------

    #[test]
    fn last_user_message_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"assistant","message":{"content":"hello"}}
{"type":"user","message":{"content":"fix the error handling"}}
{"type":"assistant","message":{"content":"done"}}
"#,
        )
        .unwrap();

        let msg = last_user_message(&path).unwrap();
        assert_eq!(msg, "fix the error handling");
    }

    #[test]
    fn last_user_message_missing_file() {
        let path = std::env::temp_dir().join("lore-nonexistent-transcript.jsonl");
        assert!(last_user_message(&path).is_none());
    }

    #[test]
    fn last_user_message_bounded_read_small_file() {
        // A file smaller than 32KB should be read in full.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"user","message":{"content":"first"}}
{"type":"user","message":{"content":"second"}}
"#,
        )
        .unwrap();

        let msg = last_user_message(&path).unwrap();
        assert_eq!(msg, "second");
    }

    #[test]
    fn last_user_message_bounded_read_large_file() {
        // A file larger than 32KB should only read the tail.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.jsonl");

        let mut content = String::new();
        // Write enough filler lines to exceed 32KB.
        for i in 0..500 {
            use std::fmt::Write as _;
            let _ = writeln!(
                content,
                "{{\"type\":\"assistant\",\"message\":{{\"content\":\"filler line {i} {}\"}}}}",
                "x".repeat(100)
            );
        }
        // The last user message should be near the end.
        content.push_str("{\"type\":\"user\",\"message\":{\"content\":\"the real query\"}}\n");
        content.push_str("{\"type\":\"assistant\",\"message\":{\"content\":\"response\"}}\n");

        assert!(content.len() > 32_768, "test file should exceed 32KB");
        std::fs::write(&path, &content).unwrap();

        let msg = last_user_message(&path).unwrap();
        assert_eq!(msg, "the real query");
    }

    #[test]
    fn last_user_message_discards_partial_first_line() {
        // When seeking into the middle of a file, the first partial line
        // should be discarded rather than causing a parse error.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.jsonl");

        let mut content = String::new();
        // Write enough data to exceed 32KB.
        for _ in 0..400 {
            content.push_str(
                "{\"type\":\"assistant\",\"message\":{\"content\":\"padding padding padding padding padding padding padding padding\"}}\n",
            );
        }
        content.push_str("{\"type\":\"user\",\"message\":{\"content\":\"query after padding\"}}\n");

        assert!(content.len() > 32_768);
        std::fs::write(&path, &content).unwrap();

        let msg = last_user_message(&path).unwrap();
        assert_eq!(msg, "query after padding");
    }

    #[test]
    fn last_user_message_no_user_messages_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-user.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":\"hello\"}}\n",
        )
        .unwrap();

        assert!(last_user_message(&path).is_none());
    }

    // -- validate_transcript_path ---------------------------------------------

    #[test]
    fn validate_transcript_path_under_home() {
        // A file under $HOME should pass validation.
        let home = std::env::var("HOME").unwrap();
        let dir = tempfile::tempdir_in(&home).unwrap();
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, "").unwrap();

        assert!(
            validate_transcript_path(&path).is_some(),
            "path under $HOME should be valid"
        );
    }

    #[test]
    fn validate_transcript_path_outside_home() {
        // A file outside $HOME should fail validation.
        // /tmp is typically NOT under $HOME.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evil.jsonl");
        std::fs::write(&path, "").unwrap();

        let home = std::env::var("HOME").unwrap();
        if !dir.path().starts_with(&home) {
            assert!(
                validate_transcript_path(&path).is_none(),
                "path outside $HOME should be rejected"
            );
        }
        // If tmp IS under $HOME (unusual), skip this assertion.
    }

    #[test]
    fn validate_transcript_path_nonexistent() {
        let path = PathBuf::from("/nonexistent/path/transcript.jsonl");
        assert!(
            validate_transcript_path(&path).is_none(),
            "nonexistent path should return None"
        );
    }

    // -- dedup_file_path ------------------------------------------------------

    #[test]
    fn dedup_file_path_returns_deterministic_hash() {
        let path = dedup_file_path("60de87ba-e944-42c0-91f5-3cd3c38938de");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("lore-session-"));
        // 16 hex chars after the prefix.
        let hash_part = filename.strip_prefix("lore-session-").unwrap();
        assert_eq!(hash_part.len(), 16, "hash should be 16 hex chars");
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex: {hash_part}"
        );
        assert!(path.starts_with(std::env::temp_dir()));

        // Same input always produces the same hash.
        let path2 = dedup_file_path("60de87ba-e944-42c0-91f5-3cd3c38938de");
        assert_eq!(path, path2, "same session ID must produce same path");
    }

    #[test]
    fn dedup_file_path_similar_ids_produce_different_hashes() {
        // These IDs would have collided under character-level sanitisation
        // (both would become "abc-123") but should differ under hashing.
        let path_a = dedup_file_path("abc:123");
        let path_b = dedup_file_path("abc/123");
        assert_ne!(path_a, path_b, "similar IDs must hash to different paths");
    }

    #[test]
    fn dedup_file_path_empty_session_id() {
        let path = dedup_file_path("");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.starts_with("lore-session-"),
            "empty ID should still produce a valid filename"
        );
        let hash_part = filename.strip_prefix("lore-session-").unwrap();
        assert_eq!(hash_part.len(), 16);
    }

    // -- read_dedup / write_dedup / reset_dedup --------------------------------

    #[test]
    fn read_dedup_missing_file_returns_empty() {
        let path = std::env::temp_dir().join("lore-nonexistent-dedup-file");
        let set = read_dedup(&path);
        assert!(set.is_empty());
    }

    #[test]
    fn write_dedup_read_dedup_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1", "c2", "c3"]).unwrap();
        let ids = read_dedup(&path);
        assert_eq!(ids.len(), 3);
        assert!(ids.contains("c1"));
        assert!(ids.contains("c2"));
        assert!(ids.contains("c3"));
    }

    #[test]
    fn write_dedup_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1"]).unwrap();
        write_dedup(&path, &["c2"]).unwrap();
        let ids = read_dedup(&path);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("c1"));
        assert!(ids.contains("c2"));
    }

    #[test]
    fn reset_dedup_clears_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1", "c2"]).unwrap();
        reset_dedup(&path).unwrap();
        let ids = read_dedup(&path);
        assert!(ids.is_empty(), "should be empty after reset");
    }

    #[test]
    fn reset_dedup_creates_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        reset_dedup(&path).unwrap();
        assert!(path.exists());
        let ids = read_dedup(&path);
        assert!(ids.is_empty());
    }

    #[test]
    fn reset_dedup_truncates_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1"]).unwrap();
        reset_dedup(&path).unwrap();
        let ids = read_dedup(&path);
        assert!(ids.is_empty(), "reset should truncate existing content");
    }

    // -- sanitize_for_log ----------------------------------------------------

    #[test]
    fn sanitize_for_log_escapes_ansi_and_newlines() {
        // ANSI CSI sequence must be rendered as visible escapes so a tampered
        // DB row cannot clear or recolour the operator's terminal.
        let payload = "\x1b[2J\x1b[31malert\x1b[0m\nnext";
        let sanitized = sanitize_for_log(payload);
        assert!(
            !sanitized.contains('\x1b'),
            "raw ESC must not leak through: {sanitized}"
        );
        assert!(
            !sanitized.contains('\n'),
            "raw newline must not leak through: {sanitized}"
        );
        assert!(
            sanitized.contains("alert"),
            "visible content should survive sanitisation: {sanitized}"
        );
    }

    #[test]
    fn sanitize_for_log_passes_through_printable_ascii() {
        let payload = "patterns/git-branch-pr.md";
        assert_eq!(sanitize_for_log(payload), payload);
    }

    // -- dedup_filter_and_record -----------------------------------------------

    fn make_search_result(id: &str) -> crate::database::SearchResult {
        crate::database::SearchResult {
            id: id.to_string(),
            title: String::new(),
            body: String::new(),
            tags: String::new(),
            source_file: "test.md".to_string(),
            heading_path: String::new(),
            score: 1.0,
            is_universal: false,
        }
    }

    #[test]
    fn dedup_filter_and_record_filters_seen_and_records_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        // Seed the dedup file with one existing ID.
        write_dedup(&path, &["c1"]).unwrap();

        let results = vec![
            make_search_result("c1"),
            make_search_result("c2"),
            make_search_result("c3"),
        ];

        let filtered = dedup_filter_and_record(&path, &results).unwrap();
        assert_eq!(filtered.len(), 2, "c1 should be filtered out");
        assert!(filtered.iter().all(|r| r.id != "c1"));

        // Verify that c2 and c3 were recorded.
        let seen = read_dedup(&path);
        assert!(seen.contains("c1"));
        assert!(seen.contains("c2"));
        assert!(seen.contains("c3"));
    }

    #[test]
    fn dedup_filter_and_record_sequential_accumulates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        // Create the file.
        reset_dedup(&path).unwrap();

        // First invocation records c1.
        let r1 = vec![make_search_result("c1")];
        let filtered1 = dedup_filter_and_record(&path, &r1).unwrap();
        assert_eq!(filtered1.len(), 1);

        // Second invocation should filter c1, keep c2.
        let r2 = vec![make_search_result("c1"), make_search_result("c2")];
        let filtered2 = dedup_filter_and_record(&path, &r2).unwrap();
        assert_eq!(filtered2.len(), 1);
        assert_eq!(filtered2[0].id, "c2");

        // Both should now be recorded.
        let seen = read_dedup(&path);
        assert_eq!(seen.len(), 2);
        assert!(seen.contains("c1"));
        assert!(seen.contains("c2"));
    }

    #[test]
    fn reset_dedup_clears_under_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1", "c2"]).unwrap();
        reset_dedup(&path).unwrap();

        // After reset, filter_and_record should see no prior IDs.
        let results = vec![make_search_result("c1")];
        let filtered = dedup_filter_and_record(&path, &results).unwrap();
        assert_eq!(filtered.len(), 1, "c1 should pass after reset");
    }
}
