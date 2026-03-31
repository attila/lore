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

pub fn default_config_path() -> PathBuf {
    PathBuf::from("lore.toml")
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
    fn default_config_path_is_lore_toml() {
        assert_eq!(default_config_path(), PathBuf::from("lore.toml"));
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
}
