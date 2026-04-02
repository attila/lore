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
#[derive(Debug, Serialize)]
pub struct HookOutput {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookSpecificOutput,
}

/// The payload nested inside `HookOutput`.
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
    match input.hook_event_name.as_str() {
        "SessionStart" => handle_session_start(input, db),
        "PreToolUse" => handle_pre_tool_use(input, db, embedder, config),
        "PostToolUse" => handle_post_tool_use(input, db, embedder, config),
        "PostCompact" => handle_post_compact(input, db),
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

/// Handle `SessionStart`: create dedup file, return meta-instruction + pattern index.
fn handle_session_start(input: &HookInput, db: &KnowledgeDB) -> anyhow::Result<Option<HookOutput>> {
    let dedup_path = session_dedup_path(input);
    if let Some(ref path) = dedup_path
        && let Err(e) = create_dedup(path)
    {
        eprintln!("lore hook: failed to create dedup file: {e}");
    }

    let context = format_session_context(db)?;
    Ok(Some(HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "SessionStart".to_string(),
            additional_context: context,
        },
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
        return Ok(None);
    }

    let Some(query) = extract_query(input) else {
        return Ok(None);
    };

    let results = search_with_threshold(db, embedder, config, &query)?;

    if results.is_empty() {
        return Ok(None);
    }

    // Dedup: filter out already-injected chunk IDs for this session.
    let dedup_path = session_dedup_path(input);
    let (results, dedup_ok) = if let Some(ref path) = dedup_path {
        let seen = read_dedup(path);
        let filtered: Vec<SearchResult> = results
            .into_iter()
            .filter(|r| !seen.contains(&r.id))
            .collect();
        // Track whether dedup read succeeded (non-empty seen set or file
        // exists). We consider dedup "ok" if the file is readable; an empty
        // set from a missing file means we should still write.
        (filtered, true)
    } else {
        (results, false)
    };

    if results.is_empty() {
        return Ok(None);
    }

    let context = format_imperative(&results);

    // Append newly injected chunk IDs to dedup file.
    if dedup_ok && let Some(ref path) = dedup_path {
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        if let Err(e) = write_dedup(path, &ids) {
            eprintln!("lore hook: failed to update dedup file: {e}");
        }
    }

    Ok(Some(HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PreToolUse".to_string(),
            additional_context: context,
        },
    }))
}

/// Handle `PostCompact`: truncate dedup, re-emit `SessionStart` content.
fn handle_post_compact(input: &HookInput, db: &KnowledgeDB) -> anyhow::Result<Option<HookOutput>> {
    let dedup_path = session_dedup_path(input);
    if let Some(ref path) = dedup_path
        && let Err(e) = truncate_dedup(path)
    {
        eprintln!("lore hook: failed to truncate dedup file: {e}");
    }

    let context = format_session_context(db)?;
    Ok(Some(HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PostCompact".to_string(),
            additional_context: context,
        },
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
        return Ok(None);
    }

    // Use stderr as a search query (clean it into terms).
    let terms = split_into_words(stderr);
    let cleaned = clean_terms(&terms);
    if cleaned.is_empty() {
        return Ok(None);
    }

    let query = cleaned.join(" OR ");
    let results = search_with_threshold(db, embedder, config, &query)?;

    if results.is_empty() {
        return Ok(None);
    }

    let context = format_imperative(&results);
    Ok(Some(HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PostToolUse".to_string(),
            additional_context: context,
        },
    }))
}

/// Shared search pipeline: embed, hybrid search, threshold filter.
///
/// Extracted so that `cmd_search` and the hook handler both call the same
/// function, avoiding drift between the two code paths.
pub fn search_with_threshold(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
    query: &str,
) -> anyhow::Result<Vec<SearchResult>> {
    let mut embed_failed = false;

    let query_embedding = if config.search.hybrid {
        match embedder.embed(query) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Warning: Ollama unreachable ({e}), falling back to text search.");
                embed_failed = true;
                None
            }
        }
    } else {
        None
    };

    let results = db.search_hybrid(query, query_embedding.as_deref(), config.search.top_k)?;

    let apply_threshold =
        config.search.hybrid && !embed_failed && config.search.min_relevance > 0.0;
    let results: Vec<_> = if apply_threshold {
        results
            .into_iter()
            .filter(|r| r.score >= config.search.min_relevance)
            .collect()
    } else {
        results
    };

    Ok(results)
}

// ---------------------------------------------------------------------------
// Session context formatting
// ---------------------------------------------------------------------------

/// Format the meta-instruction + compact pattern index returned at session
/// start and after compaction.
fn format_session_context(db: &KnowledgeDB) -> anyhow::Result<String> {
    let patterns = db.list_patterns()?;

    let mut out = String::from(
        "This project uses lore for coding conventions. \
         Relevant patterns are injected automatically via additionalContext \
         before your edits. Treat all 'REQUIRED CONVENTIONS' blocks as \
         binding constraints, not suggestions.\n\n\
         Available patterns:\n",
    );

    for p in &patterns {
        if p.tags.is_empty() {
            let _ = writeln!(out, "- {}", p.title);
        } else {
            let _ = writeln!(out, "- {} [{}]", p.title, p.tags);
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
/// Sanitizes the session ID for filename safety by replacing
/// non-alphanumeric characters with `-`.
pub fn dedup_file_path(session_id: &str) -> PathBuf {
    let sanitized: String = session_id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    std::env::temp_dir().join(format!("lore-session-{sanitized}"))
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

/// Truncate the dedup file (clear all tracked IDs).
pub fn truncate_dedup(path: &Path) -> anyhow::Result<()> {
    std::fs::write(path, "")?;
    Ok(())
}

/// Create or truncate the dedup file.
pub fn create_dedup(path: &Path) -> anyhow::Result<()> {
    std::fs::write(path, "")?;
    Ok(())
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

    // 3. Transcript tail (last user message)
    if let Some(ref path) = input.transcript_path
        && let Some(msg) = last_user_message(Path::new(path))
    {
        let truncated = truncate_str(&msg, 200);
        terms.extend(split_into_words(truncated));
    }

    // 4. Clean terms
    let cleaned = clean_terms(&terms);

    if cleaned.is_empty() {
        return None;
    }

    // 5. Assemble FTS5 query
    let or_clause = cleaned.join(" OR ");
    Some(if let Some(lang) = language {
        format!("{lang} AND ({or_clause})")
    } else {
        or_clause
    })
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

/// Read the transcript JSONL file in reverse to find the last user message.
fn last_user_message(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;

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

/// Truncate a string to at most `max_chars` characters (on a char boundary).
fn truncate_str(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    // Find the largest byte offset that is both <= max_chars bytes and a
    // valid char boundary.
    let mut end = max_chars;
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
        let _ = writeln!(out, "REQUIRED CONVENTIONS (source: {source})");
        out.push_str("Follow these rules when writing this code:\n\n");

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
        }];

        let formatted = format_imperative(&results);
        assert!(formatted.contains("REQUIRED CONVENTIONS (source: errors.md)"));
        assert!(formatted.contains("Follow these rules"));
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
            },
            SearchResult {
                id: "c2".into(),
                title: "Naming".into(),
                body: "Use snake_case.".into(),
                tags: String::new(),
                source_file: "naming.md".into(),
                heading_path: String::new(),
                score: 0.7,
            },
        ];

        let formatted = format_imperative(&results);
        assert!(formatted.contains("REQUIRED CONVENTIONS (source: errors.md)"));
        assert!(formatted.contains("REQUIRED CONVENTIONS (source: naming.md)"));
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
        assert!(last_user_message(Path::new("/tmp/nonexistent-transcript.jsonl")).is_none());
    }

    // -- dedup_file_path ------------------------------------------------------

    #[test]
    fn dedup_file_path_sanitizes_uuid() {
        let path = dedup_file_path("60de87ba-e944-42c0-91f5-3cd3c38938de");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            filename,
            "lore-session-60de87ba-e944-42c0-91f5-3cd3c38938de"
        );
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn dedup_file_path_sanitizes_special_chars() {
        let path = dedup_file_path("bad/session\\.id:with*special");
        let filename = path.file_name().unwrap().to_str().unwrap();
        // All non-alphanumeric chars should be replaced with '-'.
        assert!(
            !filename.contains('/'),
            "should not contain slashes: {filename}"
        );
        assert!(
            !filename.contains('\\'),
            "should not contain backslashes: {filename}"
        );
        assert!(
            !filename.contains(':'),
            "should not contain colons: {filename}"
        );
        assert!(filename.starts_with("lore-session-"));
    }

    // -- read_dedup / write_dedup / truncate_dedup ----------------------------

    #[test]
    fn read_dedup_missing_file_returns_empty() {
        let set = read_dedup(Path::new("/tmp/lore-nonexistent-dedup-file"));
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
    fn truncate_dedup_clears_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1", "c2"]).unwrap();
        truncate_dedup(&path).unwrap();
        let ids = read_dedup(&path);
        assert!(ids.is_empty(), "should be empty after truncation");
    }

    #[test]
    fn create_dedup_creates_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        create_dedup(&path).unwrap();
        assert!(path.exists());
        let ids = read_dedup(&path);
        assert!(ids.is_empty());
    }

    #[test]
    fn create_dedup_truncates_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup");

        write_dedup(&path, &["c1"]).unwrap();
        create_dedup(&path).unwrap();
        let ids = read_dedup(&path);
        assert!(ids.is_empty(), "create should truncate existing content");
    }
}
