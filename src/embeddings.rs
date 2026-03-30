use std::io::BufRead;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Trait for producing embedding vectors from text.
pub trait Embedder {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>>;
    fn dimensions(&self) -> usize;
}

/// Client for the Ollama embedding API.
pub struct OllamaClient {
    host: String,
    model: String,
    agent: ureq::Agent,
}

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Serialize)]
struct ShowRequest {
    name: String,
}

#[derive(Serialize)]
struct PullRequest {
    name: String,
    stream: bool,
}

#[derive(Deserialize)]
struct PullProgress {
    status: Option<String>,
}

impl OllamaClient {
    /// Creates a new `OllamaClient` with the given Ollama host URL and model name.
    pub fn new(host: &str, model: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        let agent = ureq::Agent::new_with_config(config);

        Self {
            host: host.to_string(),
            model: model.to_string(),
            agent,
        }
    }

    /// Returns `true` if the Ollama server is reachable.
    pub fn is_healthy(&self) -> bool {
        self.agent.get(&self.host).call().is_ok()
    }

    /// Returns `true` if the configured model is available on the server.
    pub fn has_model(&self) -> bool {
        let url = format!("{}/api/show", self.host);
        let req = ShowRequest {
            name: self.model.clone(),
        };
        self.agent.post(&url).send_json(&req).is_ok()
    }

    /// Pulls the configured model from Ollama, reporting progress via callback.
    pub fn pull_model(&self, on_progress: &dyn Fn(&str)) -> anyhow::Result<()> {
        let url = format!("{}/api/pull", self.host);
        let req = PullRequest {
            name: self.model.clone(),
            stream: true,
        };

        let mut resp = self.agent.post(&url).send_json(&req)?;
        let reader = std::io::BufReader::new(resp.body_mut().as_reader());

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<PullProgress>(&line) {
                if let Some(status) = msg.status {
                    on_progress(&status);
                }
            }
        }

        Ok(())
    }

    /// Returns the configured model name.
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

impl Embedder for OllamaClient {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        let url = format!("{}/api/embed", self.host);
        let req = EmbedRequest {
            model: self.model.clone(),
            input: vec![input.to_string()],
        };

        let resp: EmbedResponse = self
            .agent
            .post(&url)
            .send_json(&req)?
            .body_mut()
            .read_json()?;

        resp.embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
    }

    fn dimensions(&self) -> usize {
        match self.model.as_str() {
            "mxbai-embed-large" | "snowflake-arctic-embed2" => 1024,
            "all-minilm" => 384,
            // nomic-embed-text and anything unrecognized default to 768
            _ => 768,
        }
    }
}

/// A deterministic fake embedder for use in tests.
///
/// Produces vectors of a fixed dimensionality (default 768) seeded by a simple
/// hash of the input string, so the same input always yields the same vector.
#[cfg(test)]
pub(crate) struct FakeEmbedder {
    dims: usize,
}

#[cfg(test)]
impl FakeEmbedder {
    pub fn new() -> Self {
        Self { dims: 768 }
    }

    #[allow(dead_code)]
    pub fn with_dimensions(dims: usize) -> Self {
        Self { dims }
    }
}

#[cfg(test)]
impl Default for FakeEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Embedder for FakeEmbedder {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        // Simple FNV-1a-inspired hash to produce a deterministic seed.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in input.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }

        let mut vec = Vec::with_capacity(self.dims);
        let mut state = hash;
        for _ in 0..self.dims {
            // Xorshift64 to produce pseudo-random sequence from the seed.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Normalize to [-1, 1] range.
            #[allow(clippy::cast_precision_loss)]
            let val = (state as f32) / (u64::MAX as f32) * 2.0 - 1.0;
            vec.push(val);
        }
        Ok(vec)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_embedder_returns_correct_length() {
        let embedder = FakeEmbedder::new();
        let vec = embedder.embed("hello world").unwrap();
        assert_eq!(vec.len(), 768);
    }

    #[test]
    fn fake_embedder_consistent_for_same_input() {
        let embedder = FakeEmbedder::new();
        let v1 = embedder.embed("hello world").unwrap();
        let v2 = embedder.embed("hello world").unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn fake_embedder_different_for_different_inputs() {
        let embedder = FakeEmbedder::new();
        let v1 = embedder.embed("hello world").unwrap();
        let v2 = embedder.embed("goodbye world").unwrap();
        assert_ne!(v1, v2);
    }

    #[test]
    fn dimensions_returns_correct_values() {
        let client = OllamaClient::new("http://localhost:11434", "nomic-embed-text");
        assert_eq!(client.dimensions(), 768);

        let client = OllamaClient::new("http://localhost:11434", "mxbai-embed-large");
        assert_eq!(client.dimensions(), 1024);

        let client = OllamaClient::new("http://localhost:11434", "all-minilm");
        assert_eq!(client.dimensions(), 384);

        let client = OllamaClient::new("http://localhost:11434", "snowflake-arctic-embed2");
        assert_eq!(client.dimensions(), 1024);

        let client = OllamaClient::new("http://localhost:11434", "unknown-model");
        assert_eq!(client.dimensions(), 768);
    }

    #[test]
    fn fake_embedder_custom_dimensions() {
        let embedder = FakeEmbedder::with_dimensions(384);
        let vec = embedder.embed("test").unwrap();
        assert_eq!(vec.len(), 384);
        assert_eq!(embedder.dimensions(), 384);
    }
}
