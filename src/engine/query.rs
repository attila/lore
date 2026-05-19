//! FTS5 query extraction from a [`CallContext`].
//!
//! The agent-agnostic counterpart of `src/hook.rs`'s former `extract_query`.
//! Pulls term candidates out of file paths, Bash descriptions/commands, and
//! the transcript-tail snippet (already populated by the adapter — the
//! engine performs no I/O), runs them through filename-aware splitting and
//! `clean_terms` cleanup, and assembles a small FTS5 query string of the
//! shape `language AND (term1 OR term2 OR ...)` (with sensible fallbacks
//! when only one of the two parts survives, and `(lang1 OR lang2)` for
//! multi-language inferences such as `npm test` matching both
//! `javascript` and `typescript`).
//!
//! Language detection iterates [`crate::engine::languages::LANGUAGES`] —
//! the shared declarative table — over four signal types: file
//! extensions, marker filenames, directory-hint path components, and
//! Bash command-line tokens. The four signal helpers all return
//! `Vec<String>` because a single signal may legitimately fire for
//! multiple languages (e.g. `package.json` belongs to both
//! `javascript` and `typescript`). [`extract_query`] composes them with
//! a marker > extension > directory-hint priority chain on the file
//! path side and unions in the Bash side.
//!
//! All public functions here are total: any `&CallContext` / `&str` in,
//! deterministic output, no panics, no I/O. The Claude Code adapter is the
//! only caller that converts a `HookInput` into a `CallContext`; future
//! adapters call this module directly with their own `CallContext`.

use std::path::{Component, Path};

use crate::engine::call_context::CallContext;
use crate::engine::languages::{
    languages_for_command_keyword, languages_for_directory_hint, languages_for_extension,
    languages_for_marker_filename,
};
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

/// Decompose a [`CallContext`] into the two ingredients retrieval needs:
/// the set of inferred languages and the list of cleaned enrichment
/// terms. Returns `None` when neither side produced anything useful.
///
/// The first slot is the inferred-language set (empty `Vec` when the
/// context has no recognised file-path or bash signal, singular when
/// one entry matches, multi-valued when shared signals fire such as
/// `npm test` accumulating `{javascript, typescript}`). The second slot
/// is the cleaned, deduplicated enrichment-term list produced by
/// [`clean_terms`].
///
/// Reads from the context fields directly and never touches the
/// filesystem; the adapter is responsible for populating
/// `transcript_tail` (eager 32 KB read with `$HOME` validation) before
/// calling.
///
/// Use [`assemble_fts_query`] when you need the legacy FTS5 string
/// shape (e.g. CLI `lore extract-queries` output, hook-shim
/// backwards compatibility); retrieval callers pass the two `Vec`s
/// straight to `KnowledgeDB::search_hybrid` so the structural gate
/// can apply the membership predicate on `language_json`.
pub fn extract_query(ctx: &CallContext) -> Option<(Vec<String>, Vec<String>)> {
    let inferred = infer_languages(ctx);
    let terms = harvest_terms(ctx);
    let cleaned = clean_terms(&terms);
    if inferred.is_empty() && cleaned.is_empty() {
        None
    } else {
        Some((inferred, cleaned))
    }
}

/// Assemble the legacy FTS5 query string from an inferred-language set
/// and a cleaned enrichment-term list. Useful for callers that still
/// speak the pre-U5 query-string contract (CLI `lore extract-queries`,
/// the public hook-shim signature).
///
/// Returned shape:
///
/// * `Some("<lang> AND (term1 OR term2 OR ...)")` — single language
///   anchor plus enrichment terms.
/// * `Some("(<lang1> OR <lang2>) AND (term1 OR term2 OR ...)")` —
///   multi-language anchor (e.g. `npm test` infers both `javascript`
///   and `typescript`).
/// * `Some("<lang>")` / `Some("(<lang1> OR <lang2>)")` — language
///   anchor only (no enrichment terms).
/// * `Some("term1 OR term2 OR ...")` — enrichment terms only.
/// * `None` — both sides are empty.
pub fn assemble_fts_query(inferred: &[String], cleaned: &[String]) -> Option<String> {
    assemble_query(inferred, cleaned)
}

/// Returns the set of inferred languages for a [`CallContext`].
///
/// File-path signals chain by priority — marker filename > extension >
/// directory hint — taking the first signal that produces any match.
/// Bash command signals union with the file-path result (they
/// contribute independently when both fire). The returned `Vec` is
/// deduplicated and ordered first-seen.
///
/// Empty `Vec` means no signal matched; downstream callers must treat
/// this as the no-language case (terms-only retrieval per R11).
pub fn infer_languages(ctx: &CallContext) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Some(file_path) = ctx.file_path.as_deref() {
        // Priority chain (R7): first non-empty result wins.
        let from_path = language_from_marker_filename(file_path);
        let from_path = if from_path.is_empty() {
            language_from_extension(file_path)
        } else {
            from_path
        };
        let from_path = if from_path.is_empty() {
            language_from_directory_hint(file_path)
        } else {
            from_path
        };
        extend_unique(&mut out, from_path);
    }

    if ctx.tool_name.as_deref() == Some(TOOL_BASH) {
        let text = ctx
            .description
            .as_deref()
            .or(ctx.command.as_deref())
            .unwrap_or_default();
        extend_unique(&mut out, language_from_bash(text));
    }

    out
}

/// Push every element of `incoming` into `out` while preserving
/// first-seen order and dropping duplicates against the existing
/// contents of `out`.
fn extend_unique(out: &mut Vec<String>, incoming: Vec<String>) {
    for lang in incoming {
        if !out.iter().any(|existing| existing == &lang) {
            out.push(lang);
        }
    }
}

/// Harvest raw term candidates from the context's file path, Bash
/// description/command, and transcript-tail snippet. The result still
/// contains stop words, hex-like fragments, and duplicates — call
/// [`clean_terms`] before assembling a query.
fn harvest_terms(ctx: &CallContext) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();

    if let Some(file_path) = ctx.file_path.as_deref() {
        terms.extend(filename_terms(file_path));
    }

    if ctx.tool_name.as_deref() == Some(TOOL_BASH) {
        let text = ctx
            .description
            .as_deref()
            .or(ctx.command.as_deref())
            .unwrap_or_default();
        terms.extend(split_into_words(text));
    }

    if let Some(transcript_tail) = ctx.transcript_tail.as_deref() {
        let truncated = truncate_str(transcript_tail, TRANSCRIPT_TERM_BUDGET);
        terms.extend(split_into_words(truncated));
    }

    terms
}

/// Assemble the final FTS5 query string from the inferred-language set
/// and the cleaned enrichment terms. See [`extract_query`] for the
/// returned shapes.
fn assemble_query(inferred: &[String], cleaned: &[String]) -> Option<String> {
    let lang_part = if inferred.is_empty() {
        None
    } else if inferred.len() == 1 {
        Some(inferred[0].clone())
    } else {
        Some(format!("({})", inferred.join(" OR ")))
    };

    match (lang_part, cleaned.is_empty()) {
        (Some(lang), false) => {
            let or_clause = cleaned.join(" OR ");
            Some(format!("{lang} AND ({or_clause})"))
        }
        (Some(lang), true) => Some(lang),
        (None, false) => Some(cleaned.join(" OR ")),
        (None, true) => None,
    }
}

/// Map a file path's extension to the set of languages whose
/// `extensions` slice contains it. Returns an empty `Vec` when the
/// path has no extension or the extension is not registered.
pub fn language_from_extension(path: &str) -> Vec<String> {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(languages_for_extension)
        .unwrap_or_default()
}

/// Map a file path's basename to the set of languages whose
/// `marker_filenames` slice contains it (e.g. `Cargo.toml` → `[rust]`,
/// `package.json` → `[typescript, javascript]`). Marker matching is
/// case-sensitive because the conventional casing is part of the marker.
/// Returns an empty `Vec` when the basename is not a known marker.
pub fn language_from_marker_filename(path: &str) -> Vec<String> {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(languages_for_marker_filename)
        .unwrap_or_default()
}

/// Map a file path's directory components to the set of languages whose
/// `directory_hints` slice contains any of them (e.g.
/// `node_modules/foo/bar.js` → `[typescript, javascript]`). Components
/// are read in path order; matches across multiple components union.
/// Returns an empty `Vec` when no component matches a known hint.
pub fn language_from_directory_hint(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for component in Path::new(path).components() {
        if let Component::Normal(os) = component
            && let Some(name) = os.to_str()
        {
            extend_unique(&mut out, languages_for_directory_hint(name));
        }
    }
    out
}

/// Infer the set of languages implied by a Bash command string via
/// whole-token matching. Splits `command` on whitespace, lowercases
/// each token, filters out `KEY=VAL`-shaped env-prefix tokens, and
/// collects the canonical tokens of every entry whose
/// `command_keywords` slice contains any surviving token.
///
/// Whole-token matching (not substring) is load-bearing — `bundle
/// install` no longer matches `bun`, and `env FOO=bar cargo build`
/// yields `[rust]` despite the env-prefix preceding `cargo`. Multiple
/// languages may match the same token (`npm` registers in both
/// `javascript` and `typescript`); the returned `Vec` deduplicates
/// while preserving first-seen order.
pub fn language_from_bash(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in command.split_whitespace() {
        if is_env_assignment(raw) {
            continue;
        }
        let lower = raw.to_lowercase();
        extend_unique(&mut out, languages_for_command_keyword(&lower));
    }
    out
}

/// Returns `true` if `token` looks like a `KEY=VAL` env-prefix
/// assignment (left-hand side matches `[A-Z_][A-Z0-9_]*`).
///
/// Matches POSIX env-variable naming so well-formed prefixes such as
/// `FOO=bar` and `PATH=/usr/bin` are filtered out of the bash-token
/// scan, while non-assignment tokens (`npm`, `cargo`, `bundle`) and
/// malformed assignments (`1=foo`) are passed through to the keyword
/// match.
fn is_env_assignment(token: &str) -> bool {
    let Some((lhs, _)) = token.split_once('=') else {
        return false;
    };
    if lhs.is_empty() {
        return false;
    }
    let mut chars = lhs.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
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

    /// Test helper that re-assembles the legacy FTS5 query string from
    /// [`extract_query`]'s tuple shape. Lets the bulk of the existing
    /// assertions on the query string survive the U5 return-type
    /// refactor without per-test rewrites; tests that need the
    /// inferred-language set directly call `extract_query` and
    /// destructure.
    fn extract_query_str(ctx: &CallContext) -> Option<String> {
        let (langs, terms) = extract_query(ctx)?;
        assemble_fts_query(&langs, &terms)
    }

    // -- extract_query -------------------------------------------------------

    #[test]
    fn extract_query_rs_file_path() {
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("src/validate_email.rs".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query_str(&ctx).unwrap();
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

        let query = extract_query_str(&ctx).unwrap();
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

        let query = extract_query_str(&ctx).unwrap();
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

        let query = extract_query_str(&ctx).unwrap();
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

        let query = extract_query_str(&ctx).unwrap();
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

        let query = extract_query_str(&ctx).unwrap();
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
        assert!(extract_query_str(&ctx).is_none());
    }

    #[test]
    fn extract_query_all_none_returns_none() {
        // Plan U4 explicit case: a CallContext with all-None fields yields
        // `None` — no language anchor, no terms.
        let ctx = CallContext::empty();
        assert!(extract_query_str(&ctx).is_none());
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

        let query = extract_query_str(&ctx).expect("transcript-only query should not be None");
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
        assert_eq!(language_from_extension("src/main.rs"), vec!["rust"]);
    }

    #[test]
    fn language_extension_tsx() {
        assert_eq!(language_from_extension("App.tsx"), vec!["typescript"]);
    }

    #[test]
    fn language_extension_jsx() {
        assert_eq!(language_from_extension("App.jsx"), vec!["javascript"]);
    }

    #[test]
    fn language_extension_yaml() {
        assert_eq!(language_from_extension("ci.yml"), vec!["yaml"]);
        assert_eq!(language_from_extension("ci.yaml"), vec!["yaml"]);
    }

    #[test]
    fn language_extension_py() {
        assert_eq!(language_from_extension("script.py"), vec!["python"]);
    }

    #[test]
    fn language_extension_go() {
        assert_eq!(language_from_extension("main.go"), vec!["golang"]);
    }

    #[test]
    fn language_extension_unknown() {
        assert!(language_from_extension("notes.txt").is_empty());
    }

    #[test]
    fn language_extension_no_extension_returns_empty() {
        assert!(language_from_extension("README").is_empty());
    }

    // -- language_from_marker_filename ---------------------------------------

    #[test]
    fn marker_cargo_toml_anywhere_in_path_detects_rust() {
        assert_eq!(
            language_from_marker_filename("project/Cargo.toml"),
            vec!["rust"]
        );
    }

    #[test]
    fn marker_package_json_detects_both_js_and_ts() {
        let langs = language_from_marker_filename("frontend/package.json");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn marker_unknown_filename_returns_empty() {
        assert!(language_from_marker_filename("src/main.rs").is_empty());
    }

    // -- language_from_directory_hint ---------------------------------------

    #[test]
    fn directory_node_modules_detects_both_js_and_ts() {
        let langs = language_from_directory_hint("node_modules/foo/anything");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn directory_pycache_detects_python() {
        assert_eq!(
            language_from_directory_hint("src/__pycache__/foo.pyc"),
            vec!["python"]
        );
    }

    #[test]
    fn directory_no_hint_returns_empty() {
        assert!(language_from_directory_hint("src/lib/foo").is_empty());
    }

    // -- language_from_bash --------------------------------------------------

    #[test]
    fn bash_npm_yields_javascript_and_typescript() {
        let langs = language_from_bash("npm install express");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn bash_cargo_is_rust() {
        assert_eq!(language_from_bash("cargo build --release"), vec!["rust"]);
    }

    #[test]
    fn bash_pip_is_python() {
        assert_eq!(language_from_bash("pip install requests"), vec!["python"]);
    }

    #[test]
    fn bash_python_is_python() {
        assert_eq!(language_from_bash("python script.py"), vec!["python"]);
    }

    #[test]
    fn bash_python3_is_python() {
        assert_eq!(language_from_bash("python3 -m venv .venv"), vec!["python"]);
    }

    #[test]
    fn bash_go_is_golang() {
        assert_eq!(language_from_bash("go build ./..."), vec!["golang"]);
    }

    #[test]
    fn bash_unknown_is_empty() {
        assert!(language_from_bash("ls -la").is_empty());
    }

    #[test]
    fn bash_bundle_install_does_not_match_bun() {
        // R2 regression test: substring matcher would have matched
        // "bun" inside "bundle". Whole-token matcher must not — the
        // command resolves to `ruby` (which owns `bundle`) and never to
        // `bun`/javascript/typescript.
        let langs = language_from_bash("bundle install");
        assert_eq!(langs, vec!["ruby".to_string()]);
        assert!(!langs.contains(&"javascript".to_string()));
        assert!(!langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn bash_env_prefix_does_not_block_keyword() {
        // `env FOO=bar cargo build` should still detect rust — the
        // env-prefix tokens are filtered before keyword matching.
        assert_eq!(language_from_bash("env FOO=bar cargo build"), vec!["rust"]);
    }

    #[test]
    fn bash_multiple_env_assignments_filtered() {
        assert_eq!(
            language_from_bash("PATH=/usr/local/bin RUST_LOG=debug cargo test"),
            vec!["rust"]
        );
    }

    #[test]
    fn bash_yarn_yields_javascript_and_typescript() {
        let langs = language_from_bash("yarn add react");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn bash_pnpm_yields_javascript_and_typescript() {
        let langs = language_from_bash("pnpm test");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn bash_lowercases_uppercase_tokens() {
        // Lowercasing is performed per-token before keyword match.
        assert_eq!(language_from_bash("CARGO build"), vec!["rust"]);
    }

    // -----------------------------------------------------------------
    // End-to-end `language_from_bash` coverage for shared command
    // keywords across the expanded LANGUAGES table. The slice-level
    // tests in `languages.rs::tests` pin the data; these tests pin the
    // full split/lowercase/match pipeline through which a real Bash
    // tool call resolves.
    // -----------------------------------------------------------------

    #[test]
    fn bash_gradle_build_yields_java_kotlin_groovy() {
        // AE3 end-to-end (covers R4 Gradle three-way through the bash
        // pipeline, not just the slice).
        let langs = language_from_bash("gradle build");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn bash_gradlew_yields_java_kotlin_groovy() {
        // Note: `./gradlew` would not match because the bash tokeniser
        // does not strip leading `./`. The bare `gradlew` form is what
        // exercises the keyword path.
        let langs = language_from_bash("gradlew assembleDebug");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn bash_xcodebuild_yields_swift_and_objectivec() {
        let langs = language_from_bash("xcodebuild -scheme App");
        assert!(langs.contains(&"swift".to_string()));
        assert!(langs.contains(&"objectivec".to_string()));
    }

    #[test]
    fn bash_clang_yields_clang_and_objectivec() {
        // `clang` is shared between the C entry (token `clang`) and
        // `objectivec` (Obj-C is compiled with clang) per R5.
        let langs = language_from_bash("clang -o hello hello.c");
        assert!(langs.contains(&"clang".to_string()));
        assert!(langs.contains(&"objectivec".to_string()));
    }

    #[test]
    fn bash_single_owner_keywords_resolve_to_one_entry() {
        // Spot-check single-owner keywords across the new entries: each
        // command keyword belonging to exactly one entry resolves to
        // that entry only. Catches any future accidental cross-listing.
        for (cmd, expected) in [
            ("mvn package", "java"),
            ("cabal build", "haskell"),
            ("composer install", "php"),
            ("dotnet build", "csharp"),
            ("mix deps.get", "elixir"),
            ("sbt compile", "scala"),
            ("zig build", "zig"),
            ("terraform plan", "terraform"),
            ("perl script.pl", "perl"),
            ("ruby script.rb", "ruby"),
        ] {
            let langs = language_from_bash(cmd);
            assert_eq!(
                langs,
                vec![expected.to_string()],
                "{cmd} should resolve to [{expected}], got {langs:?}"
            );
        }
    }

    // -- infer_languages priority chain --------------------------------------

    #[test]
    fn infer_marker_beats_extension() {
        // AE2: node_modules/foo/Cargo.toml infers rust via the marker
        // filename, NOT typescript via the directory hint. Marker
        // outranks extension which outranks directory hint.
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("node_modules/foo/Cargo.toml".to_string()),
            ..CallContext::empty()
        };
        assert_eq!(infer_languages(&ctx), vec!["rust"]);
    }

    #[test]
    fn infer_extension_beats_directory_hint() {
        // src/lib.rs sitting under node_modules (contrived but valid):
        // extension wins because marker filename did not match.
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("node_modules/foo/lib.rs".to_string()),
            ..CallContext::empty()
        };
        assert_eq!(infer_languages(&ctx), vec!["rust"]);
    }

    #[test]
    fn infer_directory_hint_when_nothing_else_matches() {
        // A file with no recognised extension or marker, sitting under
        // node_modules — the directory hint is the only signal.
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("node_modules/foo/anything".to_string()),
            ..CallContext::empty()
        };
        let langs = infer_languages(&ctx);
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn infer_file_and_bash_union() {
        // File path infers rust, bash command infers python.
        // The set is unioned.
        let ctx = CallContext {
            tool_name: Some("Bash".to_string()),
            command: Some("python -m my_module".to_string()),
            file_path: Some("src/main.rs".to_string()),
            ..CallContext::empty()
        };
        let langs = infer_languages(&ctx);
        assert!(langs.contains(&"rust".to_string()));
        assert!(langs.contains(&"python".to_string()));
    }

    #[test]
    fn infer_empty_when_no_signal() {
        // AE6: README.md edit with no language anchor and no bash
        // signal — empty inferred set.
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("README.md".to_string()),
            ..CallContext::empty()
        };
        assert!(infer_languages(&ctx).is_empty());
    }

    // -- extract_query multi-language anchor shape ---------------------------

    #[test]
    fn extract_query_multi_language_uses_or_group() {
        // AE9: `npm test` infers {javascript, typescript}; the FTS
        // query anchors with `(javascript OR typescript)` wrapped in
        // parens so the AND-with-terms parse remains unambiguous.
        let ctx = CallContext {
            tool_name: Some("Bash".to_string()),
            command: Some("npm test authentication".to_string()),
            ..CallContext::empty()
        };

        let query = extract_query_str(&ctx).unwrap();
        assert!(
            query.contains("javascript") && query.contains("typescript"),
            "should infer both languages: {query}"
        );
        assert!(
            query.contains(" OR ") && query.contains("AND"),
            "should have OR-grouped anchor and AND-joined terms: {query}"
        );
    }

    #[test]
    fn extract_query_bash_bundle_install_no_typescript_anchor() {
        // R2 / regression: `bundle install` must not pull in `bun`'s
        // typescript anchor.
        let ctx = CallContext {
            tool_name: Some("Bash".to_string()),
            command: Some("bundle install".to_string()),
            ..CallContext::empty()
        };
        let query = extract_query_str(&ctx);
        // Either None (if no enrichment terms either) or an OR-only
        // clause with no language anchor.
        if let Some(q) = query {
            assert!(
                !q.starts_with("typescript ") && !q.contains("(typescript"),
                "must not produce typescript anchor: {q}"
            );
            assert!(
                !q.starts_with("javascript ") && !q.contains("(javascript"),
                "must not produce javascript anchor: {q}"
            );
        }
    }

    #[test]
    fn extract_query_marker_filename_beats_extension() {
        // R3 demonstration: a hypothetical hand-built file path tests
        // the priority chain end-to-end through extract_query.
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("project/Cargo.toml".to_string()),
            ..CallContext::empty()
        };
        let query = extract_query_str(&ctx).unwrap();
        // toml extension is unknown to LANGUAGES — without the marker
        // signal, this would have no anchor.
        assert!(
            query.contains("rust"),
            "marker filename should produce rust anchor: {query}"
        );
    }

    // -- single-tuple new-language demonstration (R3) -----------------------

    #[test]
    fn r3_table_iteration_covers_all_signal_types() {
        // R3 spirit: adding a new entry would be a one-tuple change.
        // We can't add entries at runtime, but we can verify the
        // existing table fires each of the four signal helpers for at
        // least one entry — proving the iteration covers all four
        // shapes uniformly.
        assert!(!language_from_extension("a.rs").is_empty());
        assert!(!language_from_marker_filename("Cargo.toml").is_empty());
        assert!(!language_from_directory_hint("node_modules/x").is_empty());
        assert!(!language_from_bash("cargo build").is_empty());
    }
}
