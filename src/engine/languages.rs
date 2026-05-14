//! Shared declarative language table.
//!
//! Single source of truth for the four language-detection signal types
//! consumed by [`crate::engine::query`]: file extensions, Bash command
//! keywords, marker filenames, and directory hints. The current entries
//! cover the six languages lore detected before this refactor; adding a
//! new language is a single struct literal in [`LANGUAGES`].
//!
//! Each entry pairs a canonical FTS5-safe `token` (what lands in the
//! `language:` frontmatter field and in retrieval queries) with a human
//! `display_name` and four `&'static [&'static str]` signal lists. Signal
//! lists may overlap across entries — `package.json`, `npm`, and
//! `node_modules` legitimately belong to both `javascript` and
//! `typescript`. Detection accumulates languages as a set when shared
//! signals fire.
//!
//! Linear iteration is appropriate: six entries today, low double digits
//! long-term. No `OnceLock`, `phf_map`, or `lazy_static` — the
//! static-slice convention from `STOP_WORDS` already exists in
//! `query.rs`.

/// One language entry in the shared [`LANGUAGES`] table.
///
/// The four signal-list fields are evaluated independently by the
/// matching helpers in [`crate::engine::query`]. A single entry may
/// participate in multiple signal types; entries are not exclusive over
/// any single signal.
#[derive(Debug, Clone, Copy)]
pub struct LanguageEntry {
    /// Canonical FTS5-safe token. Used in `language:` frontmatter,
    /// retrieval queries, and structural-gate membership checks. Always
    /// lowercase ASCII; never an English stop-word (so `golang`, not
    /// `go`).
    pub token: &'static str,
    /// Human-readable name for surfaces that show language names in
    /// prose: authoring guide tables, CLI listings, help text. May
    /// differ from `token` when the token is sanitised for FTS5
    /// (`golang` → "Go").
    pub display_name: &'static str,
    /// File extensions (without leading dot) that imply this language.
    /// Matched case-insensitively against `Path::extension()`.
    pub extensions: &'static [&'static str],
    /// Bash command-line tokens that imply this language. Matched as
    /// whole tokens via `split_whitespace`, never substring; `bundle`
    /// will not match `bun`.
    pub command_keywords: &'static [&'static str],
    /// Filenames whose presence as the basename of a tool's `file_path`
    /// implies this language (e.g. `Cargo.toml`, `package.json`).
    /// Compared case-sensitively because the conventional casing is
    /// part of the marker.
    pub marker_filenames: &'static [&'static str],
    /// Directory-name components in a tool's `file_path` that imply
    /// this language (e.g. `node_modules`, `.venv`). Matched
    /// case-sensitively against any path component.
    pub directory_hints: &'static [&'static str],
}

/// The shared language table. Order is not significant for correctness;
/// `extract_query` accumulates matches across all entries.
///
/// Per R16 (initial coverage), this lists the six languages that lore
/// detected before the refactor. R17 keeps the slice at six; the
/// follow-on language pack adds entries one at a time as separate PRs.
pub static LANGUAGES: &[LanguageEntry] = &[
    LanguageEntry {
        token: "rust",
        display_name: "Rust",
        extensions: &["rs"],
        command_keywords: &["cargo"],
        marker_filenames: &["Cargo.toml", "Cargo.lock"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "typescript",
        display_name: "TypeScript",
        extensions: &["ts", "tsx"],
        command_keywords: &["npm", "npx", "yarn", "bun", "pnpm"],
        marker_filenames: &["tsconfig.json", "package.json"],
        directory_hints: &["node_modules"],
    },
    LanguageEntry {
        token: "javascript",
        display_name: "JavaScript",
        extensions: &["js", "jsx"],
        command_keywords: &["npm", "npx", "yarn", "bun", "pnpm"],
        marker_filenames: &["package.json", "package-lock.json"],
        directory_hints: &["node_modules"],
    },
    LanguageEntry {
        token: "yaml",
        display_name: "YAML",
        extensions: &["yml", "yaml"],
        command_keywords: &[],
        marker_filenames: &[],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "python",
        display_name: "Python",
        extensions: &["py"],
        command_keywords: &["pip", "python", "python3"],
        marker_filenames: &["pyproject.toml", "requirements.txt", "Pipfile", "setup.py"],
        directory_hints: &["__pycache__", ".venv"],
    },
    LanguageEntry {
        token: "golang",
        display_name: "Go",
        extensions: &["go"],
        command_keywords: &["go"],
        marker_filenames: &["go.mod", "go.sum"],
        directory_hints: &[],
    },
];

/// Returns `true` when `token` matches the canonical `token` of any
/// entry in [`LANGUAGES`].
///
/// Used by the frontmatter validator (`chunking.rs`) and the MCP search
/// path (`server.rs`) to discriminate known canonical tokens from
/// unknown ones. Comparison is case-sensitive; callers normalise to
/// lowercase before invoking.
pub fn is_known_token(token: &str) -> bool {
    LANGUAGES.iter().any(|entry| entry.token == token)
}

/// Iterate over `LANGUAGES` collecting the canonical tokens of every
/// entry whose `extensions` slice contains `ext` (matched after
/// `ext.to_lowercase()`).
///
/// Returns an empty `Vec` when no entry matches. Multiple entries may
/// match the same extension in principle, though the initial table has
/// no such overlap.
pub fn languages_for_extension(ext: &str) -> Vec<String> {
    let lower = ext.to_lowercase();
    LANGUAGES
        .iter()
        .filter(|entry| entry.extensions.contains(&lower.as_str()))
        .map(|entry| entry.token.to_string())
        .collect()
}

/// Iterate over `LANGUAGES` collecting the canonical tokens of every
/// entry whose `command_keywords` slice contains `keyword`.
///
/// Comparison is case-sensitive against the lowercase-normalised
/// keyword; callers are expected to lowercase the bash token before
/// invoking. Multiple entries may match (e.g. `npm` registers in both
/// `javascript` and `typescript` per R5).
pub fn languages_for_command_keyword(keyword: &str) -> Vec<String> {
    LANGUAGES
        .iter()
        .filter(|entry| entry.command_keywords.contains(&keyword))
        .map(|entry| entry.token.to_string())
        .collect()
}

/// Iterate over `LANGUAGES` collecting the canonical tokens of every
/// entry whose `marker_filenames` slice contains `filename`.
///
/// Filename comparison is case-sensitive — `Cargo.toml` matches but
/// `cargo.toml` does not, because the conventional casing is part of
/// the marker. Multiple entries may match (e.g. `package.json` for
/// both `javascript` and `typescript`).
pub fn languages_for_marker_filename(filename: &str) -> Vec<String> {
    LANGUAGES
        .iter()
        .filter(|entry| entry.marker_filenames.contains(&filename))
        .map(|entry| entry.token.to_string())
        .collect()
}

/// Iterate over `LANGUAGES` collecting the canonical tokens of every
/// entry whose `directory_hints` slice contains `component`.
///
/// Comparison is case-sensitive against the literal component name
/// (e.g. `node_modules`, `__pycache__`). Multiple entries may match
/// (e.g. `node_modules` for both `javascript` and `typescript`).
pub fn languages_for_directory_hint(component: &str) -> Vec<String> {
    LANGUAGES
        .iter()
        .filter(|entry| entry.directory_hints.contains(&component))
        .map(|entry| entry.token.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_entry_has_expected_signals() {
        let rust = LANGUAGES.iter().find(|e| e.token == "rust").unwrap();
        assert_eq!(rust.display_name, "Rust");
        assert!(rust.extensions.contains(&"rs"));
        assert!(rust.command_keywords.contains(&"cargo"));
        assert!(rust.marker_filenames.contains(&"Cargo.toml"));
    }

    #[test]
    fn golang_token_avoids_english_stop_word() {
        // Display name is "Go"; the FTS-safe token is "golang" because
        // bare `go` would conflict with the English stop-word list and
        // FTS5 tokeniser defaults.
        let golang = LANGUAGES.iter().find(|e| e.token == "golang").unwrap();
        assert_eq!(golang.display_name, "Go");
        assert!(golang.extensions.contains(&"go"));
        assert!(golang.command_keywords.contains(&"go"));
    }

    #[test]
    fn npm_keyword_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_command_keyword("npm");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn package_json_marker_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_marker_filename("package.json");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn node_modules_directory_hint_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_directory_hint("node_modules");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn cargo_toml_marker_is_case_sensitive() {
        assert_eq!(languages_for_marker_filename("Cargo.toml"), vec!["rust"]);
        assert!(languages_for_marker_filename("cargo.toml").is_empty());
    }

    #[test]
    fn extension_lookup_is_case_insensitive() {
        assert_eq!(languages_for_extension("RS"), vec!["rust"]);
        assert_eq!(languages_for_extension("rs"), vec!["rust"]);
    }

    #[test]
    fn extension_tsx_resolves_to_typescript() {
        assert_eq!(languages_for_extension("tsx"), vec!["typescript"]);
    }

    #[test]
    fn unknown_extension_returns_empty() {
        assert!(languages_for_extension("txt").is_empty());
    }

    #[test]
    fn unknown_command_keyword_returns_empty() {
        assert!(languages_for_command_keyword("bundle").is_empty());
    }

    #[test]
    fn is_known_token_accepts_canonical_tokens() {
        assert!(is_known_token("rust"));
        assert!(is_known_token("typescript"));
        assert!(is_known_token("javascript"));
        assert!(is_known_token("yaml"));
        assert!(is_known_token("python"));
        assert!(is_known_token("golang"));
    }

    #[test]
    fn is_known_token_rejects_unknown_and_display_names() {
        assert!(!is_known_token("rrust"));
        assert!(!is_known_token("kotlin"));
        // Display names are not tokens — authors must type the token.
        assert!(!is_known_token("Rust"));
        assert!(!is_known_token("Go"));
    }
}
