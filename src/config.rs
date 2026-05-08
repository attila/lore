use std::path::{Path, PathBuf};

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

/// Resolve the XDG base directory for the given variable, falling back to `$HOME/<subpath>`.
fn resolve_xdg_base(
    xdg_value: Option<String>,
    home_value: Option<String>,
    home_subpath: &str,
    purpose: &str,
) -> anyhow::Result<PathBuf> {
    if let Some(val) = xdg_value
        && !val.is_empty()
    {
        return Ok(PathBuf::from(val));
    }
    let home = home_value.ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine {purpose} directory: $HOME is not set. \
             Use --config to specify a path."
        )
    })?;
    Ok(PathBuf::from(home).join(home_subpath))
}

/// Default config file path: `$XDG_CONFIG_HOME/lore/lore.toml` or `~/.config/lore/lore.toml`.
pub fn default_config_path() -> anyhow::Result<PathBuf> {
    let dir = resolve_xdg_base(
        std::env::var("XDG_CONFIG_HOME").ok(),
        std::env::var("HOME").ok(),
        ".config",
        "config",
    )?;
    Ok(dir.join("lore").join("lore.toml"))
}

/// Default database file path: `$XDG_DATA_HOME/lore/knowledge.db` or `~/.local/share/lore/knowledge.db`.
pub fn default_database_path() -> anyhow::Result<PathBuf> {
    let dir = resolve_xdg_base(
        std::env::var("XDG_DATA_HOME").ok(),
        std::env::var("HOME").ok(),
        ".local/share",
        "data",
    )?;
    Ok(dir.join("lore").join("knowledge.db"))
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

    #[test]
    fn xdg_config_home_set() {
        let path = resolve_xdg_base(
            Some("/custom/config".to_string()),
            Some("/home/user".to_string()),
            ".config",
            "config",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/custom/config"));
    }

    #[test]
    fn xdg_data_home_set() {
        let path = resolve_xdg_base(
            Some("/custom/data".to_string()),
            Some("/home/user".to_string()),
            ".local/share",
            "data",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/custom/data"));
    }

    #[test]
    fn xdg_var_unset_falls_back_to_home() {
        let path =
            resolve_xdg_base(None, Some("/home/user".to_string()), ".config", "config").unwrap();
        assert_eq!(path, PathBuf::from("/home/user/.config"));
    }

    #[test]
    fn xdg_var_empty_falls_back_to_home() {
        let path = resolve_xdg_base(
            Some(String::new()),
            Some("/home/user".to_string()),
            ".config",
            "config",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/.config"));
    }

    #[test]
    fn home_unset_returns_error() {
        let result = resolve_xdg_base(None, None, ".config", "config");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("$HOME is not set"),
            "error should mention $HOME, got: {msg}"
        );
        assert!(
            msg.contains("--config"),
            "error should mention --config, got: {msg}"
        );
    }

    #[test]
    fn xdg_data_falls_back_to_home_local_share() {
        let path =
            resolve_xdg_base(None, Some("/home/user".to_string()), ".local/share", "data").unwrap();
        assert_eq!(path, PathBuf::from("/home/user/.local/share"));
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
}
