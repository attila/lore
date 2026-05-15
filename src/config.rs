use std::path::{Path, PathBuf};

use etcetera::base_strategy::{BaseStrategy, Xdg};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub knowledge_dir: PathBuf,
    pub database: PathBuf,
    // TODO: evaluate TCP transport as alternative to stdio
    pub bind: String,
    pub ollama: OllamaConfig,
    pub search: SearchConfig,
    pub chunking: ChunkingConfig,
    #[serde(default)]
    pub git: Option<GitConfig>,
    #[serde(default)]
    pub trace: TraceConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitConfig {
    pub inbox_branch_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OllamaConfig {
    pub host: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchConfig {
    pub hybrid: bool,
    pub top_k: usize,
    #[serde(default = "default_min_relevance")]
    pub min_relevance: f64,
    /// Optional threshold floor applied only to universal-tagged results.
    /// `None` (default) → inherit `min_relevance`. Lets operators raise the
    /// universal floor without affecting ranked non-universal injections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_relevance_universal: Option<f64>,
}

impl SearchConfig {
    /// Effective relevance floor for universal results: the explicit
    /// `min_relevance_universal` override if set, otherwise `min_relevance`.
    pub fn effective_min_relevance_universal(&self) -> f64 {
        self.min_relevance_universal.unwrap_or(self.min_relevance)
    }
}

fn default_min_relevance() -> f64 {
    0.6
}

/// Per-hook trace logging config (Track 2 Observability).
///
/// Mirrors the `SearchConfig` per-field-default pattern so the `[trace]` section
/// is discoverable in every fresh `lore init` while remaining off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceConfig {
    #[serde(default = "default_trace_enabled")]
    pub enabled: bool,
    #[serde(default = "default_retain_days")]
    pub retain_days: u32,
    #[serde(default = "default_gzip_older_than_days")]
    pub gzip_older_than_days: u32,
    #[serde(default = "default_include_full_command")]
    pub include_full_command: bool,
    #[serde(default = "default_include_transcript_tail")]
    pub include_transcript_tail: bool,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            enabled: default_trace_enabled(),
            retain_days: default_retain_days(),
            gzip_older_than_days: default_gzip_older_than_days(),
            include_full_command: default_include_full_command(),
            include_transcript_tail: default_include_transcript_tail(),
        }
    }
}

fn default_trace_enabled() -> bool {
    false
}
fn default_retain_days() -> u32 {
    30
}
fn default_gzip_older_than_days() -> u32 {
    7
}
fn default_include_full_command() -> bool {
    false
}
fn default_include_transcript_tail() -> bool {
    false
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkingConfig {
    pub strategy: String,
    // TODO: implement token-based chunk size limiting
    pub max_tokens: usize,
}

impl Config {
    pub fn default_with(knowledge_dir: PathBuf, database: PathBuf, model: &str) -> Self {
        Self {
            knowledge_dir,
            database,
            bind: "localhost:3100".to_string(),
            ollama: OllamaConfig {
                host: "http://127.0.0.1:11434".to_string(),
                model: model.to_string(),
            },
            search: SearchConfig {
                hybrid: true,
                top_k: 5,
                min_relevance: default_min_relevance(),
                min_relevance_universal: None,
            },
            chunking: ChunkingConfig {
                strategy: "heading".to_string(),
                max_tokens: 1024,
            },
            git: None,
            trace: TraceConfig::default(),
        }
    }

    /// Returns `true` when per-hook tracing should be active for this process.
    ///
    /// Precedence: `LORE_TRACE` env var (when recognised) overrides the
    /// persistent `[trace] enabled` config flag. Recognised truthy values:
    /// `1`, `true`, `yes` (case-sensitive). Recognised falsy values: `0`,
    /// `false`, `no` (case-sensitive). Any other value, including the empty
    /// string, is treated as unset and silently falls through to
    /// `self.trace.enabled` — mirroring `LORE_DEBUG`'s fail-soft semantics.
    pub fn trace_enabled(&self) -> bool {
        match std::env::var("LORE_TRACE").ok().as_deref() {
            Some("1" | "true" | "yes") => true,
            Some("0" | "false" | "no") => false,
            _ => self.trace.enabled,
        }
    }

    pub fn inbox_branch_prefix(&self) -> Option<&str> {
        self.git.as_ref().map(|g| g.inbox_branch_prefix.as_str())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path).map_err(|_| {
            anyhow::anyhow!(
                "Config not found at {}. Run 'lore init' first.",
                path.display()
            )
        })?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}

/// Default config file path: `$XDG_CONFIG_HOME/lore/lore.toml` or `~/.config/lore/lore.toml`.
pub fn default_config_path() -> anyhow::Result<PathBuf> {
    let xdg = xdg_strategy("config")?;
    Ok(xdg.config_dir().join("lore").join("lore.toml"))
}

/// Default database file path: `$XDG_DATA_HOME/lore/knowledge.db` or `~/.local/share/lore/knowledge.db`.
pub fn default_database_path() -> anyhow::Result<PathBuf> {
    let xdg = xdg_strategy("data")?;
    Ok(xdg.data_dir().join("lore").join("knowledge.db"))
}

/// Default trace directory: `$XDG_STATE_HOME/lore/traces` or `~/.local/state/lore/traces`.
///
/// Returns the directory that holds Track 2 Observability trace files. Individual
/// trace filenames are determined by the writer (per-session id); this helper
/// returns only the parent directory, with no trailing separator.
pub fn default_trace_dir() -> anyhow::Result<PathBuf> {
    let xdg = xdg_strategy("state")?;
    // `BaseStrategy::state_dir` returns `Option<PathBuf>` for trait
    // compatibility with non-XDG strategies; the `Xdg` implementation
    // always returns `Some`.
    let state = xdg.state_dir().expect("Xdg::state_dir always returns Some");
    Ok(state.join("lore").join("traces"))
}

/// Resolve the trace directory honouring the test-only `LORE_TRACE_DIR`
/// override. Single source of truth for `lore status`, the MCP
/// `lore_status` tool, the `lore trace { why, prune }` subcommands, and
/// the hook trace integration. Returning [`anyhow::Result`] mirrors
/// `default_trace_dir`'s contract — callers translate the `Err` shape
/// into their own discipline (`Option` for hook fire-and-forget,
/// propagation for CLI surfaces).
pub fn resolve_trace_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("LORE_TRACE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    default_trace_dir()
}

/// Construct an `Xdg` strategy, mapping the missing-`$HOME` case onto the
/// existing operator-facing wording. We pre-check `$HOME` ourselves because
/// `etcetera` (via `std::env::home_dir`) falls back to `/etc/passwd` when
/// `$HOME` is unset, which would silently produce a path instead of the
/// explicit `--config` recovery hint.
fn xdg_strategy(purpose: &str) -> anyhow::Result<Xdg> {
    if std::env::var_os("HOME").is_none_or(|v| v.is_empty()) {
        return Err(anyhow::anyhow!(
            "Cannot determine {purpose} directory: $HOME is not set. \
             Use --config to specify a path."
        ));
    }
    Xdg::new().map_err(|_| {
        anyhow::anyhow!(
            "Cannot determine {purpose} directory: $HOME is not set. \
             Use --config to specify a path."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Config {
        Config::default_with(
            PathBuf::from("docs"),
            PathBuf::from("lore.db"),
            "nomic-embed-text",
        )
    }

    #[test]
    fn round_trip_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.toml");

        let original = sample_config();
        original.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn default_values_are_sensible() {
        let config = sample_config();

        assert_eq!(config.bind, "localhost:3100");
        assert_eq!(config.ollama.host, "http://127.0.0.1:11434");
        assert_eq!(config.ollama.model, "nomic-embed-text");
        assert!(config.search.hybrid);
        assert_eq!(config.search.top_k, 5);
        assert!((config.search.min_relevance - 0.6).abs() < f64::EPSILON);
        assert_eq!(config.chunking.strategy, "heading");
        assert_eq!(config.chunking.max_tokens, 1024);
    }

    #[test]
    fn load_nonexistent_path_mentions_lore_init() {
        let result = Config::load(Path::new("/tmp/nonexistent/lore.toml"));
        let err = result.unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("lore init"),
            "error should mention 'lore init', got: {msg}"
        );
        assert!(
            msg.contains("/tmp/nonexistent/lore.toml"),
            "error should include the path, got: {msg}"
        );
    }

    // -- default_config_path / default_database_path (U2: etcetera swap) ----
    //
    // Each scenario pins one operator-reachable branch of the public
    // helpers, exercised through env mutation under `temp_env::with_vars`.

    #[test]
    fn default_config_path_uses_xdg_config_home_when_set() {
        temp_env::with_vars(
            [
                ("XDG_CONFIG_HOME", Some("/custom/config")),
                ("HOME", Some("/home/user")),
            ],
            || {
                let path = default_config_path().unwrap();
                assert_eq!(path, PathBuf::from("/custom/config/lore/lore.toml"));
            },
        );
    }

    #[test]
    fn default_database_path_uses_xdg_data_home_when_set() {
        temp_env::with_vars(
            [
                ("XDG_DATA_HOME", Some("/custom/data")),
                ("HOME", Some("/home/user")),
            ],
            || {
                let path = default_database_path().unwrap();
                assert_eq!(path, PathBuf::from("/custom/data/lore/knowledge.db"));
            },
        );
    }

    #[test]
    fn default_config_path_falls_back_to_home_when_xdg_unset() {
        temp_env::with_vars(
            [
                ("XDG_CONFIG_HOME", None::<&str>),
                ("HOME", Some("/home/user")),
            ],
            || {
                let path = default_config_path().unwrap();
                assert_eq!(path, PathBuf::from("/home/user/.config/lore/lore.toml"));
            },
        );
    }

    #[test]
    fn default_config_path_falls_back_to_home_when_xdg_empty() {
        // R-PR3 hard constraint: the empty-string branch must remain the
        // $HOME-based default. etcetera honours this via `is_absolute()`
        // — empty fails the absolute check, so the default applies.
        temp_env::with_vars(
            [("XDG_CONFIG_HOME", Some("")), ("HOME", Some("/home/user"))],
            || {
                let path = default_config_path().unwrap();
                assert_eq!(path, PathBuf::from("/home/user/.config/lore/lore.toml"));
            },
        );
    }

    #[test]
    fn default_config_path_home_unset_returns_error_mentioning_config() {
        temp_env::with_vars(
            [("XDG_CONFIG_HOME", None::<&str>), ("HOME", None::<&str>)],
            || {
                let result = default_config_path();
                let err = result.unwrap_err();
                let msg = err.to_string();
                assert!(
                    msg.contains("config"),
                    "error should mention config purpose, got: {msg}"
                );
                assert!(
                    msg.contains("$HOME is not set"),
                    "error should mention $HOME, got: {msg}"
                );
                assert!(
                    msg.contains("--config"),
                    "error should mention --config, got: {msg}"
                );
            },
        );
    }

    #[test]
    fn default_database_path_falls_back_to_home_local_share_when_xdg_unset() {
        temp_env::with_vars(
            [
                ("XDG_DATA_HOME", None::<&str>),
                ("HOME", Some("/home/user")),
            ],
            || {
                let path = default_database_path().unwrap();
                assert_eq!(
                    path,
                    PathBuf::from("/home/user/.local/share/lore/knowledge.db")
                );
            },
        );
    }

    #[test]
    fn default_database_path_home_unset_returns_error_mentioning_config() {
        // Symmetry with `default_config_path_home_unset_returns_error_mentioning_config`.
        // The legacy suite only covered the `config` purpose token; pinning
        // `data` here costs little and documents the contract for both
        // helpers explicitly.
        temp_env::with_vars(
            [("XDG_DATA_HOME", None::<&str>), ("HOME", None::<&str>)],
            || {
                let result = default_database_path();
                let err = result.unwrap_err();
                let msg = err.to_string();
                assert!(
                    msg.contains("data"),
                    "error should mention data purpose, got: {msg}"
                );
                assert!(
                    msg.contains("$HOME is not set"),
                    "error should mention $HOME, got: {msg}"
                );
                assert!(
                    msg.contains("--config"),
                    "error should mention --config, got: {msg}"
                );
            },
        );
    }

    #[test]
    fn round_trip_with_git_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.toml");

        let mut config = sample_config();
        config.git = Some(super::GitConfig {
            inbox_branch_prefix: "inbox/".to_string(),
        });
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);
    }

    #[test]
    fn loads_without_git_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.toml");

        let config = sample_config();
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert!(loaded.git.is_none());
    }

    #[test]
    fn inbox_branch_prefix_accessor() {
        let mut config = sample_config();
        assert_eq!(config.inbox_branch_prefix(), None);

        config.git = Some(super::GitConfig {
            inbox_branch_prefix: "inbox/".to_string(),
        });
        assert_eq!(config.inbox_branch_prefix(), Some("inbox/"));
    }

    // -- min_relevance_universal (U6) ----------------------------------------

    #[test]
    fn default_with_leaves_universal_floor_unset() {
        // AE6: a fresh install must match current behaviour exactly. The
        // override stays None and the accessor falls through to min_relevance.
        let config = sample_config();
        assert!(
            config.search.min_relevance_universal.is_none(),
            "default_with must leave min_relevance_universal unset"
        );
        assert!(
            (config.search.effective_min_relevance_universal() - 0.6).abs() < f64::EPSILON,
            "without override, accessor must return min_relevance default (0.6)"
        );
    }

    #[test]
    fn round_trip_without_universal_floor_inherits_min_relevance() {
        let toml_text = "\
            knowledge_dir = \"docs\"\n\
            database = \"lore.db\"\n\
            bind = \"localhost:3100\"\n\n\
            [ollama]\n\
            host = \"http://127.0.0.1:11434\"\n\
            model = \"nomic-embed-text\"\n\n\
            [search]\n\
            hybrid = true\n\
            top_k = 5\n\
            min_relevance = 0.6\n\n\
            [chunking]\n\
            strategy = \"heading\"\n\
            max_tokens = 1024\n";

        let cfg: Config = toml::from_str(toml_text).unwrap();
        assert!(
            cfg.search.min_relevance_universal.is_none(),
            "absent key must deserialise to None"
        );
        assert!(
            (cfg.search.effective_min_relevance_universal() - 0.6).abs() < f64::EPSILON,
            "accessor must inherit min_relevance when override absent"
        );
    }

    #[test]
    fn round_trip_with_universal_floor_returns_override() {
        let toml_text = "\
            knowledge_dir = \"docs\"\n\
            database = \"lore.db\"\n\
            bind = \"localhost:3100\"\n\n\
            [ollama]\n\
            host = \"http://127.0.0.1:11434\"\n\
            model = \"nomic-embed-text\"\n\n\
            [search]\n\
            hybrid = true\n\
            top_k = 5\n\
            min_relevance = 0.6\n\
            min_relevance_universal = 0.7\n\n\
            [chunking]\n\
            strategy = \"heading\"\n\
            max_tokens = 1024\n";

        let cfg: Config = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.search.min_relevance_universal, Some(0.7));
        assert!(
            (cfg.search.effective_min_relevance_universal() - 0.7).abs() < f64::EPSILON,
            "explicit override must take precedence over min_relevance"
        );
        // Non-universal floor is unchanged.
        assert!((cfg.search.min_relevance - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn accessor_tracks_min_relevance_when_override_absent() {
        // If only min_relevance is overridden, the universal floor follows
        // the new value (inherit-from-`min_relevance` semantics).
        let toml_text = "\
            knowledge_dir = \"docs\"\n\
            database = \"lore.db\"\n\
            bind = \"localhost:3100\"\n\n\
            [ollama]\n\
            host = \"http://127.0.0.1:11434\"\n\
            model = \"nomic-embed-text\"\n\n\
            [search]\n\
            hybrid = true\n\
            top_k = 5\n\
            min_relevance = 0.8\n\n\
            [chunking]\n\
            strategy = \"heading\"\n\
            max_tokens = 1024\n";

        let cfg: Config = toml::from_str(toml_text).unwrap();
        assert!(cfg.search.min_relevance_universal.is_none());
        assert!(
            (cfg.search.effective_min_relevance_universal() - 0.8).abs() < f64::EPSILON,
            "accessor must track raised min_relevance"
        );
    }

    #[test]
    fn save_and_load_preserves_universal_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.toml");

        let mut config = sample_config();
        config.search.min_relevance_universal = Some(0.75);
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.search.min_relevance_universal, Some(0.75));
        assert_eq!(loaded, config);
    }

    // -- default_trace_dir (U1: etcetera path resolution) -------------------
    //
    // These tests mutate the process environment via `temp_env`, which
    // serialises env access through a reentrant mutex so concurrent test
    // threads don't observe each other's overrides.

    #[test]
    fn default_trace_dir_uses_xdg_state_home_when_set() {
        temp_env::with_vars(
            [
                ("XDG_STATE_HOME", Some("/custom/state")),
                ("HOME", Some("/home/user")),
            ],
            || {
                let path = default_trace_dir().unwrap();
                assert_eq!(path, PathBuf::from("/custom/state/lore/traces"));
            },
        );
    }

    #[test]
    fn default_trace_dir_falls_back_to_home_when_xdg_unset() {
        temp_env::with_vars(
            [
                ("XDG_STATE_HOME", None::<&str>),
                ("HOME", Some("/home/user")),
            ],
            || {
                let path = default_trace_dir().unwrap();
                assert_eq!(path, PathBuf::from("/home/user/.local/state/lore/traces"));
            },
        );
    }

    #[test]
    fn default_trace_dir_falls_back_to_home_when_xdg_empty() {
        // R-PR3: empty XDG_*_HOME must fall back to the $HOME-based default.
        // etcetera honours this via an `is_absolute()` check on the env value
        // — an empty string is not absolute, so the default applies.
        temp_env::with_vars(
            [("XDG_STATE_HOME", Some("")), ("HOME", Some("/home/user"))],
            || {
                let path = default_trace_dir().unwrap();
                assert_eq!(path, PathBuf::from("/home/user/.local/state/lore/traces"));
            },
        );
    }

    // -- TraceConfig + trace_enabled (U1) ------------------------------------

    #[test]
    fn trace_config_defaults_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.toml");
        let config = sample_config();
        config.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert!(!loaded.trace.enabled);
        assert_eq!(loaded.trace.retain_days, 30);
        assert_eq!(loaded.trace.gzip_older_than_days, 7);
        assert!(!loaded.trace.include_full_command);
        assert!(!loaded.trace.include_transcript_tail);
    }

    #[test]
    fn trace_config_round_trip_with_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-config.toml");
        let mut config = sample_config();
        config.trace.enabled = true;
        config.trace.retain_days = 14;
        config.trace.gzip_older_than_days = 3;
        config.trace.include_full_command = true;
        config.trace.include_transcript_tail = true;
        config.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn trace_config_absent_section_deserialises_to_defaults() {
        let toml_text = "\
            knowledge_dir = \"docs\"\n\
            database = \"lore.db\"\n\
            bind = \"localhost:3100\"\n\n\
            [ollama]\n\
            host = \"http://127.0.0.1:11434\"\n\
            model = \"nomic-embed-text\"\n\n\
            [search]\n\
            hybrid = true\n\
            top_k = 5\n\
            min_relevance = 0.6\n\n\
            [chunking]\n\
            strategy = \"heading\"\n\
            max_tokens = 1024\n";
        let cfg: Config = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.trace, TraceConfig::default());
    }

    #[test]
    fn trace_config_empty_section_deserialises_to_defaults() {
        let toml_text = "\
            knowledge_dir = \"docs\"\n\
            database = \"lore.db\"\n\
            bind = \"localhost:3100\"\n\n\
            [ollama]\n\
            host = \"http://127.0.0.1:11434\"\n\
            model = \"nomic-embed-text\"\n\n\
            [search]\n\
            hybrid = true\n\
            top_k = 5\n\
            min_relevance = 0.6\n\n\
            [chunking]\n\
            strategy = \"heading\"\n\
            max_tokens = 1024\n\n\
            [trace]\n";
        let cfg: Config = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.trace, TraceConfig::default());
    }

    #[test]
    fn trace_enabled_env_var_overrides_config_off() {
        // AE1: env var wins over config when config is off.
        let mut config = sample_config();
        config.trace.enabled = false;
        temp_env::with_var("LORE_TRACE", Some("1"), || {
            assert!(config.trace_enabled());
        });
        temp_env::with_var("LORE_TRACE", Some("true"), || {
            assert!(config.trace_enabled());
        });
        temp_env::with_var("LORE_TRACE", Some("yes"), || {
            assert!(config.trace_enabled());
        });
    }

    #[test]
    fn trace_enabled_env_var_overrides_config_on() {
        // AE1: env var wins over config when config is on.
        let mut config = sample_config();
        config.trace.enabled = true;
        temp_env::with_var("LORE_TRACE", Some("0"), || {
            assert!(!config.trace_enabled());
        });
        temp_env::with_var("LORE_TRACE", Some("false"), || {
            assert!(!config.trace_enabled());
        });
        temp_env::with_var("LORE_TRACE", Some("no"), || {
            assert!(!config.trace_enabled());
        });
    }

    #[test]
    fn trace_enabled_falls_through_when_env_unset() {
        let mut config = sample_config();
        config.trace.enabled = false;
        temp_env::with_var("LORE_TRACE", None::<&str>, || {
            assert!(!config.trace_enabled());
        });
        config.trace.enabled = true;
        temp_env::with_var("LORE_TRACE", None::<&str>, || {
            assert!(config.trace_enabled());
        });
    }

    #[test]
    fn trace_enabled_falls_through_on_unrecognised_env_value() {
        // Mirrors LORE_DEBUG's fail-soft contract: unknown / wrongly-cased
        // / empty values are treated as unset, no warning emitted.
        let mut config = sample_config();
        config.trace.enabled = true;
        for raw in ["maybe", "TRUE", "True", "Yes", "ON", "", " 1", "1 "] {
            temp_env::with_var("LORE_TRACE", Some(raw), || {
                assert!(
                    config.trace_enabled(),
                    "value {raw:?} should fall through to config.trace.enabled=true"
                );
            });
        }
        config.trace.enabled = false;
        for raw in ["maybe", "TRUE", "FALSE", "Off", ""] {
            temp_env::with_var("LORE_TRACE", Some(raw), || {
                assert!(
                    !config.trace_enabled(),
                    "value {raw:?} should fall through to config.trace.enabled=false"
                );
            });
        }
    }

    #[test]
    fn default_trace_dir_home_unset_returns_error_mentioning_config() {
        temp_env::with_vars(
            [("XDG_STATE_HOME", None::<&str>), ("HOME", None::<&str>)],
            || {
                let result = default_trace_dir();
                let err = result.unwrap_err();
                let msg = err.to_string();
                assert!(
                    msg.contains("state"),
                    "error should mention state purpose, got: {msg}"
                );
                assert!(
                    msg.contains("$HOME is not set"),
                    "error should mention $HOME, got: {msg}"
                );
                assert!(
                    msg.contains("--config"),
                    "error should mention --config, got: {msg}"
                );
            },
        );
    }
}
