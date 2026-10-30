use std::path::{Path, PathBuf};

use etcetera::base_strategy::{BaseStrategy, Xdg};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub knowledge_dir: PathBuf,
    pub database: PathBuf,
    // TODO: evaluate TCP transport as alternative to stdio
    pub bind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingConfig>,
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
    #[serde(default, skip_serializing_if = "Dimensions::is_auto")]
    pub dimensions: Dimensions,
}

/// The `[embedding]` table: an OpenAI-compatible embedding server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
    pub provider: String,
    /// Base URL; requests go to `{url}/embeddings`.
    pub url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_environment_variable_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Dimensions::is_auto",
        deserialize_with = "embedding_dimensions"
    )]
    pub dimensions: Dimensions,
}

/// The only `[embedding] provider` lore takes.
pub const OPENAI_COMPATIBLE: &str = "openai-compatible";

/// The embedding server table a configuration uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmbeddingTable<'a> {
    Ollama(&'a OllamaConfig),
    OpenAiCompatible(&'a EmbeddingConfig),
}

impl<'a> EmbeddingTable<'a> {
    /// The table's name as a config file spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ollama(_) => "[ollama]",
            Self::OpenAiCompatible(_) => "[embedding]",
        }
    }

    /// The server's address: `[ollama] host` or `[embedding] url`.
    pub fn url(self) -> &'a str {
        match self {
            Self::Ollama(ollama) => &ollama.host,
            Self::OpenAiCompatible(embedding) => &embedding.url,
        }
    }

    pub fn model(self) -> &'a str {
        match self {
            Self::Ollama(ollama) => &ollama.model,
            Self::OpenAiCompatible(embedding) => &embedding.model,
        }
    }
}

/// Width of the configured model's vectors. `Auto` takes the width the
/// model returns; `Fixed` is a check against it, never sent to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dimensions {
    #[default]
    Auto,
    Fixed(usize),
}

impl Dimensions {
    /// The width the vector table must have, when the config sets one.
    pub fn fixed(self) -> Option<usize> {
        match self {
            Self::Auto => None,
            Self::Fixed(width) => Some(width),
        }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

impl Serialize for Dimensions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Fixed(width) => serializer.serialize_u64(*width as u64),
        }
    }
}

impl<'de> Deserialize<'de> for Dimensions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DimensionsVisitor("[ollama]"))
    }
}

fn embedding_dimensions<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Dimensions, D::Error> {
    deserializer.deserialize_any(DimensionsVisitor("[embedding]"))
}

/// Reads `dimensions` in the table it names.
struct DimensionsVisitor(&'static str);

impl serde::de::Visitor<'_> for DimensionsVisitor {
    type Value = Dimensions;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\"auto\" or a positive integer for {} dimensions",
            self.0
        )
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Dimensions, E> {
        if value == "auto" {
            Ok(Dimensions::Auto)
        } else {
            Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
        }
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Dimensions, E> {
        usize::try_from(value)
            .ok()
            .filter(|width| *width > 0)
            .map(Dimensions::Fixed)
            .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Signed(value), &self))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Dimensions, E> {
        usize::try_from(value)
            .ok()
            .filter(|width| *width > 0)
            .map(Dimensions::Fixed)
            .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Unsigned(value), &self))
    }
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
            ollama: Some(OllamaConfig {
                host: "http://127.0.0.1:11434".to_string(),
                model: model.to_string(),
                dimensions: Dimensions::Auto,
            }),
            embedding: None,
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
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
                "Config not found at {}. Run 'lore init' first.",
                path.display()
            ),
            Err(e) => anyhow::bail!("Cannot read config at {}: {e}", path.display()),
        };
        let config: Self = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Cannot parse config at {}: {e}", path.display()))?;
        config.check_embedding_table(path)?;
        Ok(config)
    }

    /// Refuse an embedding server table no request could work with. The
    /// messages never echo a url, because it may hold a key.
    fn check_embedding_table(&self, path: &Path) -> anyhow::Result<()> {
        let refuse =
            |reason: &str| anyhow::anyhow!("Cannot use config at {}: {reason}", path.display());
        match (&self.embedding, &self.ollama) {
            (None, None) => Err(refuse(
                "it has neither an [ollama] nor an [embedding] table. Add one; \
                 docs/configuration.md describes both.",
            )),
            (Some(embedding), _) => {
                if embedding.provider != OPENAI_COMPATIBLE {
                    return Err(refuse(&format!(
                        "[embedding] provider is \"{}\", and the only provider it takes is \
                         \"{OPENAI_COMPATIBLE}\". Set up Ollama in an [ollama] table instead.",
                        embedding.provider
                    )));
                }
                let scheme = embedding.url.to_ascii_lowercase();
                if !(scheme.starts_with("http://") || scheme.starts_with("https://")) {
                    return Err(refuse(
                        "[embedding] url has no http:// or https:// scheme. Start it with one \
                         of them.",
                    ));
                }
                if embedding.url.contains(['?', '#']) {
                    return Err(refuse(
                        "[embedding] url has a query or a fragment, and lore adds /embeddings \
                         to the url's path. Remove everything from the first \"?\" or \"#\".",
                    ));
                }
                Ok(())
            }
            (None, Some(ollama)) => {
                if ollama.host.contains(['?', '#']) {
                    return Err(refuse(
                        "[ollama] host has a query or a fragment, and lore adds /api/embed to \
                         the host's path. Remove everything from the first \"?\" or \"#\".",
                    ));
                }
                Ok(())
            }
        }
    }

    /// The embedding server table lore uses: `[embedding]` when the file has
    /// one, else `[ollama]`.
    ///
    /// # Panics
    ///
    /// On a config with neither table, which [`Config::load`] refuses.
    pub fn embedding_table(&self) -> EmbeddingTable<'_> {
        match &self.embedding {
            Some(embedding) => EmbeddingTable::OpenAiCompatible(embedding),
            None => EmbeddingTable::Ollama(
                self.ollama
                    .as_ref()
                    .expect("Config::load refuses a config with neither table"),
            ),
        }
    }

    /// The warning for a file with both tables, read from `path`.
    pub fn both_tables_warning(&self, path: &Path) -> Option<String> {
        (self.embedding.is_some() && self.ollama.is_some()).then(|| {
            format!(
                "Warning: {} has both an [embedding] and an [ollama] table. Lore uses \
                 [embedding] and ignores [ollama]. Remove the table you do not use.",
                path.display()
            )
        })
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

/// Resolve the trace directory used by `lore status`, the MCP
/// `lore_status` tool, the `lore trace { why, prune }` subcommands, and
/// the hook trace integration. Returning [`anyhow::Result`] mirrors
/// `default_trace_dir`'s contract — callers translate the `Err` shape
/// into their own discipline (`Option` for hook fire-and-forget,
/// propagation for CLI surfaces).
///
/// **Test-only override:** under `cfg(debug_assertions)` (i.e. dev and
/// `cargo test` builds), `LORE_TRACE_DIR` short-circuits the XDG lookup.
/// Release builds ignore the variable so operators cannot accidentally
/// rely on it; the plan's "trace directory is XDG-state-only" decision
/// is preserved on the production surface. Per-environment redirection
/// is available via `XDG_STATE_HOME` in either build mode.
pub fn resolve_trace_dir() -> anyhow::Result<PathBuf> {
    #[cfg(debug_assertions)]
    {
        if let Some(dir) = std::env::var_os("LORE_TRACE_DIR") {
            return Ok(PathBuf::from(dir));
        }
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
        let ollama = config.ollama.as_ref().unwrap();
        assert_eq!(ollama.host, "http://127.0.0.1:11434");
        assert_eq!(ollama.model, "nomic-embed-text");
        assert_eq!(config.embedding, None);
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

    #[test]
    fn load_malformed_file_names_it_and_the_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        std::fs::write(&path, "knowledge_dir = [unclosed\n").unwrap();

        let msg = Config::load(&path).unwrap_err().to_string();

        assert!(
            msg.contains(&format!("Cannot parse config at {}", path.display())),
            "{msg}"
        );
        assert!(msg.contains("TOML parse error"), "{msg}");
        assert!(!msg.contains("lore init"), "{msg}");
    }

    #[test]
    fn load_directory_at_path_names_it_and_the_os_error() {
        let dir = tempfile::tempdir().unwrap();

        let msg = Config::load(dir.path()).unwrap_err().to_string();

        assert!(
            msg.contains(&format!("Cannot read config at {}: ", dir.path().display())),
            "{msg}"
        );
        assert!(msg.contains("os error"), "{msg}");
        assert!(!msg.contains("lore init"), "{msg}");
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

    // -- [ollama] dimensions --------------------------------------------------

    const SAVED_DEFAULT: &str = "\
knowledge_dir = \"docs\"
database = \"lore.db\"
bind = \"localhost:3100\"

[ollama]
host = \"http://127.0.0.1:11434\"
model = \"nomic-embed-text\"

[search]
hybrid = true
top_k = 5
min_relevance = 0.6

[chunking]
strategy = \"heading\"
max_tokens = 1024

[trace]
enabled = false
retain_days = 30
gzip_older_than_days = 7
include_full_command = false
include_transcript_tail = false
";

    fn with_dimensions(line: &str) -> String {
        SAVED_DEFAULT.replace(
            "model = \"nomic-embed-text\"\n",
            &format!("model = \"nomic-embed-text\"\n{line}\n"),
        )
    }

    #[test]
    fn a_config_without_dimensions_loads_unchanged_and_saves_without_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        std::fs::write(&path, SAVED_DEFAULT).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, sample_config());
        assert_eq!(loaded.ollama.as_ref().unwrap().dimensions, Dimensions::Auto);

        loaded.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAVED_DEFAULT);
    }

    #[test]
    fn dimensions_auto_reads_as_absent() {
        let cfg: Config = toml::from_str(&with_dimensions("dimensions = \"auto\"")).unwrap();
        assert_eq!(cfg, sample_config());
        assert_eq!(cfg.ollama.unwrap().dimensions.fixed(), None);
    }

    #[test]
    fn dimensions_integer_is_a_fixed_width_that_survives_a_save() {
        let text = with_dimensions("dimensions = 1024");
        let cfg: Config = toml::from_str(&text).unwrap();
        let dimensions = cfg.ollama.as_ref().unwrap().dimensions;
        assert_eq!(dimensions, Dimensions::Fixed(1024));
        assert_eq!(dimensions.fixed(), Some(1024));
        assert_eq!(toml::to_string_pretty(&cfg).unwrap(), text);
    }

    #[test]
    fn dimensions_other_than_auto_or_a_positive_integer_is_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        for value in ["0", "-768", "\"wide\"", "\"AUTO\"", "768.0", "true"] {
            std::fs::write(&path, with_dimensions(&format!("dimensions = {value}"))).unwrap();
            let msg = format!("{:#}", Config::load(&path).unwrap_err());
            assert!(
                msg.contains("\"auto\" or a positive integer for [ollama] dimensions"),
                "value {value}: {msg}"
            );
        }
    }

    // -- [embedding] ----------------------------------------------------------

    const OLLAMA_TABLE: &str = "\
[ollama]
host = \"http://127.0.0.1:11434\"
model = \"nomic-embed-text\"
";

    const EMBEDDING_TABLE: &str = "\
[embedding]
provider = \"openai-compatible\"
url = \"http://127.0.0.1:8080/v1\"
model = \"nomic-embed-text-v1.5\"
api_key_environment_variable_name = \"LLAMA_API_KEY\"
dimensions = 768
";

    /// `SAVED_DEFAULT` with its `[ollama]` table replaced by `tables`.
    fn with_tables(tables: &str) -> String {
        SAVED_DEFAULT.replace(OLLAMA_TABLE, tables)
    }

    /// The message `Config::load` fails with for a file holding `tables`.
    fn load_error(tables: &str) -> (String, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        std::fs::write(&path, with_tables(tables)).unwrap();
        let msg = format!("{:#}", Config::load(&path).unwrap_err());
        (msg, path.display().to_string())
    }

    #[test]
    fn an_embedding_table_loads_and_saves_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        let text = with_tables(EMBEDDING_TABLE);
        std::fs::write(&path, &text).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.ollama, None);
        let table = loaded.embedding_table();
        assert!(matches!(table, EmbeddingTable::OpenAiCompatible(_)));
        assert_eq!(table.name(), "[embedding]");
        assert_eq!(table.url(), "http://127.0.0.1:8080/v1");
        assert_eq!(table.model(), "nomic-embed-text-v1.5");
        let embedding = loaded.embedding.as_ref().unwrap();
        assert_eq!(
            embedding.api_key_environment_variable_name.as_deref(),
            Some("LLAMA_API_KEY")
        );
        assert_eq!(embedding.dimensions, Dimensions::Fixed(768));
        assert_eq!(loaded.both_tables_warning(&path), None);

        loaded.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn an_ollama_table_is_the_one_used_without_an_embedding_table() {
        let config = sample_config();
        let table = config.embedding_table();
        assert!(matches!(table, EmbeddingTable::Ollama(_)));
        assert_eq!(table.name(), "[ollama]");
        assert_eq!(table.url(), "http://127.0.0.1:11434");
        assert_eq!(table.model(), "nomic-embed-text");
        assert!(
            !toml::to_string_pretty(&config)
                .unwrap()
                .contains("embedding")
        );
    }

    #[test]
    fn an_unknown_embedding_key_is_a_parse_error() {
        let (msg, path) = load_error(&EMBEDDING_TABLE.replace("dimensions", "api_key"));
        assert!(
            msg.starts_with(&format!("Cannot parse config at {path}: ")),
            "{msg}"
        );
        assert!(msg.contains("unknown field `api_key`"), "{msg}");
    }

    #[test]
    fn an_embedding_provider_other_than_openai_compatible_is_refused() {
        let (msg, path) = load_error(&EMBEDDING_TABLE.replace("openai-compatible", "ollama"));
        assert_eq!(
            msg,
            format!(
                "Cannot use config at {path}: [embedding] provider is \"ollama\", and the only \
                 provider it takes is \"openai-compatible\". Set up Ollama in an [ollama] table \
                 instead."
            )
        );
    }

    #[test]
    fn a_config_without_an_embedding_server_table_is_refused() {
        let (msg, path) = load_error("");
        assert_eq!(
            msg,
            format!(
                "Cannot use config at {path}: it has neither an [ollama] nor an [embedding] \
                 table. Add one; docs/configuration.md describes both."
            )
        );
    }

    #[test]
    fn an_embedding_url_lore_cannot_extend_is_refused_without_echoing_it() {
        const QUERY: &str = "[embedding] url has a query or a fragment, and lore adds \
                             /embeddings to the url's path. Remove everything from the first \
                             \"?\" or \"#\".";
        const SCHEME: &str =
            "[embedding] url has no http:// or https:// scheme. Start it with one of them.";
        for (url, reason) in [
            ("http://h/v1?key=secret", QUERY),
            ("https://h/v1#secret", QUERY),
            ("h:8080/secret", SCHEME),
            ("ftp://h/secret", SCHEME),
        ] {
            let (msg, path) = load_error(&EMBEDDING_TABLE.replace("http://127.0.0.1:8080/v1", url));
            assert_eq!(
                msg,
                format!("Cannot use config at {path}: {reason}"),
                "{url}"
            );
            assert!(!msg.contains("secret"), "{msg}");
        }
        let upper = EMBEDDING_TABLE.replace("http://", "HTTPS://");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        std::fs::write(&path, with_tables(&upper)).unwrap();
        assert!(Config::load(&path).is_ok());
    }

    #[test]
    fn an_ollama_host_with_a_query_or_a_fragment_is_refused() {
        for host in ["http://127.0.0.1:11434?a=b", "http://127.0.0.1:11434/#x"] {
            let (msg, path) = load_error(&OLLAMA_TABLE.replace("http://127.0.0.1:11434", host));
            assert_eq!(
                msg,
                format!(
                    "Cannot use config at {path}: [ollama] host has a query or a fragment, and \
                     lore adds /api/embed to the host's path. Remove everything from the first \
                     \"?\" or \"#\"."
                )
            );
        }
    }

    #[test]
    fn embedding_dimensions_other_than_auto_or_a_positive_integer_name_the_embedding_table() {
        let (msg, _) = load_error(&EMBEDDING_TABLE.replace("768", "0"));
        assert!(
            msg.contains("\"auto\" or a positive integer for [embedding] dimensions"),
            "{msg}"
        );
    }

    #[test]
    fn a_file_with_both_tables_uses_embedding_and_warns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lore.toml");
        std::fs::write(
            &path,
            with_tables(&format!("{OLLAMA_TABLE}\n{EMBEDDING_TABLE}")),
        )
        .unwrap();

        let loaded = Config::load(&path).unwrap();
        assert!(matches!(
            loaded.embedding_table(),
            EmbeddingTable::OpenAiCompatible(_)
        ));
        assert_eq!(
            loaded.both_tables_warning(&path).unwrap(),
            format!(
                "Warning: {} has both an [embedding] and an [ollama] table. Lore uses \
                 [embedding] and ignores [ollama]. Remove the table you do not use.",
                path.display()
            )
        );
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
