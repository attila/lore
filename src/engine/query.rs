//! FTS5 query extraction from a [`CallContext`].
//!
//! The agent-agnostic counterpart of `src/hook.rs`'s former `extract_query`.
//! Pulls term candidates out of file paths, Bash descriptions/commands, and
//! the transcript-tail snippet (already populated by the adapter — the
//! engine performs no I/O), runs them through filename-aware splitting and
//! `clean_terms` cleanup, and assembles a small FTS5 query string of the
//! shape `language AND (term1 OR term2 OR ...)` (with sensible fallbacks
//! when only one of the two parts survives).
//!
//! All public functions here are total: any `&CallContext` / `&str` in,
//! deterministic output, no panics, no I/O. The Claude Code adapter is the
//! only caller that converts a `HookInput` into a `CallContext`; future
//! adapters call this module directly with their own `CallContext`.

use std::path::Path;

use crate::engine::call_context::CallContext;
use crate::engine::text::{split_into_words, truncate_str};

/// Tool-name string the Bash branch keys off.
const TOOL_BASH: &str = "Bash";

/// Maximum transcript-tail bytes consumed for term harvesting.
///
/// The adapter populates `transcript_tail` with the trailing 32 KB of the
/// user's transcript (see `src/hook.rs`'s `last_user_message`); this cap
/// further bounds the slice we feed into `split_into_words` when building
/// the FTS query, so a long final user message contributes a finite number
/// of candidate terms.
const TRANSCRIPT_TERM_BUDGET: usize = 200;

/// Stop-list for `clean_terms`: short, high-frequency, low-information
/// English words that would otherwise dominate the OR clause.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "that", "this", "then", "when", "will", "has",
    "have", "was", "are", "not", "but", "can", "all", "its", "our", "use", "new", "let", "set",
    "get", "add", "run", "see", "how", "may", "per", "via", "yet", "also", "just", "some", "been",
    "were", "what", "they", "each", "which", "their", "there", "about", "would", "could", "should",
    "these", "those", "other", "than", "them", "your", "does", "here",
];

/// Build an FTS5 query from a [`CallContext`].
///
/// Returns `None` when no meaningful terms can be extracted. The shape is
/// one of:
///
/// * `"<lang> AND (term1 OR term2 OR ...)"` — language anchor plus
///   enrichment terms.
/// * `"<lang>"` — language anchor only (no enrichment terms survived
///   cleaning).
/// * `"term1 OR term2 OR ..."` — enrichment terms only (no language).
/// * `None` — neither a language anchor nor any cleaned terms.
///
/// Reads from the context fields directly and never touches the
/// filesystem; the adapter is responsible for populating
/// `transcript_tail` (eager 32 KB read with `$HOME` validation) before
/// calling.
pub fn extract_query(ctx: &CallContext) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut language: Option<String> = None;

    // 1. File path signals (Edit, Write, Read, etc.)
    if let Some(file_path) = ctx.file_path.as_deref() {
        if let Some(lang) = language_from_extension(file_path) {
            language = Some(lang);
        }
        terms.extend(filename_terms(file_path));
    }

    // 2. Bash signals — prefer description, fall back to command.
    if ctx.tool_name.as_deref() == Some(TOOL_BASH) {
        let text = ctx
            .description
            .as_deref()
            .or(ctx.command.as_deref())
            .unwrap_or_default();

        if language.is_none() {
            language = language_from_bash(text);
        }

        terms.extend(split_into_words(text));
    }

    // 3. Transcript tail (last user message). Populated by the adapter; we
    //    just slice the budget and harvest words.
    if let Some(transcript_tail) = ctx.transcript_tail.as_deref() {
        let truncated = truncate_str(transcript_tail, TRANSCRIPT_TERM_BUDGET);
        terms.extend(split_into_words(truncated));
    }

    // 4. Clean terms.
    let cleaned = clean_terms(&terms);

    // 5. Assemble FTS5 query.
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

/// Map file extension to a language keyword for FTS anchor.
pub fn language_from_extension(path: &str) -> Option<String> {
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

/// Infer language from a Bash command string (substring match — not a
/// shell-aware tokeniser; happy with `cargo build` or `npm run foo`).
pub fn language_from_bash(command: &str) -> Option<String> {
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
pub fn filename_terms(path: &str) -> Vec<String> {
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
///
/// Treats uppercase letters mid-string as boundaries, lowercase the
/// resulting fragments, and split on non-alphanumeric runs (underscores,
/// hyphens, dots, etc.).
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

/// Clean terms: strip non-alpha, filter short, filter hex-like, filter
/// stop words, deduplicate while preserving first-seen order.
pub fn clean_terms(raw: &[String]) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- extract_query -------------------------------------------------------

    #[test]
    fn extract_query_rs_file_path() {
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("src/validate_email.rs".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).unwrap();
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
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("src/components/UserProfile.tsx".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).unwrap();
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
    fn extract_query_bash_with_cargo_description() {
        let ctx = CallContext {
            tool_name: Some("Bash".to_string()),
            description: Some("Run cargo test for error handling".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).unwrap();
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
        // No description, command-only path falls back to the command for
        // term harvesting.
        let ctx = CallContext {
            tool_name: Some("Bash".to_string()),
            command: Some("npm test authentication".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).unwrap();
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
    fn extract_query_bash_cargo_build_command() {
        // Plan U4 explicit case: `cargo build` produces a `rust AND (cargo
        // OR build)` style query (the language anchor lands the AND clause,
        // and the cleaned `cargo` / `build` terms populate the OR side).
        let ctx = CallContext {
            tool_name: Some("Bash".to_string()),
            command: Some("cargo build".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).unwrap();
        assert!(
            query.starts_with("rust AND ("),
            "should anchor on rust with AND clause: {query}"
        );
        assert!(query.contains("cargo"), "should retain cargo term: {query}");
        assert!(query.contains("build"), "should retain build term: {query}");
    }

    #[test]
    fn extract_query_edit_rust_file_yields_language_and_filename_terms() {
        // Plan U4 explicit case: Edit on src/foo.rs produces a query with
        // a rust anchor (cleaning drops the 3-char filename `foo`, so the
        // OR clause may be language-only — we just assert the anchor
        // survives).
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("src/foo.rs".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).unwrap();
        assert!(
            query.contains("rust"),
            "should anchor rust language: {query}"
        );
    }

    #[test]
    fn extract_query_no_signals_returns_none() {
        // .txt has no language anchor, and "a" is too short after cleaning.
        let ctx = CallContext {
            tool_name: Some("Read".to_string()),
            file_path: Some("a.txt".to_string()),
            ..CallContext::empty()
        };
        assert!(extract_query(&ctx).is_none());
    }

    #[test]
    fn extract_query_all_none_returns_none() {
        // Plan U4 explicit case: a CallContext with all-None fields yields
        // `None` — no language anchor, no terms.
        let ctx = CallContext::empty();
        assert!(extract_query(&ctx).is_none());
    }

    #[test]
    fn extract_query_transcript_tail_only_yields_terms() {
        // Plan U4 explicit case: only `transcript_tail` populated. The
        // engine harvests terms from it (after the 200-byte truncate),
        // producing an OR-only query with no language anchor.
        let ctx = CallContext {
            transcript_tail: Some("debug the authentication bug in the login flow".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query(&ctx).expect("transcript-only query should not be None");
        // No language anchor — pure OR clause.
        assert!(
            !query.contains(" AND "),
            "transcript-only query should have no AND anchor: {query}"
        );
        assert!(
            query.contains("authentication"),
            "should harvest a term from the transcript tail: {query}"
        );
        assert!(
            query.contains("login") || query.contains("debug"),
            "should harvest other terms from the transcript tail: {query}"
        );
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
}
