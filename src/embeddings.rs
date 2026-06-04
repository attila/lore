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
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<u64>,
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

/// Progress update from Ollama's `/api/pull` NDJSON stream.
#[derive(Debug, Deserialize)]
pub struct PullProgress {
    /// Human-readable status (e.g. "pulling sha256:abc...", "verifying").
    pub status: Option<String>,
    /// Total bytes to download for the current layer.
    pub total: Option<u64>,
    /// Bytes downloaded so far for the current layer.
    pub completed: Option<u64>,
}

/// Structured error from a runtime probe of the embedding endpoint.
///
/// Variants distinguish failure modes that need different remediation —
/// runner-failed vs timeout vs transport — so a slow disk does not get
/// misdiagnosed as a runner-bundle bug.
#[derive(Debug, Clone)]
pub enum ProbeError {
    /// HTTP 5xx from Ollama: the inference call reached the server but the
    /// runner subprocess failed. Verified shape for the broken-Homebrew
    /// runner-bundle case.
    RunnerFailed { status: u16, body: String },
    /// HTTP 200 with an `error` field in the body. Some Ollama failure modes
    /// use this shape on `/api/chat` and `/api/generate`; coverage here is
    /// defensive — the broken-Homebrew case fires `RunnerFailed`, not this.
    InferenceError { body: String },
    /// Request timed out — `ureq::Error::Timeout`.
    Timeout,
    /// Catchall for ureq transport errors (`Io`, `ConnectionFailed`,
    /// `HostNotFound`, `Decompress`, ...). Future `#[non_exhaustive]` variants
    /// fall through here without structural change.
    Transport(String),
    /// Non-2xx, non-5xx HTTP — rare; catches misconfigured proxies.
    HttpStatus { status: u16, body: String },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rendered = render_failure(self);
        write!(f, "{}", rendered.error_line)
    }
}

impl std::error::Error for ProbeError {}

/// Human-facing strings derived from a `ProbeError`, one per user-facing
/// surface. Centralising the variant→strings match in `render_failure` keeps
/// the four surfaces (status line, provision errors/actions, hook warning)
/// in sync when the enum gains a variant.
#[derive(Debug, Clone)]
pub struct RenderedFailure {
    /// Hook FTS-fallback warning short reason (e.g. `"inference error"`).
    pub short_reason: String,
    /// `lore status --full` Runtime-line body (after the `✗` prefix).
    pub status_line: String,
    /// `ProvisionResult::errors` entry — what went wrong.
    pub error_line: String,
    /// `ProvisionResult::actions` entry — what the user can do about it.
    pub action_line: String,
}

/// Render a `ProbeError` into the human-facing strings used across surfaces.
///
/// Adding a `ProbeError` variant requires updating exactly this match arm
/// (and the classification in `classify_embed_response` / `classify_ureq_error`)
/// for all surfaces to render it.
pub fn render_failure(err: &ProbeError) -> RenderedFailure {
    match err {
        ProbeError::RunnerFailed { body, .. } | ProbeError::InferenceError { body } => {
            let body_msg = extract_body_message(body);
            let body_suffix = body_msg
                .as_deref()
                .map(|m| format!(" — {m}"))
                .unwrap_or_default();
            RenderedFailure {
                short_reason: "inference error".to_string(),
                status_line: format!("inference failed{body_suffix}"),
                error_line: format!(
                    "Ollama reached the model but the inference call failed{body_suffix}"
                ),
                action_line: "Check 'ollama serve' logs for the underlying cause. \
                              Common cases: (1) Homebrew-installed Ollama with a broken \
                              runner bundle — try 'brew reinstall ollama'; (2) disk space \
                              exhausted; (3) GPU/Metal initialisation failure in a headless \
                              or sandboxed environment. The Ollama log line usually starts \
                              with 'llama runner process'."
                    .to_string(),
            }
        }
        ProbeError::Timeout => RenderedFailure {
            short_reason: "timed out".to_string(),
            status_line: "inference timed out (>30s) — check 'ollama serve' is healthy"
                .to_string(),
            error_line: "Ollama inference call timed out (>30s)".to_string(),
            action_line: "Check 'ollama serve' is healthy and the model isn't loading \
                          from cold storage. If the host is responsive, the runner may be \
                          hung — restart Ollama."
                .to_string(),
        },
        ProbeError::Transport(msg) => RenderedFailure {
            short_reason: "transport error".to_string(),
            status_line: format!("transport error — {msg}"),
            error_line: format!("Could not reach Ollama for inference: {msg}"),
            action_line: "Verify Ollama is running and reachable at the configured host."
                .to_string(),
        },
        ProbeError::HttpStatus { status, .. } => RenderedFailure {
            short_reason: format!("HTTP {status}"),
            status_line: format!("unexpected HTTP {status}"),
            error_line: format!("Ollama returned unexpected HTTP {status}"),
            action_line: "Check whether a proxy or middleware is intercepting the request. \
                          Direct access to Ollama should never return this status."
                .to_string(),
        },
    }
}

/// Send a POST that expects an `/api/embed`-shaped response and classify the
/// outcome.
///
/// Returns the raw response body as a string on HTTP 2xx; returns a structured
/// `ProbeError` on transport failures, timeouts, or non-2xx responses.
///
/// Implements the ureq 3.x mechanics required by both `probe` and
/// `Embedder::embed`:
/// 1. `send_json` returns `Ok(Response)` for HTTP 5xx in ureq 3.x — it does
///    not short-circuit through `?`.
/// 2. Check `response.status()` before treating the response as success.
/// 3. Read the body as text on non-2xx so the real diagnostic surfaces
///    instead of being swallowed by a deserialisation error.
pub fn classify_embed_response(
    resp_result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<String, ProbeError> {
    let mut resp = match resp_result {
        Ok(r) => r,
        Err(e) => return Err(classify_ureq_error(&e)),
    };

    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().unwrap_or_default();

    if (200..300).contains(&status) {
        Ok(body)
    } else if (500..600).contains(&status) {
        Err(ProbeError::RunnerFailed { status, body })
    } else {
        Err(ProbeError::HttpStatus { status, body })
    }
}

fn classify_ureq_error(e: &ureq::Error) -> ProbeError {
    if matches!(e, ureq::Error::Timeout(_)) {
        ProbeError::Timeout
    } else {
        ProbeError::Transport(e.to_string())
    }
}

/// Body shape used to detect HTTP-200-with-error-field responses from
/// `/api/embed`. Lives at file scope so both `OllamaClient::probe` and
/// `Embedder::embed` use the same definition without re-declaring it inside
/// function bodies.
#[derive(Deserialize)]
struct EmbedErrorBody {
    error: Option<String>,
}

/// Extract a one-line human-facing message from an arbitrary response body.
///
/// 1. JSON `{ "error": "..." }` → use the error field.
/// 2. Otherwise the first non-empty line of the trimmed body.
/// 3. Control characters replaced with spaces; trimmed.
/// 4. Truncated to 80 characters with `…` if longer.
/// 5. Empty → `None` (caller falls back to a variant-specific string without
///    the dash-body suffix).
fn extract_body_message(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: String,
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(parsed) = serde_json::from_str::<ErrorBody>(trimmed) {
        let cleaned = clean_for_one_line(&parsed.error);
        if !cleaned.is_empty() {
            return Some(truncate_chars(&cleaned, 80));
        }
    }

    let line = trimmed.lines().find(|l| !l.trim().is_empty())?;
    let cleaned = clean_for_one_line(line);
    if cleaned.is_empty() {
        None
    } else {
        Some(truncate_chars(&cleaned, 80))
    }
}

fn clean_for_one_line(s: &str) -> String {
    let replaced: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    replaced.trim().to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
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
    pub fn pull_model(&self, on_progress: &dyn Fn(&PullProgress)) -> anyhow::Result<()> {
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
                on_progress(&msg);
            }
        }

        Ok(())
    }

    /// Returns the configured model name.
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// Verify that Ollama can actually run inference on the configured model.
    ///
    /// Unlike `is_healthy` / `has_model`, this exercises the runner subprocess
    /// — catches breakage where the model manifest is on disk but the runner
    /// is missing or unloadable (e.g. the broken Homebrew bottle).
    ///
    /// `keep_alive_secs` controls how long Ollama keeps the model resident
    /// after this call:
    /// - `None`: Ollama's default (5 minutes). Caller intends to do more work
    ///   soon — typical for `provision()` ahead of ingest.
    /// - `Some(0)`: unload immediately after the call. Caller is just probing
    ///   — typical for `lore status --full`.
    /// - `Some(n)`: keep loaded for `n` seconds.
    pub fn probe(&self, keep_alive_secs: Option<u64>) -> Result<(), ProbeError> {
        let url = format!("{}/api/embed", self.host);
        let req = EmbedRequest {
            model: self.model.clone(),
            input: vec![".".to_string()],
            keep_alive: keep_alive_secs,
        };

        let body = classify_embed_response(self.agent.post(&url).send_json(&req))?;

        // 2xx — check for HTTP-200-with-error-field shape.
        if let Ok(parsed) = serde_json::from_str::<EmbedErrorBody>(&body)
            && parsed.error.is_some()
        {
            return Err(ProbeError::InferenceError { body });
        }

        Ok(())
    }
}

impl Embedder for OllamaClient {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        let url = format!("{}/api/embed", self.host);
        let req = EmbedRequest {
            model: self.model.clone(),
            input: vec![input.to_string()],
            keep_alive: None,
        };

        // Use the shared ureq-3.x-aware classifier so HTTP 5xx (broken-runner
        // case) produces a structured `ProbeError` instead of an opaque serde
        // failure from `read_json` parsing the error body as `EmbedResponse`.
        // The hook FTS-fallback path downcasts to `ProbeError` to classify
        // the warning.
        let body = classify_embed_response(self.agent.post(&url).send_json(&req))
            .map_err(anyhow::Error::new)?;

        // 2xx with `error` field — surface as inference error.
        if let Ok(parsed) = serde_json::from_str::<EmbedErrorBody>(&body)
            && parsed.error.is_some()
        {
            return Err(anyhow::Error::new(ProbeError::InferenceError { body }));
        }

        let resp: EmbedResponse = serde_json::from_str(&body)?;
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
#[cfg(any(test, feature = "test-support"))]
pub struct FakeEmbedder {
    dims: usize,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeEmbedder {
    pub fn new() -> Self {
        Self { dims: 768 }
    }

    #[allow(dead_code)]
    pub fn with_dimensions(dims: usize) -> Self {
        Self { dims }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for FakeEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
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

/// An embedder that always fails. Used to test the search fallback path
/// when Ollama is unreachable.
#[cfg(any(test, feature = "test-support"))]
pub struct FailingEmbedder {
    dims: usize,
}

#[cfg(any(test, feature = "test-support"))]
impl FailingEmbedder {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Embedder for FailingEmbedder {
    fn embed(&self, _input: &str) -> anyhow::Result<Vec<f32>> {
        anyhow::bail!("Ollama is unreachable")
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

    #[test]
    fn pull_progress_deserializes_all_fields() {
        let json = r#"{"status":"pulling sha256:abc","total":274000000,"completed":142000000}"#;
        let p: PullProgress = serde_json::from_str(json).unwrap();
        assert_eq!(p.status.as_deref(), Some("pulling sha256:abc"));
        assert_eq!(p.total, Some(274_000_000));
        assert_eq!(p.completed, Some(142_000_000));
    }

    #[test]
    fn pull_progress_deserializes_status_only() {
        let json = r#"{"status":"verifying sha256:abc"}"#;
        let p: PullProgress = serde_json::from_str(json).unwrap();
        assert_eq!(p.status.as_deref(), Some("verifying sha256:abc"));
        assert_eq!(p.total, None);
        assert_eq!(p.completed, None);
    }

    #[test]
    fn pull_progress_deserializes_completed_without_total() {
        let json = r#"{"status":"pulling","completed":50000}"#;
        let p: PullProgress = serde_json::from_str(json).unwrap();
        assert_eq!(p.status.as_deref(), Some("pulling"));
        assert_eq!(p.total, None);
        assert_eq!(p.completed, Some(50_000));
    }
}
