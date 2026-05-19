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
//! Linear iteration is appropriate at the current low-double-digit
//! entry count. No `OnceLock`, `phf_map`, or `lazy_static` — the
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
/// The original six entries (PR #50, R16) are kept in their historical
/// positions; subsequent additions appended below in alphabetical-by-token
/// order for reviewer-friendly diffs.
pub static LANGUAGES: &[LanguageEntry] = &[
    LanguageEntry {
        token: "rust",
        display_name: "Rust",
        extensions: &["rs"],
        command_keywords: &["cargo"],
        marker_filenames: &[
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "rust-toolchain",
        ],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "typescript",
        display_name: "TypeScript",
        extensions: &["ts", "tsx"],
        command_keywords: &["npm", "npx", "yarn", "bun", "pnpm"],
        marker_filenames: &[
            "tsconfig.json",
            "package.json",
            "package-lock.json",
            ".node-version",
            ".nvmrc",
            "yarn.lock",
            "pnpm-lock.yaml",
            "bun.lockb",
        ],
        directory_hints: &["node_modules"],
    },
    LanguageEntry {
        token: "javascript",
        display_name: "JavaScript",
        extensions: &["js", "jsx"],
        command_keywords: &["npm", "npx", "yarn", "bun", "pnpm"],
        marker_filenames: &[
            "package.json",
            "package-lock.json",
            ".node-version",
            ".nvmrc",
            "yarn.lock",
            "pnpm-lock.yaml",
            "bun.lockb",
        ],
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
        marker_filenames: &[
            "pyproject.toml",
            "requirements.txt",
            "Pipfile",
            "setup.py",
            ".python-version",
            "Pipfile.lock",
            "poetry.lock",
            "uv.lock",
        ],
        directory_hints: &["__pycache__", ".venv"],
    },
    LanguageEntry {
        token: "golang",
        display_name: "Go",
        extensions: &["go"],
        command_keywords: &["go"],
        marker_filenames: &["go.mod", "go.sum", ".go-version", "go.work"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "bash",
        display_name: "Shell",
        extensions: &["sh", "bash", "zsh"],
        command_keywords: &["bash", "sh", "zsh"],
        marker_filenames: &[],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "clang",
        display_name: "C",
        extensions: &["c", "h"],
        command_keywords: &["clang", "gcc"],
        marker_filenames: &["CMakeLists.txt"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "clojure",
        display_name: "Clojure",
        extensions: &["clj", "cljs", "cljc", "edn"],
        command_keywords: &["clj", "clojure", "lein"],
        marker_filenames: &["project.clj", "deps.edn"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "cpp",
        display_name: "C++",
        extensions: &["cpp", "cxx", "cc", "h", "hpp", "hxx"],
        command_keywords: &["clang++", "g++"],
        marker_filenames: &["CMakeLists.txt"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "csharp",
        display_name: "C#",
        extensions: &["cs", "csx", "csproj", "sln"],
        command_keywords: &["dotnet"],
        marker_filenames: &["global.json", "nuget.config"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "dart",
        display_name: "Dart",
        extensions: &["dart"],
        command_keywords: &["dart", "flutter", "pub"],
        marker_filenames: &["pubspec.yaml", "pubspec.lock"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "elixir",
        display_name: "Elixir",
        extensions: &["ex", "exs"],
        command_keywords: &["mix", "iex", "elixir", "elixirc"],
        marker_filenames: &["mix.exs", "mix.lock"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "groovy",
        display_name: "Groovy",
        extensions: &["groovy", "gvy", "gradle"],
        command_keywords: &["groovy", "groovyc", "gradle", "gradlew"],
        marker_filenames: &["build.gradle", "settings.gradle"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "haskell",
        display_name: "Haskell",
        extensions: &["hs", "lhs", "cabal"],
        command_keywords: &["ghc", "ghci", "cabal", "stack", "runghc", "runhaskell"],
        marker_filenames: &["stack.yaml", "cabal.project"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "java",
        display_name: "Java",
        extensions: &["java"],
        command_keywords: &["java", "javac", "mvn", "gradle", "gradlew"],
        marker_filenames: &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "kotlin",
        display_name: "Kotlin",
        extensions: &["kt", "kts"],
        command_keywords: &["kotlin", "kotlinc", "gradle", "gradlew"],
        marker_filenames: &[
            "build.gradle.kts",
            "settings.gradle.kts",
            "build.gradle",
            "settings.gradle",
        ],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "lua",
        display_name: "Lua",
        extensions: &["lua", "rockspec"],
        command_keywords: &["lua", "luac", "luarocks"],
        marker_filenames: &[".luarc.json"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "nix",
        display_name: "Nix",
        extensions: &["nix"],
        command_keywords: &["nix", "nix-shell", "nix-build", "nix-env"],
        marker_filenames: &["flake.nix", "flake.lock", "default.nix", "shell.nix"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "objectivec",
        display_name: "Objective-C",
        extensions: &["m", "mm"],
        command_keywords: &["pod", "xcodebuild", "clang"],
        marker_filenames: &["Podfile"],
        directory_hints: &["Pods"],
    },
    LanguageEntry {
        token: "perl",
        display_name: "Perl",
        extensions: &["pl", "pm", "pod"],
        command_keywords: &["perl", "cpan", "cpanm"],
        marker_filenames: &["cpanfile", "Makefile.PL"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "php",
        display_name: "PHP",
        extensions: &["php"],
        command_keywords: &["php", "composer"],
        marker_filenames: &["composer.json", "composer.lock", ".php-version"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "ruby",
        display_name: "Ruby",
        extensions: &["rb"],
        command_keywords: &["ruby", "bundle", "rake", "gem"],
        marker_filenames: &["Gemfile", "Gemfile.lock", "Rakefile", ".ruby-version"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "scala",
        display_name: "Scala",
        extensions: &["scala", "sc"],
        command_keywords: &["scala", "scalac", "sbt"],
        marker_filenames: &["build.sbt"],
        directory_hints: &[],
    },
    LanguageEntry {
        token: "swift",
        display_name: "Swift",
        extensions: &["swift"],
        command_keywords: &["swift", "xcodebuild"],
        marker_filenames: &["Package.swift", "Podfile"],
        directory_hints: &["Pods"],
    },
    LanguageEntry {
        token: "terraform",
        display_name: "Terraform",
        extensions: &["tf", "tfvars"],
        command_keywords: &["terraform", "tofu", "tflint"],
        marker_filenames: &["terraform.tfvars", ".terraform.lock.hcl"],
        directory_hints: &[".terraform"],
    },
    LanguageEntry {
        token: "zig",
        display_name: "Zig",
        extensions: &["zig", "zon"],
        command_keywords: &["zig"],
        marker_filenames: &["build.zig", "build.zig.zon"],
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

/// Returns the human-readable `display_name` for `token`, falling back
/// to `token` itself when no entry matches.
///
/// Suitable for any surface that renders language tokens to operators.
/// The fallback covers the case where a stored `language_json` array
/// contains a token the table does not (yet) cover — for example, a
/// knowledge base ingested while pinned to a newer language pack than
/// the running binary. Returning the raw token keeps the output
/// legible rather than silently dropping it.
///
/// The returned slice's lifetime is tied to `token`: known tokens
/// return a `&'static str` borrowed from [`LANGUAGES`]; unknown tokens
/// return the input slice unchanged. Callers that need to own the
/// result should clone it.
pub fn display_name_for(token: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|entry| entry.token == token)
        .map_or(token, |entry| entry.display_name)
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

    fn entry_for(token: &str) -> &'static LanguageEntry {
        LANGUAGES
            .iter()
            .find(|e| e.token == token)
            .unwrap_or_else(|| panic!("no entry for token {token}"))
    }

    #[test]
    fn rust_entry_has_expected_signals() {
        let rust = entry_for("rust");
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
        let golang = entry_for("golang");
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
        // `nosuchcmd` is a synthetic canary: no LANGUAGES entry registers
        // it. (The original canary `bundle` is now a Ruby command keyword.)
        assert!(languages_for_command_keyword("nosuchcmd").is_empty());
    }

    #[test]
    fn is_known_token_accepts_canonical_tokens() {
        assert!(is_known_token("rust"));
        assert!(is_known_token("typescript"));
        assert!(is_known_token("javascript"));
        assert!(is_known_token("yaml"));
        assert!(is_known_token("python"));
        assert!(is_known_token("golang"));
        assert!(is_known_token("bash"));
        assert!(is_known_token("clang"));
        assert!(is_known_token("clojure"));
        assert!(is_known_token("cpp"));
        assert!(is_known_token("csharp"));
        assert!(is_known_token("dart"));
        assert!(is_known_token("elixir"));
        assert!(is_known_token("groovy"));
        assert!(is_known_token("haskell"));
        assert!(is_known_token("java"));
        assert!(is_known_token("kotlin"));
        assert!(is_known_token("lua"));
        assert!(is_known_token("nix"));
        assert!(is_known_token("objectivec"));
        assert!(is_known_token("perl"));
        assert!(is_known_token("php"));
        assert!(is_known_token("ruby"));
        assert!(is_known_token("scala"));
        assert!(is_known_token("swift"));
        assert!(is_known_token("terraform"));
        assert!(is_known_token("zig"));
    }

    #[test]
    fn is_known_token_rejects_unknown_and_display_names() {
        assert!(!is_known_token("rrust"));
        // MATLAB is the deferred `.m` contestation owner (origin Key
        // Decisions): `.m` is claimed for `objectivec`, MATLAB-pattern
        // authors hit the R12 unknown-token-warn path indefinitely.
        assert!(!is_known_token("matlab"));
        // Display names are not tokens — authors must type the token.
        assert!(!is_known_token("Rust"));
        assert!(!is_known_token("Go"));
    }

    #[test]
    fn display_name_for_resolves_known_tokens() {
        assert_eq!(display_name_for("rust"), "Rust");
        assert_eq!(display_name_for("typescript"), "TypeScript");
        assert_eq!(display_name_for("javascript"), "JavaScript");
        assert_eq!(display_name_for("yaml"), "YAML");
        assert_eq!(display_name_for("python"), "Python");
        // `golang` is the canonical token; "Go" is the display name.
        assert_eq!(display_name_for("golang"), "Go");
        assert_eq!(display_name_for("bash"), "Shell");
        assert_eq!(display_name_for("clang"), "C");
        assert_eq!(display_name_for("clojure"), "Clojure");
        assert_eq!(display_name_for("cpp"), "C++");
        assert_eq!(display_name_for("csharp"), "C#");
        assert_eq!(display_name_for("dart"), "Dart");
        assert_eq!(display_name_for("elixir"), "Elixir");
        assert_eq!(display_name_for("groovy"), "Groovy");
        assert_eq!(display_name_for("haskell"), "Haskell");
        assert_eq!(display_name_for("java"), "Java");
        assert_eq!(display_name_for("kotlin"), "Kotlin");
        assert_eq!(display_name_for("lua"), "Lua");
        assert_eq!(display_name_for("nix"), "Nix");
        assert_eq!(display_name_for("objectivec"), "Objective-C");
        assert_eq!(display_name_for("perl"), "Perl");
        assert_eq!(display_name_for("php"), "PHP");
        assert_eq!(display_name_for("ruby"), "Ruby");
        assert_eq!(display_name_for("scala"), "Scala");
        assert_eq!(display_name_for("swift"), "Swift");
        assert_eq!(display_name_for("terraform"), "Terraform");
        assert_eq!(display_name_for("zig"), "Zig");
    }

    #[test]
    fn display_name_for_falls_back_to_raw_token_when_unknown() {
        assert_eq!(display_name_for("matlab"), "matlab");
        assert_eq!(display_name_for(""), "");
    }

    // -----------------------------------------------------------------
    // Entry-level signal tests for the 21 new languages.
    //
    // Each asserts: canonical token resolves, display name matches, at
    // least one extension is listed, and (where applicable) at least one
    // command keyword and one marker. Mirrors `rust_entry_has_expected_signals`.
    // -----------------------------------------------------------------

    #[test]
    fn bash_entry_has_expected_signals() {
        let e = entry_for("bash");
        assert_eq!(e.display_name, "Shell");
        assert!(e.extensions.contains(&"sh"));
        assert!(e.extensions.contains(&"bash"));
        assert!(e.extensions.contains(&"zsh"));
        assert!(e.command_keywords.contains(&"bash"));
    }

    #[test]
    fn clang_entry_has_expected_signals() {
        let e = entry_for("clang");
        assert_eq!(e.display_name, "C");
        assert!(e.extensions.contains(&"c"));
        assert!(e.extensions.contains(&"h"));
        assert!(e.command_keywords.contains(&"clang"));
        assert!(e.marker_filenames.contains(&"CMakeLists.txt"));
    }

    #[test]
    fn clojure_entry_has_expected_signals() {
        let e = entry_for("clojure");
        assert_eq!(e.display_name, "Clojure");
        assert!(e.extensions.contains(&"clj"));
        assert!(e.command_keywords.contains(&"clojure"));
        assert!(e.marker_filenames.contains(&"deps.edn"));
    }

    #[test]
    fn cpp_entry_has_expected_signals() {
        let e = entry_for("cpp");
        assert_eq!(e.display_name, "C++");
        assert!(e.extensions.contains(&"cpp"));
        assert!(e.extensions.contains(&"h"));
        assert!(e.extensions.contains(&"hpp"));
        assert!(e.command_keywords.contains(&"g++"));
        assert!(e.marker_filenames.contains(&"CMakeLists.txt"));
    }

    #[test]
    fn csharp_entry_has_expected_signals() {
        let e = entry_for("csharp");
        assert_eq!(e.display_name, "C#");
        assert!(e.extensions.contains(&"cs"));
        assert!(e.command_keywords.contains(&"dotnet"));
        assert!(e.marker_filenames.contains(&"global.json"));
    }

    #[test]
    fn dart_entry_has_expected_signals() {
        let e = entry_for("dart");
        assert_eq!(e.display_name, "Dart");
        assert!(e.extensions.contains(&"dart"));
        assert!(e.command_keywords.contains(&"dart"));
        assert!(e.marker_filenames.contains(&"pubspec.yaml"));
    }

    #[test]
    fn elixir_entry_has_expected_signals() {
        // AE6: `.ex` resolves to `elixir`.
        let e = entry_for("elixir");
        assert_eq!(e.display_name, "Elixir");
        assert!(e.extensions.contains(&"ex"));
        assert!(e.extensions.contains(&"exs"));
        assert!(e.command_keywords.contains(&"mix"));
        assert!(e.marker_filenames.contains(&"mix.exs"));
    }

    #[test]
    fn groovy_entry_has_expected_signals() {
        let e = entry_for("groovy");
        assert_eq!(e.display_name, "Groovy");
        assert!(e.extensions.contains(&"groovy"));
        assert!(e.command_keywords.contains(&"groovy"));
        assert!(e.marker_filenames.contains(&"build.gradle"));
    }

    #[test]
    fn haskell_entry_has_expected_signals() {
        let e = entry_for("haskell");
        assert_eq!(e.display_name, "Haskell");
        assert!(e.extensions.contains(&"hs"));
        assert!(e.command_keywords.contains(&"cabal"));
        assert!(e.marker_filenames.contains(&"stack.yaml"));
    }

    #[test]
    fn java_entry_has_expected_signals() {
        let e = entry_for("java");
        assert_eq!(e.display_name, "Java");
        assert!(e.extensions.contains(&"java"));
        assert!(e.command_keywords.contains(&"javac"));
        assert!(e.marker_filenames.contains(&"pom.xml"));
    }

    #[test]
    fn kotlin_entry_has_expected_signals() {
        let e = entry_for("kotlin");
        assert_eq!(e.display_name, "Kotlin");
        assert!(e.extensions.contains(&"kt"));
        assert!(e.extensions.contains(&"kts"));
        assert!(e.command_keywords.contains(&"kotlinc"));
        assert!(e.marker_filenames.contains(&"build.gradle.kts"));
    }

    #[test]
    fn lua_entry_has_expected_signals() {
        let e = entry_for("lua");
        assert_eq!(e.display_name, "Lua");
        assert!(e.extensions.contains(&"lua"));
        assert!(e.command_keywords.contains(&"lua"));
        assert!(e.marker_filenames.contains(&".luarc.json"));
    }

    #[test]
    fn nix_entry_has_expected_signals() {
        let e = entry_for("nix");
        assert_eq!(e.display_name, "Nix");
        assert!(e.extensions.contains(&"nix"));
        assert!(e.command_keywords.contains(&"nix"));
        assert!(e.marker_filenames.contains(&"flake.nix"));
    }

    #[test]
    fn objectivec_entry_has_expected_signals() {
        let e = entry_for("objectivec");
        assert_eq!(e.display_name, "Objective-C");
        assert!(e.extensions.contains(&"m"));
        assert!(e.extensions.contains(&"mm"));
        assert!(e.command_keywords.contains(&"xcodebuild"));
        assert!(e.marker_filenames.contains(&"Podfile"));
        assert!(e.directory_hints.contains(&"Pods"));
    }

    #[test]
    fn perl_entry_has_expected_signals() {
        let e = entry_for("perl");
        assert_eq!(e.display_name, "Perl");
        assert!(e.extensions.contains(&"pl"));
        assert!(e.extensions.contains(&"pm"));
        assert!(e.command_keywords.contains(&"perl"));
        assert!(e.marker_filenames.contains(&"cpanfile"));
    }

    #[test]
    fn php_entry_has_expected_signals() {
        let e = entry_for("php");
        assert_eq!(e.display_name, "PHP");
        assert!(e.extensions.contains(&"php"));
        assert!(e.command_keywords.contains(&"php"));
        assert!(e.marker_filenames.contains(&"composer.json"));
    }

    #[test]
    fn ruby_entry_has_expected_signals() {
        let e = entry_for("ruby");
        assert_eq!(e.display_name, "Ruby");
        assert!(e.extensions.contains(&"rb"));
        assert!(e.command_keywords.contains(&"ruby"));
        assert!(e.marker_filenames.contains(&"Gemfile"));
    }

    #[test]
    fn scala_entry_has_expected_signals() {
        let e = entry_for("scala");
        assert_eq!(e.display_name, "Scala");
        assert!(e.extensions.contains(&"scala"));
        assert!(e.command_keywords.contains(&"sbt"));
        assert!(e.marker_filenames.contains(&"build.sbt"));
    }

    #[test]
    fn swift_entry_has_expected_signals() {
        let e = entry_for("swift");
        assert_eq!(e.display_name, "Swift");
        assert!(e.extensions.contains(&"swift"));
        assert!(e.command_keywords.contains(&"swift"));
        assert!(e.marker_filenames.contains(&"Package.swift"));
        assert!(e.directory_hints.contains(&"Pods"));
    }

    #[test]
    fn terraform_entry_has_expected_signals() {
        let e = entry_for("terraform");
        assert_eq!(e.display_name, "Terraform");
        assert!(e.extensions.contains(&"tf"));
        assert!(e.extensions.contains(&"tfvars"));
        // `.hcl` is intentionally NOT in the terraform entry — origin
        // Key Decisions: avoid R5 contestation with Packer / Vault /
        // Consul / Boundary. Broader HCL coverage is deferred.
        assert!(!e.extensions.contains(&"hcl"));
        assert!(e.command_keywords.contains(&"terraform"));
        assert!(e.marker_filenames.contains(&"terraform.tfvars"));
        assert!(e.directory_hints.contains(&".terraform"));
    }

    #[test]
    fn zig_entry_has_expected_signals() {
        let e = entry_for("zig");
        assert_eq!(e.display_name, "Zig");
        assert!(e.extensions.contains(&"zig"));
        assert!(e.command_keywords.contains(&"zig"));
        assert!(e.marker_filenames.contains(&"build.zig"));
    }

    // -----------------------------------------------------------------
    // Shared-signal multi-membership tests (new-only sets, per U1).
    // Composition-cascade discipline: each three-way / two-way claim
    // earns its own pinned assertion so a silent drop on one entry
    // surfaces immediately.
    // -----------------------------------------------------------------

    #[test]
    fn gradle_keyword_fires_for_java_kotlin_and_groovy() {
        // AE3: `gradle build` resolves to `{java, kotlin, groovy}`.
        let langs = languages_for_command_keyword("gradle");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn gradlew_keyword_fires_for_java_kotlin_and_groovy() {
        let langs = languages_for_command_keyword("gradlew");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn build_gradle_marker_fires_for_java_kotlin_and_groovy() {
        let langs = languages_for_marker_filename("build.gradle");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn settings_gradle_marker_fires_for_java_kotlin_and_groovy() {
        let langs = languages_for_marker_filename("settings.gradle");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn build_gradle_kts_marker_fires_for_java_and_kotlin_only() {
        // Groovy does not use KTS — explicitly excluded per origin
        // R4 key decision.
        let langs = languages_for_marker_filename("build.gradle.kts");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(!langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn settings_gradle_kts_marker_fires_for_java_and_kotlin_only() {
        let langs = languages_for_marker_filename("settings.gradle.kts");
        assert!(langs.contains(&"java".to_string()));
        assert!(langs.contains(&"kotlin".to_string()));
        assert!(!langs.contains(&"groovy".to_string()));
    }

    #[test]
    fn cmake_lists_marker_fires_for_clang_and_cpp() {
        let langs = languages_for_marker_filename("CMakeLists.txt");
        assert!(langs.contains(&"clang".to_string()));
        assert!(langs.contains(&"cpp".to_string()));
    }

    #[test]
    fn h_extension_fires_for_clang_and_cpp() {
        // AE1: `.h` resolves to `{clang, cpp}` (R5 multi-entry —
        // modern C++ codebases overwhelmingly use `.h` for headers).
        let langs = languages_for_extension("h");
        assert!(langs.contains(&"clang".to_string()));
        assert!(langs.contains(&"cpp".to_string()));
    }

    #[test]
    fn xcodebuild_keyword_fires_for_swift_and_objectivec() {
        let langs = languages_for_command_keyword("xcodebuild");
        assert!(langs.contains(&"swift".to_string()));
        assert!(langs.contains(&"objectivec".to_string()));
    }

    #[test]
    fn podfile_marker_fires_for_swift_and_objectivec() {
        // AE5: `Podfile` resolves to `{swift, objectivec}`.
        let langs = languages_for_marker_filename("Podfile");
        assert!(langs.contains(&"swift".to_string()));
        assert!(langs.contains(&"objectivec".to_string()));
    }

    #[test]
    fn pods_directory_hint_fires_for_swift_and_objectivec() {
        let langs = languages_for_directory_hint("Pods");
        assert!(langs.contains(&"swift".to_string()));
        assert!(langs.contains(&"objectivec".to_string()));
    }

    #[test]
    fn clang_keyword_fires_for_clang_and_objectivec() {
        // Obj-C is genuinely compiled with clang — R5 shared, not
        // contested.
        let langs = languages_for_command_keyword("clang");
        assert!(langs.contains(&"clang".to_string()));
        assert!(langs.contains(&"objectivec".to_string()));
    }

    // -----------------------------------------------------------------
    // Contested-signal resolution tests (R3 + AE1/AE2).
    // -----------------------------------------------------------------

    #[test]
    fn mm_extension_resolves_only_to_objectivec() {
        // `.mm` is Objective-C++. Single-owner to `objectivec`.
        let langs = languages_for_extension("mm");
        assert_eq!(langs.len(), 1);
        assert!(langs.contains(&"objectivec".to_string()));
    }

    #[test]
    fn m_extension_resolves_only_to_objectivec() {
        // AE2: `.m` is single-owner to `objectivec`; MATLAB-pattern
        // authors route through R12's unknown-token-warn path.
        let langs = languages_for_extension("m");
        assert_eq!(langs, vec!["objectivec".to_string()]);
        assert!(!langs.contains(&"clang".to_string()));
        assert!(!langs.contains(&"cpp".to_string()));
    }

    #[test]
    fn h_extension_does_not_resolve_to_objectivec() {
        // Negative half of AE1: Obj-C source files are typically
        // `.m`/`.mm`; Obj-C patterns about C-family headers declare
        // `language:` explicitly.
        let langs = languages_for_extension("h");
        assert!(!langs.contains(&"objectivec".to_string()));
    }

    // -----------------------------------------------------------------
    // Back-fill shared-marker tests (U2): Node ecosystem markers now
    // multi-entry across `javascript` and `typescript`.
    // -----------------------------------------------------------------

    #[test]
    fn package_lock_json_marker_fires_for_both_javascript_and_typescript() {
        // AE4: pins the PR #50 regression — `package-lock.json` must
        // resolve to both, not just `javascript`.
        let langs = languages_for_marker_filename("package-lock.json");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn node_version_marker_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_marker_filename(".node-version");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn nvmrc_marker_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_marker_filename(".nvmrc");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn yarn_lock_marker_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_marker_filename("yarn.lock");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn pnpm_lock_yaml_marker_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_marker_filename("pnpm-lock.yaml");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    #[test]
    fn bun_lockb_marker_fires_for_both_javascript_and_typescript() {
        let langs = languages_for_marker_filename("bun.lockb");
        assert!(langs.contains(&"javascript".to_string()));
        assert!(langs.contains(&"typescript".to_string()));
    }

    // -----------------------------------------------------------------
    // Back-fill single-owner marker tests (U2).
    //
    // Assert single-ownership as "length == 1 AND contains <token>" so
    // the tests do not couple to LANGUAGES slice ordering. A future
    // legitimate co-claim on any of these markers will surface as a
    // clear membership change, not an opaque ordering failure.
    // -----------------------------------------------------------------

    fn assert_single_owner_marker(marker: &str, token: &str) {
        let langs = languages_for_marker_filename(marker);
        assert_eq!(
            langs.len(),
            1,
            "{marker} should resolve to exactly one entry, got {langs:?}"
        );
        assert!(
            langs.contains(&token.to_string()),
            "{marker} should resolve to {token}, got {langs:?}"
        );
    }

    #[test]
    fn python_version_marker_fires_for_python_only() {
        assert_single_owner_marker(".python-version", "python");
    }

    #[test]
    fn pipfile_lock_marker_fires_for_python_only() {
        assert_single_owner_marker("Pipfile.lock", "python");
    }

    #[test]
    fn poetry_lock_marker_fires_for_python_only() {
        assert_single_owner_marker("poetry.lock", "python");
    }

    #[test]
    fn uv_lock_marker_fires_for_python_only() {
        assert_single_owner_marker("uv.lock", "python");
    }

    #[test]
    fn go_version_marker_fires_for_golang_only() {
        assert_single_owner_marker(".go-version", "golang");
    }

    #[test]
    fn go_work_marker_fires_for_golang_only() {
        assert_single_owner_marker("go.work", "golang");
    }

    #[test]
    fn rust_toolchain_toml_marker_fires_for_rust_only() {
        assert_single_owner_marker("rust-toolchain.toml", "rust");
    }

    #[test]
    fn rust_toolchain_marker_fires_for_rust_only() {
        assert_single_owner_marker("rust-toolchain", "rust");
    }
}
