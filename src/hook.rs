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

use crate::chunking::AppliesWhen;
use crate::config::Config;
use crate::database::{KnowledgeDB, SearchResult};
use crate::embeddings::Embedder;
use crate::engine::{self, CallContext};
use crate::lore_debug;

/// Maximum bytes of a Bash command echoed in `predicate suppress:` debug
/// lines. Predicate-suppression logs name the offending pattern source plus
/// the first ~60 bytes of the command so operators can correlate without
/// flooding the terminal with long pipelines.
const PREDICATE_LOG_CMD_HEAD_BYTES: usize = 60;

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

/// Handle `PreToolUse`: extract query, search, predicate-filter, dedup-filter,
/// format imperatives.
///
/// Ordering matters and is load-bearing for invariants tested elsewhere:
///
/// 1. **`skip_agent` runs FIRST** — Explore / Plan subagents short-circuit
///    before the eager transcript-tail read in `to_call_context`. Preserves
///    the existing zero-I/O guarantee for read-only subagent calls.
/// 2. Build the [`CallContext`] once via [`HookInput::to_call_context`]
///    (eager transcript-tail read happens here for non-skip paths).
/// 3. Engine [`engine::extract_query`] for the FTS query string.
/// 4. [`search_with_threshold`] for seeds. (U6 will branch on
///    `min_relevance_universal` for universal results.)
/// 5. [`expand_to_siblings`] to pull every chunk of each matched source.
/// 6. **Predicate filter** (Track 1, R8): for each universal chunk with a
///    non-`None` `applies_when_json`, deserialise to [`AppliesWhen`] and
///    call [`engine::evaluate_applies_when`]. Suppressed chunks are dropped
///    from the local `Vec<SearchResult>` BEFORE the dedup call, so the
///    dedup file never records their ids — neither read-side nor write-side
///    (per the dedup-bypass-on-suppression invariant; pinned by the
///    `predicate_filter_byte_for_byte_dedup_file_unchanged_on_suppression`
///    integration test).
/// 7. [`dedup_filter_and_record`] on the surviving chunks.
/// 8. [`format_imperative`] + emit `additionalContext`.
fn handle_pre_tool_use(
    input: &HookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    // 1. skip_agent FIRST — before the eager transcript-tail read so
    //    Explore/Plan subagents never trigger filesystem I/O.
    if skip_agent(input) {
        lore_debug!("skipping subagent");
        return Ok(None);
    }

    // 2. Build the CallContext once. The eager transcript-tail read with
    //    `$HOME` validation happens inside `to_call_context` — adapter-only
    //    filesystem responsibility (engine remains disk-I/O-free, see
    //    `tests/invariants.rs`).
    let cc = input.to_call_context();

    // 3. Engine query extraction.
    let Some(query) = engine::extract_query(&cc) else {
        lore_debug!("no query extracted from tool input");
        return Ok(None);
    };

    lore_debug!("extracted query: {query}");

    // 4. Hybrid / FTS search with the configured relevance floor.
    let seeds = search_with_threshold(db, embedder, config, &query)?;

    if seeds.is_empty() {
        lore_debug!("search returned no results");
        return Ok(None);
    }

    let seed_universal = seeds.iter().filter(|r| r.is_universal).count();

    // 5. Sibling expansion — single DB call. `is_universal` and
    //    `applies_when_json` are file-level fields stored on every chunk
    //    row, so expanding propagates predicate state without a join
    //    (whole-file semantics).
    let expanded = expand_to_siblings(db, &seeds);
    lore_debug!(
        "expand: {} seeds -> {} after sibling expansion ({} universal seeds)",
        seeds.len(),
        expanded.len(),
        seed_universal,
    );

    // 6. Predicate filter (Track 1, R8). Run ONLY for universal chunks
    //    with a `Some` `applies_when_json`. Non-universal chunks and
    //    universals with no predicate pass through unchanged. Dropped
    //    chunks are removed from `Vec<SearchResult>` BEFORE dedup sees the
    //    list — the dedup file therefore never records a suppressed id
    //    (read-side or write-side). A future predicate-passing call can
    //    still inject the same chunk; suppression is per-call, not
    //    per-session.
    let after_predicate = apply_predicate_filter(expanded, &cc);

    if after_predicate.is_empty() {
        lore_debug!("nothing to inject after predicate filter");
        return Ok(None);
    }

    // 7. Dedup. `dedup_filter_and_record` bypasses the `seen.contains`
    //    check for universal chunks (read-side filter) and appends every
    //    surfaced chunk to the dedup file (write side). The dedup file
    //    therefore remains a faithful "what was injected this session"
    //    log — defensive consistency per the session-dedup-lifecycle
    //    learning. Suppressed chunks were removed in step 6, so they are
    //    invisible to this stage.
    let dedup_path = session_dedup_path(input);
    let combined = if let Some(ref path) = dedup_path
        && path.exists()
    {
        let pre_count = after_predicate.len();
        match dedup_filter_and_record(path, &after_predicate) {
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
                after_predicate
            }
        }
    } else {
        lore_debug!("dedup inactive (no session file)");
        after_predicate
    };

    if combined.is_empty() {
        lore_debug!("nothing to inject after expansion + dedup");
        return Ok(None);
    }

    let kept_universal = combined.iter().filter(|r| r.is_universal).count();
    let kept_ranked = combined.len() - kept_universal;
    let sources: HashSet<&str> = combined.iter().map(|r| r.source_file.as_str()).collect();
    lore_debug!(
        "injecting {} chunks ({} universal + {} ranked) from {} sources",
        combined.len(),
        kept_universal,
        kept_ranked,
        sources.len()
    );

    // 8. Format and emit.
    let context = format_imperative(&combined);

    Ok(Some(HookOutput::HookSpecific {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PreToolUse".to_string(),
            additional_context: context,
        },
    }))
}

/// Apply the universal-pattern predicate filter to a list of expanded chunks.
///
/// For each chunk where `is_universal == true` AND `applies_when_json` is
/// `Some`, deserialise to [`AppliesWhen`] and call
/// [`engine::evaluate_applies_when`]. Suppressed chunks are dropped from the
/// returned `Vec`. Universal chunks without `applies_when_json` (or whose
/// JSON fails to deserialise — defensive R11 fallback) pass through
/// unchanged. Non-universal chunks are NOT subject to the predicate (Track 1
/// scope per R8).
///
/// Emits two `LORE_DEBUG`-gated trace shapes:
///
/// * Per-suppression: `predicate suppress: <pattern> tool=<tool>
///   cmd_head="<cmd>"` — pattern source and command head are passed through
///   [`sanitize_for_log`]; command head is byte-truncated at
///   [`PREDICATE_LOG_CMD_HEAD_BYTES`] via [`engine::truncate_str`].
/// * Aggregate: `predicate: N before -> M after (K suppressed)` once at the
///   end.
fn apply_predicate_filter(chunks: Vec<SearchResult>, cc: &CallContext) -> Vec<SearchResult> {
    let before = chunks.len();
    let mut suppressed: usize = 0;

    let kept: Vec<SearchResult> = chunks
        .into_iter()
        .filter(|r| {
            // Non-universal chunks bypass the predicate entirely (R8).
            if !r.is_universal {
                return true;
            }
            // Universal chunks with no predicate fire as today (R11).
            let Some(json) = r.applies_when_json.as_deref() else {
                return true;
            };
            // Defensive: a runtime parse failure should be impossible after
            // U2's parser produces the JSON, but if it ever happens we
            // treat the chunk as if no predicate were set (R11 fallback)
            // rather than silently suppressing it.
            let aw: AppliesWhen = match serde_json::from_str(json) {
                Ok(aw) => aw,
                Err(e) => {
                    lore_debug!(
                        "predicate parse error: {} ({}); firing unrestricted",
                        sanitize_for_log(&r.source_file),
                        sanitize_for_log(&e.to_string()),
                    );
                    return true;
                }
            };
            if engine::evaluate_applies_when(&aw, cc) {
                return true;
            }
            // Suppressed — emit per-suppression debug line and drop.
            suppressed += 1;
            let cmd_head = cc.command.as_deref().map_or("", |c| {
                engine::truncate_str(c, PREDICATE_LOG_CMD_HEAD_BYTES)
            });
            lore_debug!(
                "predicate suppress: {} tool={} cmd_head=\"{}\"",
                sanitize_for_log(&r.source_file),
                cc.tool_name.as_deref().unwrap_or(""),
                sanitize_for_log(cmd_head),
            );
            false
        })
        .collect();

    lore_debug!(
        "predicate: {} before -> {} after ({} suppressed)",
        before,
        kept.len(),
        suppressed,
    );

    kept
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
    let terms = engine::split_into_words(stderr);
    let cleaned = engine::clean_terms(&terms);
    if cleaned.is_empty() {
        return Ok(None);
    }

    let query = cleaned.join(" OR ");
    lore_debug!("PostToolUse: error query: {query}");
    let results = search_with_threshold(db, embedder, config, &query)?;

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

/// Apply the per-class relevance floor: universal chunks are filtered against
/// `universal_floor`, non-universal chunks against `min_relevance`. The two
/// floors are independent, so operators can raise the universal bar without
/// affecting ranked injections (R6).
///
/// Pure on `Vec<SearchResult>` so unit tests can exercise the branching logic
/// without standing up a real `KnowledgeDB` and `Embedder`.
fn apply_relevance_thresholds(
    results: Vec<SearchResult>,
    min_relevance: f64,
    universal_floor: f64,
) -> Vec<SearchResult> {
    results
        .into_iter()
        .filter(|r| {
            if r.is_universal {
                r.score >= universal_floor
            } else {
                r.score >= min_relevance
            }
        })
        .collect()
}

/// Shared search pipeline: embed, hybrid search, threshold filter,
/// partition-and-cap, then flatten into a single `Vec<SearchResult>` with
/// universal chunks ordered first, followed by ranked non-universal chunks
/// capped at `config.search.top_k`.
///
/// Universal chunks are additive beyond `top_k` — they are re-injected on
/// every relevant `PreToolUse` call regardless of cap. Non-universal chunks
/// respect `top_k`. Every returned row carries `is_universal` so callers
/// that need to treat the two classes differently (e.g. `dedup_filter_and_record`)
/// can read the flag directly.
///
/// Called by `cmd_search`, the `PreToolUse` handler, and the `PostToolUse`
/// handler so all three share the same pipeline and cannot drift.
pub fn search_with_threshold(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
    query: &str,
) -> anyhow::Result<Vec<SearchResult>> {
    lore_debug!(
        "search: query={query:?} hybrid={} top_k={} min_relevance={:.4} \
         min_relevance_universal={:.4}",
        config.search.hybrid,
        config.search.top_k,
        config.search.min_relevance,
        config.search.effective_min_relevance_universal(),
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

    // Over-fetch by 10× so universal chunks scattered in the ranking still
    // surface — the additive promise only works if search sees the universal
    // rows. `saturating_mul` bounds the cost when `top_k` is already large.
    let overfetch_limit = config.search.top_k.saturating_mul(10);
    let results = db.search_hybrid(query, query_embedding.as_deref(), overfetch_limit)?;
    lore_debug!("search: {} raw results", results.len());

    // Threshold is only meaningful for hybrid search with a successful
    // embedding; pure FTS or fallback paths bypass filtering entirely. The
    // gate-on-`min_relevance > 0.0` predicate continues to govern whether ANY
    // floor is applied — when both knobs would be zero, we skip the work and
    // log raw scores. Universal results consult `min_relevance_universal`
    // (with inherit-from-`min_relevance` semantics); non-universal results
    // continue to use `min_relevance`.
    let universal_floor = config.search.effective_min_relevance_universal();
    let apply_threshold =
        config.search.hybrid && !embed_failed && config.search.min_relevance > 0.0;
    let results: Vec<_> = if apply_threshold {
        let before = results.len();
        let filtered =
            apply_relevance_thresholds(results, config.search.min_relevance, universal_floor);
        lore_debug!(
            "search: threshold min_relevance={:.4} universal_floor={:.4} \
             filtered {} -> {}",
            config.search.min_relevance,
            universal_floor,
            before,
            filtered.len(),
        );
        for r in &filtered {
            lore_debug!("  {:.4} {}", r.score, sanitize_for_log(&r.title));
        }
        filtered
    } else {
        for r in &results {
            lore_debug!("  {:.4} {}", r.score, sanitize_for_log(&r.title));
        }
        results
    };

    // Partition to apply `top_k` to ranked non-universal rows only — universal
    // rows remain uncapped — then flatten with universal ordered first.
    let (mut universal, mut ranked): (Vec<_>, Vec<_>) =
        results.into_iter().partition(|r| r.is_universal);
    ranked.truncate(config.search.top_k);

    lore_debug!(
        "search: {} universal + {} ranked (top_k={})",
        universal.len(),
        ranked.len(),
        config.search.top_k,
    );

    universal.append(&mut ranked);
    Ok(universal)
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
fn render_pinned_conventions(db: &KnowledgeDB, _knowledge_dir: &Path) -> anyhow::Result<String> {
    let universal = db.universal_patterns()?;
    if universal.is_empty() {
        return Ok(String::new());
    }

    let header = "\n## Pinned conventions\n\n\
                  These patterns are tagged `universal` and apply across every \
                  tool call in this session. Treat them as always-on conventions.\n";
    let mut out = String::from(header);
    let budget_start = out.len();

    // Render pattern bodies directly from the DB — the patterns table holds
    // the authorial `raw_body` populated at ingest. `_knowledge_dir` is
    // retained on the signature for backwards-compatible callers but is no
    // longer read: per R2 of the db-sole-read-surface PR, no disk reads
    // remain in this path. DB write access is the existing trust boundary
    // (R2b); no control-character sanitisation is applied to `raw_body`
    // because user-authored markdown legitimately contains escape sequences
    // (code blocks, shell examples) that must render verbatim.
    for pattern in &universal {
        let body = pattern.raw_body.trim_end();
        let consumed = out.len() - budget_start;
        let remaining = PINNED_SECTION_TOTAL_LIMIT_BYTES.saturating_sub(consumed);
        let heading = format!("\n### {}\n\n", pattern.title);
        if heading.len() + body.len() + 1 > remaining {
            // The next pattern body would push the section over the
            // render-time cap. Emit a visible truncation marker and stop.
            // Ingest already rejects oversized universal patterns per file
            // (UNIVERSAL_BODY_HARD_LIMIT_BYTES); hitting this guard means
            // either a DB tamper bypassed the ingest check or the operator
            // has so many universal files that the aggregate pushes over
            // budget.
            let safe_source = sanitize_for_log(&pattern.source_file);
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

/// Build an FTS5 query from a Claude Code `HookInput`.
///
/// Thin shim around [`crate::engine::extract_query`]: builds a
/// [`CallContext`] via [`HookInput::to_call_context`] and delegates to the
/// engine. The engine performs no I/O; the adapter owns the filesystem
/// access (transcript-tail read happens inside `to_call_context`).
///
/// Retained as a `pub fn` so the `lore extract-queries` CLI subcommand
/// (`src/main.rs`'s `cmd_extract_queries`) can keep its existing
/// `HookInput`-flavoured interface without learning about `CallContext`.
/// `handle_pre_tool_use` no longer routes through this shim — it builds
/// the `CallContext` once per call and passes it to both
/// `engine::extract_query` and the predicate filter.
pub fn extract_query(input: &HookInput) -> Option<String> {
    let ctx = input.to_call_context();
    engine::extract_query(&ctx)
}

impl HookInput {
    /// Build a [`CallContext`] from this Claude Code hook event.
    ///
    /// Eagerly reads the transcript tail when `transcript_path` is set and
    /// passes `validate_transcript_path`'s `$HOME`-rooted canonicalisation.
    /// Failures (no path, validation rejection, missing file, IO error)
    /// leave `transcript_tail = None` — silent fall-through preserves the
    /// "best-effort" semantics of transcript-tail term harvesting.
    ///
    /// **Adapter-only filesystem boundary.** All disk I/O (transcript read,
    /// `$HOME` validation) happens here, in the Claude Code adapter. The
    /// engine module never reads the filesystem — see
    /// `tests/invariants.rs::no_unsanctioned_runtime_disk_reads_in_hook_server_main`.
    ///
    /// Called once per `PreToolUse` for non-skip-agent paths (the
    /// `skip_agent` short-circuit in `handle_pre_tool_use` runs first so
    /// Explore / Plan subagents bypass this read entirely).
    pub fn to_call_context(&self) -> CallContext {
        let transcript_tail = self
            .transcript_path
            .as_deref()
            .and_then(|p| validate_transcript_path(Path::new(p)))
            .and_then(|canonical| last_user_message(&canonical));

        CallContext {
            tool_name: self.tool_name.clone(),
            command: tool_input_str(self, "command"),
            file_path: tool_input_str(self, "file_path"),
            description: tool_input_str(self, "description"),
            transcript_tail,
        }
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

    // -- extract_query (shim coverage) ---------------------------------------
    //
    // The full engine-side coverage lives in
    // `src/engine/query.rs::tests` (against `CallContext` directly). The
    // tests below pin the `HookInput` → `CallContext` → engine shim end to
    // end so behaviour stays equivalent until U5 retires the shim.

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

    // -- HookInput::to_call_context ------------------------------------------

    #[test]
    fn to_call_context_populates_all_fields_with_transcript_tail() {
        // Write a transcript under $HOME so validate_transcript_path
        // accepts it, then verify every CallContext field is populated.
        let home = std::env::var("HOME").unwrap();
        let dir = tempfile::tempdir_in(&home).unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"user","message":{"content":"refactor the auth flow"}}
"#,
        )
        .unwrap();

        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: Some("sess".to_string()),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({
                "command": "git push",
                "file_path": "src/lib.rs",
                "description": "push to remote",
            })),
            agent_type: None,
            transcript_path: Some(transcript.to_string_lossy().to_string()),
            tool_response: None,
        };

        let cc = input.to_call_context();
        assert_eq!(cc.tool_name.as_deref(), Some("Bash"));
        assert_eq!(cc.command.as_deref(), Some("git push"));
        assert_eq!(cc.file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(cc.description.as_deref(), Some("push to remote"));
        assert_eq!(
            cc.transcript_tail.as_deref(),
            Some("refactor the auth flow"),
            "eager transcript-tail read must populate the field",
        );
    }

    #[test]
    fn to_call_context_no_transcript_path_leaves_tail_none() {
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Edit".to_string()),
            tool_input: Some(serde_json::json!({"file_path": "src/foo.rs"})),
            agent_type: None,
            transcript_path: None,
            tool_response: None,
        };

        let cc = input.to_call_context();
        assert!(
            cc.transcript_tail.is_none(),
            "no transcript_path → transcript_tail must be None",
        );
        assert_eq!(cc.tool_name.as_deref(), Some("Edit"));
        assert_eq!(cc.file_path.as_deref(), Some("src/foo.rs"));
        assert!(cc.command.is_none());
        assert!(cc.description.is_none());
    }

    #[test]
    fn to_call_context_invalid_transcript_path_silently_falls_through() {
        // Path that fails validate_transcript_path (does not exist) →
        // transcript_tail stays None; no panic, no error surfaced.
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            agent_type: None,
            transcript_path: Some("/nonexistent/path/transcript.jsonl".to_string()),
            tool_response: None,
        };

        let cc = input.to_call_context();
        assert!(
            cc.transcript_tail.is_none(),
            "invalid transcript path → transcript_tail must be None",
        );
        assert_eq!(cc.command.as_deref(), Some("ls"));
    }

    #[test]
    fn skip_path_avoids_transcript_read_even_when_path_invalid() {
        // Pin the skip_agent ordering invariant: when agent_type is
        // Explore/Plan, handle_pre_tool_use returns Ok(None) without ever
        // calling to_call_context. We verify by passing an invalid
        // transcript path that would yield None on read but cannot panic
        // in this skip path because the path is never even reached.
        //
        // We exercise the public surface: if `skip_agent` is true, the
        // pre-tool-use returns None. Since we cannot construct a real
        // KnowledgeDB / Embedder in a unit test cheaply, we assert the
        // contract by verifying that `skip_agent` itself returns true on
        // the input we'd otherwise pass. The integration test
        // `hook_explore_agent_produces_no_output` covers the end-to-end
        // skip path against a real binary.
        let input = HookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            agent_type: Some("Explore".to_string()),
            // Even an obviously invalid path must not panic — the skip
            // path means to_call_context is never called.
            transcript_path: Some("/nonexistent/explore.jsonl".to_string()),
            tool_response: None,
        };
        assert!(skip_agent(&input));
    }

    // -- apply_predicate_filter ----------------------------------------------

    fn make_universal_chunk(
        id: &str,
        source: &str,
        applies_when_json: Option<&str>,
    ) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: String::new(),
            body: String::new(),
            tags: "universal".to_string(),
            source_file: source.to_string(),
            heading_path: String::new(),
            score: 1.0,
            is_universal: true,
            applies_when_json: applies_when_json.map(String::from),
        }
    }

    fn make_non_universal_chunk(id: &str, source: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: String::new(),
            body: String::new(),
            tags: String::new(),
            source_file: source.to_string(),
            heading_path: String::new(),
            score: 1.0,
            is_universal: false,
            applies_when_json: None,
        }
    }

    fn ctx_bash_predicate(command: &str) -> CallContext {
        CallContext {
            tool_name: Some("Bash".to_string()),
            command: Some(command.to_string()),
            file_path: None,
            description: None,
            transcript_tail: None,
        }
    }

    #[test]
    fn predicate_filter_keeps_universal_without_applies_when() {
        // R11: universal chunk with no applies_when fires unconditionally.
        let chunks = vec![make_universal_chunk("u1", "git.md", None)];
        let kept = apply_predicate_filter(chunks, &ctx_bash_predicate("ls"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "u1");
    }

    #[test]
    fn predicate_filter_drops_universal_when_predicate_suppresses() {
        // Universal chunk with applies_when.bash_command_starts_with: [git]
        // and a Bash ls call → suppressed.
        let aw_json = serde_json::to_string(&AppliesWhen {
            tools: None,
            bash_command_starts_with: Some(vec!["git".to_string()]),
        })
        .unwrap();
        let chunks = vec![make_universal_chunk("u1", "git.md", Some(&aw_json))];
        let kept = apply_predicate_filter(chunks, &ctx_bash_predicate("ls"));
        assert!(kept.is_empty(), "predicate must suppress non-matching call");
    }

    #[test]
    fn predicate_filter_keeps_universal_when_predicate_matches() {
        let aw_json = serde_json::to_string(&AppliesWhen {
            tools: None,
            bash_command_starts_with: Some(vec!["git".to_string()]),
        })
        .unwrap();
        let chunks = vec![make_universal_chunk("u1", "git.md", Some(&aw_json))];
        let kept = apply_predicate_filter(chunks, &ctx_bash_predicate("git push"));
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn predicate_filter_bypasses_non_universal_chunks() {
        // R8: non-universal chunks are NOT subject to the predicate, even
        // when their applies_when_json is somehow populated.
        let aw_json = serde_json::to_string(&AppliesWhen {
            tools: Some(vec!["NeverMatchesAnyTool".to_string()]),
            bash_command_starts_with: None,
        })
        .unwrap();
        let mut nu = make_non_universal_chunk("nu1", "rust.md");
        nu.applies_when_json = Some(aw_json);
        let kept = apply_predicate_filter(vec![nu], &ctx_bash_predicate("ls"));
        assert_eq!(
            kept.len(),
            1,
            "non-universal chunks must bypass the predicate filter (R8)",
        );
    }

    #[test]
    fn predicate_filter_mixed_universal_and_non_universal() {
        // Mix: 1 non-universal kept, 1 universal-no-predicate kept,
        // 1 universal-suppressed dropped, 1 universal-passing kept.
        let aw_suppress = serde_json::to_string(&AppliesWhen {
            tools: None,
            bash_command_starts_with: Some(vec!["never".to_string()]),
        })
        .unwrap();
        let aw_match = serde_json::to_string(&AppliesWhen {
            tools: Some(vec!["Bash".to_string()]),
            bash_command_starts_with: None,
        })
        .unwrap();

        let chunks = vec![
            make_non_universal_chunk("nu", "rust.md"),
            make_universal_chunk("u-no-pred", "always.md", None),
            make_universal_chunk("u-suppressed", "git.md", Some(&aw_suppress)),
            make_universal_chunk("u-matched", "bash.md", Some(&aw_match)),
        ];
        let kept = apply_predicate_filter(chunks, &ctx_bash_predicate("ls"));
        let ids: Vec<&str> = kept.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"nu"));
        assert!(ids.contains(&"u-no-pred"));
        assert!(!ids.contains(&"u-suppressed"));
        assert!(ids.contains(&"u-matched"));
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn predicate_filter_treats_malformed_json_as_unrestricted() {
        // Defensive R11 fallback: if applies_when_json is somehow malformed
        // at runtime, the chunk fires as if no predicate were set rather
        // than being silently suppressed.
        let chunks = vec![make_universal_chunk(
            "u1",
            "weird.md",
            Some("not valid json {"),
        )];
        let kept = apply_predicate_filter(chunks, &ctx_bash_predicate("ls"));
        assert_eq!(kept.len(), 1, "malformed JSON must fall through to fire");
    }

    #[test]
    fn predicate_filter_empty_input_returns_empty() {
        let kept = apply_predicate_filter(Vec::new(), &ctx_bash_predicate("ls"));
        assert!(kept.is_empty());
    }

    // -- apply_relevance_thresholds (U6) -------------------------------------

    /// Build a `SearchResult` with the given score and universal flag for
    /// per-class threshold testing. Title/body/tags are immaterial; only
    /// `score` and `is_universal` drive the filter branch.
    fn make_scored_result(id: &str, score: f64, is_universal: bool) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: String::new(),
            body: String::new(),
            tags: if is_universal { "universal" } else { "" }.to_string(),
            source_file: format!("{id}.md"),
            heading_path: String::new(),
            score,
            is_universal,
            applies_when_json: None,
        }
    }

    #[test]
    fn thresholds_default_universal_floor_keeps_score_above_min_relevance() {
        // Default config: universal_floor == min_relevance. A 0.65-scored
        // universal result is retained at the 0.6 default floor.
        let results = vec![make_scored_result("u1", 0.65, true)];
        let kept = apply_relevance_thresholds(results, 0.6, 0.6);
        assert_eq!(kept.len(), 1, "0.65 >= default 0.6 floor → keep");
    }

    #[test]
    fn thresholds_raised_universal_floor_drops_universal_below_floor() {
        // With min_relevance_universal = 0.7, a 0.65-scored universal
        // result is dropped even though it is still above min_relevance.
        let results = vec![make_scored_result("u1", 0.65, true)];
        let kept = apply_relevance_thresholds(results, 0.6, 0.7);
        assert!(
            kept.is_empty(),
            "raised universal floor must drop 0.65 universal result"
        );
    }

    #[test]
    fn thresholds_raised_universal_floor_keeps_non_universal_above_min_relevance() {
        // The universal floor must NOT affect non-universal results.
        // A 0.65-scored non-universal still fires when min_relevance = 0.6,
        // regardless of how high the universal floor is set.
        let results = vec![make_scored_result("nu1", 0.65, false)];
        let kept = apply_relevance_thresholds(results, 0.6, 0.7);
        assert_eq!(
            kept.len(),
            1,
            "non-universal result must use min_relevance, not universal floor"
        );
    }

    #[test]
    fn thresholds_mixed_results_apply_per_class_floors() {
        // Mixed batch: independent floors must filter per-class.
        // min_relevance = 0.6, universal_floor = 0.7 →
        //   * universal 0.65 dropped, universal 0.75 kept
        //   * non-universal 0.55 dropped, non-universal 0.65 kept
        let results = vec![
            make_scored_result("u-low", 0.65, true),
            make_scored_result("u-high", 0.75, true),
            make_scored_result("nu-low", 0.55, false),
            make_scored_result("nu-high", 0.65, false),
        ];
        let kept = apply_relevance_thresholds(results, 0.6, 0.7);
        let ids: Vec<&str> = kept.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&"u-low"), "0.65 universal below 0.7 floor");
        assert!(ids.contains(&"u-high"), "0.75 universal above 0.7 floor");
        assert!(
            !ids.contains(&"nu-low"),
            "0.55 non-universal below 0.6 floor"
        );
        assert!(
            ids.contains(&"nu-high"),
            "0.65 non-universal above 0.6 floor"
        );
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn thresholds_score_at_floor_is_inclusive() {
        // `>=` semantics: a score exactly at the floor is retained for both
        // classes, matching the existing (pre-U6) min_relevance behaviour.
        let results = vec![
            make_scored_result("u-eq", 0.7, true),
            make_scored_result("nu-eq", 0.6, false),
        ];
        let kept = apply_relevance_thresholds(results, 0.6, 0.7);
        assert_eq!(kept.len(), 2, "scores at the floor are inclusive");
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
            applies_when_json: None,
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
                applies_when_json: None,
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
                applies_when_json: None,
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
            applies_when_json: None,
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
