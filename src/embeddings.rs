use std::io::BufRead;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Trait for producing embedding vectors from text.
pub trait Embedder {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>>;
    /// Backend name recorded with the index, such as `ollama`.
    fn provider(&self) -> &str;
    /// Model name recorded with the index.
    fn model(&self) -> &str;
    /// Width the configuration requires of this model's vectors. A build
    /// whose measured width differs must stop before it sizes the vector
    /// table.
    fn configured_dimensions(&self) -> Option<usize> {
        None
    }
}

/// Text embedded once per index build. Changing it makes every recorded
/// probe vector incomparable with a fresh one.
pub const PROBE_TEXT: &str = "lore embedding identity probe";

/// The embedding model an index was built with, measured rather than
/// looked up: `dimensions` is the length of `probe_vector`.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingIdentity {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub probe_vector: Vec<f32>,
}

impl EmbeddingIdentity {
    /// Embed [`PROBE_TEXT`] and record who answered and how wide the vector is.
    /// A vector with no direction fails like a model that did not answer: no
    /// later probe could match it.
    pub fn measure(embedder: &dyn Embedder) -> anyhow::Result<Self> {
        let probe_vector = embedder.embed(PROBE_TEXT)?;
        if probe_vector.is_empty() {
            anyhow::bail!(
                "embedding model {} returned an empty vector",
                embedder.model()
            );
        }
        let norm: f64 = probe_vector.iter().map(|x| f64::from(*x).powi(2)).sum();
        if !(norm.is_finite() && norm > 0.0) {
            anyhow::bail!(
                "embedding model {} returned a vector with no direction",
                embedder.model()
            );
        }
        Ok(Self {
            provider: embedder.provider().to_string(),
            model: embedder.model().to_string(),
            dimensions: probe_vector.len(),
            probe_vector,
        })
    }
}

/// Provider and model recorded with an index's vector table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedModel {
    pub provider: String,
    pub model: String,
}

/// Why vectors from the configured model must not meet the index's: a
/// query searches text only, and an ingest writes no vectors.
#[derive(Debug, Clone, PartialEq)]
pub enum VectorMismatch {
    /// The index records another provider or model than the one configured.
    Model {
        recorded: String,
        configured: String,
    },
    /// The query vector is not as wide as the index's vectors. `recorded` is
    /// `None` for an index that has no recorded model.
    Width {
        recorded: Option<String>,
        configured: String,
        index: usize,
        query: usize,
    },
    /// The configured model answers [`PROBE_TEXT`] unlike the model that
    /// built the index under the same name.
    Probe { model: String, similarity: f64 },
    /// The configured model's vectors are not as wide as `[ollama]
    /// dimensions` requires. `--force` measures the same width again, so it
    /// is no remedy.
    Configured {
        model: String,
        measured: usize,
        configured: usize,
    },
}

/// Cosine similarity a fresh probe vector must reach against the recorded
/// one for the configured model to count as the model that built the index.
pub const PROBE_SIMILARITY_FLOOR: f64 = 0.99;

impl VectorMismatch {
    /// Compare the recorded model with `embedder` before embedding a query.
    /// An index with no recorded model passes: only its width can tell.
    pub fn of_model(recorded: Option<&RecordedModel>, embedder: &dyn Embedder) -> Option<Self> {
        let recorded = recorded?;
        let same = recorded.provider == embedder.provider()
            && same_model(&recorded.provider, &recorded.model, embedder.model());
        (!same).then(|| Self::Model {
            recorded: recorded.model.clone(),
            configured: embedder.model().to_string(),
        })
    }

    /// Compare a query vector's width with the width of the index's vectors.
    pub fn of_width(
        recorded: Option<&RecordedModel>,
        embedder: &dyn Embedder,
        index: usize,
        query: usize,
    ) -> Option<Self> {
        (index != query).then(|| Self::Width {
            recorded: recorded.map(|r| r.model.clone()),
            configured: embedder.model().to_string(),
            index,
            query,
        })
    }

    /// Compare a fresh [`PROBE_TEXT`] vector with the one recorded when the
    /// index was built. Vectors of different widths never match.
    pub fn of_probe(embedder: &dyn Embedder, recorded: &[f32], fresh: &[f32]) -> Option<Self> {
        let similarity = cosine_similarity(recorded, fresh);
        let same = similarity >= PROBE_SIMILARITY_FLOOR;
        (!same).then(|| Self::Probe {
            model: embedder.model().to_string(),
            similarity,
        })
    }

    /// Compare the configured model's measured width with `[ollama]
    /// dimensions`. Without a configured width, any width passes.
    pub fn of_configured(embedder: &dyn Embedder, measured: usize) -> Option<Self> {
        let configured = embedder.configured_dimensions()?;
        (configured != measured).then(|| Self::Configured {
            model: embedder.model().to_string(),
            measured,
            configured,
        })
    }

    /// What the user does about it.
    pub fn remedy(&self) -> String {
        match self {
            Self::Configured {
                measured,
                configured,
                ..
            } => format!(
                "Set [ollama] dimensions to {measured} or \"auto\", or configure a model that \
                 returns {configured}-dimension vectors"
            ),
            _ => "Run 'lore ingest --force' to rebuild the index".to_string(),
        }
    }

    /// What differs, without what it means for the caller.
    pub fn cause(&self) -> String {
        match self {
            Self::Model {
                recorded,
                configured,
            } => format!(
                "the index was built with {recorded} but lore is configured for {configured}"
            ),
            Self::Width {
                recorded,
                configured,
                index,
                query,
            } => {
                let from = recorded.as_deref().unwrap_or("an unrecorded model");
                format!(
                    "the index holds {index}-dimension vectors from {from} but {configured} \
                     returns {query} dimensions"
                )
            }
            Self::Probe { model, similarity } => {
                // Rounded down, so a value just below the floor never prints as the floor.
                let shown = (similarity.clamp(-1.0, 1.0) * 1000.0).floor() / 1000.0;
                format!(
                    "{model} is not the model that built the index under that name: its \
                     vectors match the recorded ones at cosine similarity {shown:.3}, below \
                     {PROBE_SIMILARITY_FLOOR}"
                )
            }
            Self::Configured {
                model,
                measured,
                configured,
            } => format!(
                "{model} returns {measured}-dimension vectors but [ollama] dimensions is \
                 {configured}"
            ),
        }
    }
}

/// Ollama resolves an untagged model name to its `:latest` tag, so the two
/// spellings name the same model; any other tag is a different model.
pub fn same_model(provider: &str, recorded: &str, configured: &str) -> bool {
    fn untagged(model: &str) -> &str {
        model.strip_suffix(":latest").unwrap_or(model)
    }
    if provider == "ollama" {
        untagged(recorded) == untagged(configured)
    } else {
        recorded == configured
    }
}

/// Zero for vectors of different widths or of zero length. Accumulates in
/// f64 so rounding over a thousand dimensions cannot push a same-model probe
/// below [`PROBE_SIMILARITY_FLOOR`].
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let norms = (norm_a * norm_b).sqrt();
    if norms == 0.0 {
        return 0.0;
    }
    dot / norms
}

impl std::fmt::Display for VectorMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, so searches use text only. {}",
            self.cause(),
            self.remedy()
        )
    }
}

/// Client for the Ollama embedding API.
pub struct OllamaClient {
    host: String,
    model: String,
    dimensions: Option<usize>,
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
///
/// Marked `#[non_exhaustive]` so future variants are not breaking changes for
/// downstream `match` consumers. New variants should be added with a matching
/// arm in `render_failure` so all rendering surfaces stay in sync.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
///
/// Marked `#[non_exhaustive]` so adding a surface (additional field) later is
/// not a breaking change to consumers that pattern-match or construct this.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
            status_line: "inference timed out (>30s) — check 'ollama serve' is healthy".to_string(),
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
    // Read the body as bytes and decode lossily so a non-UTF-8 sequence
    // doesn't collapse the entire diagnostic. Ollama emits UTF-8 in
    // practice, but a misbehaving proxy or intercepting middleware might
    // not. `from_utf8_lossy` preserves what's decodable and replaces
    // invalid bytes with the U+FFFD replacement character.
    let body = resp
        .body_mut()
        .read_to_vec()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();

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
/// 4. Truncated to 240 characters with `…` if longer — generous enough to
///    surface Ollama runner errors with full search paths
///    (`/opt/homebrew/Cellar/ollama/.../runners/...`) on typical terminals,
///    bounded enough to cap pathological multi-paragraph error bodies.
/// 5. Empty → `None` (caller falls back to a variant-specific string without
///    the dash-body suffix).
fn extract_body_message(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: String,
    }

    const MAX_BODY_CHARS: usize = 240;

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(parsed) = serde_json::from_str::<ErrorBody>(trimmed) {
        let cleaned = clean_for_one_line(&parsed.error);
        if !cleaned.is_empty() {
            return Some(truncate_chars(&cleaned, MAX_BODY_CHARS));
        }
    }

    let line = trimmed.lines().find(|l| !l.trim().is_empty())?;
    let cleaned = clean_for_one_line(line);
    if cleaned.is_empty() {
        None
    } else {
        Some(truncate_chars(&cleaned, MAX_BODY_CHARS))
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
    ///
    /// The internal agent is configured with `http_status_as_error(false)` so
    /// HTTP 4xx/5xx responses come back as `Ok(Response)` rather than
    /// `Err(Error::StatusCode(_))`. The runtime probe and `Embedder::embed`
    /// both need to read the response body on 5xx to surface Ollama's actual
    /// diagnostic (`llama runner process has terminated: …`) — the error
    /// variant in ureq 3 does not carry the body, only the status code.
    pub fn new(host: &str, model: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::new_with_config(config);

        Self {
            host: host.to_string(),
            model: model.to_string(),
            dimensions: None,
            agent,
        }
    }

    /// Require the model's vectors to be `dimensions` wide when an ingest
    /// builds the vector table.
    #[must_use]
    pub fn with_dimensions(mut self, dimensions: Option<usize>) -> Self {
        self.dimensions = dimensions;
        self
    }

    fn embed_request(&self, input: &str) -> EmbedRequest {
        EmbedRequest {
            model: self.model.clone(),
            input: vec![input.to_string()],
            keep_alive: None,
        }
    }

    /// Returns `true` if the Ollama server is reachable AND answering 2xx.
    ///
    /// Since the agent is configured with `http_status_as_error(false)`, we
    /// must check the status code manually — an HTTP 500 response from a
    /// broken daemon would otherwise look "healthy".
    pub fn is_healthy(&self) -> bool {
        self.agent
            .get(&self.host)
            .call()
            .is_ok_and(|r| r.status().is_success())
    }

    /// Returns `true` if the configured model is available on the server.
    ///
    /// Checks the status code manually so a non-2xx response from `/api/show`
    /// (model not found is typically HTTP 404) does not falsely look like
    /// success under `http_status_as_error(false)`.
    pub fn has_model(&self) -> bool {
        let url = format!("{}/api/show", self.host);
        let req = ShowRequest {
            name: self.model.clone(),
        };
        self.agent
            .post(&url)
            .send_json(&req)
            .is_ok_and(|r| r.status().is_success())
    }

    /// Pulls the configured model from Ollama, reporting progress via callback.
    pub fn pull_model(&self, on_progress: &dyn Fn(&PullProgress)) -> anyhow::Result<()> {
        let url = format!("{}/api/pull", self.host);
        let req = PullRequest {
            name: self.model.clone(),
            stream: true,
        };

        let mut resp = self.agent.post(&url).send_json(&req)?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            // Agent is configured with `http_status_as_error(false)`, so a
            // 5xx from /api/pull does not auto-fail through `?`. Bail
            // explicitly with the body so callers see the real reason.
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            anyhow::bail!("Ollama /api/pull returned HTTP {status}: {body}");
        }
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

        // Verify the response actually contains an embedding vector. A 2xx
        // body of `{}` or `{"embeddings":[]}` would otherwise pass the probe
        // without proving the runner ran.
        let resp = serde_json::from_str::<EmbedResponse>(&body)
            .map_err(|_| ProbeError::InferenceError { body: body.clone() })?;
        if resp.embeddings.is_empty() {
            return Err(ProbeError::InferenceError { body });
        }

        Ok(())
    }
}

impl Embedder for OllamaClient {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        let url = format!("{}/api/embed", self.host);
        let req = self.embed_request(input);

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

        // A 2xx body that doesn't deserialise as `EmbedResponse` (missing
        // `embeddings` field, malformed shape) is itself an inference failure
        // from our perspective — surface it as `ProbeError::InferenceError`
        // so the hook downcast classifies it correctly instead of falling
        // through to the generic "unreachable" label.
        let resp: EmbedResponse = serde_json::from_str(&body)
            .map_err(|_| anyhow::Error::new(ProbeError::InferenceError { body: body.clone() }))?;

        resp.embeddings
            .into_iter()
            .next()
            // Empty `embeddings: []` is the same diagnostic-quality failure:
            // Ollama responded successfully but produced no vector. Tag as
            // InferenceError so callers see a classified failure.
            .ok_or_else(|| anyhow::Error::new(ProbeError::InferenceError { body }))
    }

    fn provider(&self) -> &'static str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn configured_dimensions(&self) -> Option<usize> {
        self.dimensions
    }
}

/// Model name the test fakes report. Matches `Config::default_with`'s
/// default so a fake-built index carries the configured identity.
#[cfg(any(test, feature = "test-support"))]
const FAKE_MODEL: &str = "nomic-embed-text";

/// A deterministic fake embedder for use in tests.
///
/// Produces vectors of a fixed dimensionality (default 768) seeded by a simple
/// hash of the input string, so the same input always yields the same vector.
#[cfg(any(test, feature = "test-support"))]
pub struct FakeEmbedder {
    dims: usize,
    model: String,
    configured: Option<usize>,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeEmbedder {
    pub fn new() -> Self {
        Self::with_dimensions(768)
    }

    #[allow(dead_code)]
    pub fn with_dimensions(dims: usize) -> Self {
        Self::with_model(FAKE_MODEL, dims)
    }

    #[allow(dead_code)]
    pub fn with_model(model: &str, dims: usize) -> Self {
        Self {
            dims,
            model: model.to_string(),
            configured: None,
        }
    }

    /// The fake as configured with `[ollama] dimensions = configured`.
    #[allow(dead_code)]
    #[must_use]
    pub fn configured_for(mut self, configured: usize) -> Self {
        self.configured = Some(configured);
        self
    }

    /// Width of every vector this fake produces.
    pub fn dimensions(&self) -> usize {
        self.dims
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

    fn provider(&self) -> &'static str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn configured_dimensions(&self) -> Option<usize> {
        self.configured
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

    /// Width a test gives the vector table it builds alongside this fake.
    pub fn dimensions(&self) -> usize {
        self.dims
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Embedder for FailingEmbedder {
    fn embed(&self, _input: &str) -> anyhow::Result<Vec<f32>> {
        anyhow::bail!("Ollama is unreachable")
    }

    fn provider(&self) -> &'static str {
        "ollama"
    }

    fn model(&self) -> &str {
        FAKE_MODEL
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
    fn measure_records_provider_model_and_probe_width() {
        let embedder = FakeEmbedder::with_model("some-unlisted-model", 1024);
        let identity = EmbeddingIdentity::measure(&embedder).unwrap();
        assert_eq!(identity.provider, "ollama");
        assert_eq!(identity.model, "some-unlisted-model");
        assert_eq!(identity.dimensions, 1024);
        assert_eq!(identity.probe_vector, embedder.embed(PROBE_TEXT).unwrap());
    }

    #[test]
    fn measure_fails_when_the_embedder_fails() {
        assert!(EmbeddingIdentity::measure(&FailingEmbedder::new(4)).is_err());
    }

    #[test]
    fn measure_fails_when_the_probe_vector_has_no_direction() {
        struct ConstantEmbedder(f32);
        impl Embedder for ConstantEmbedder {
            fn embed(&self, _input: &str) -> anyhow::Result<Vec<f32>> {
                Ok(vec![self.0; 4])
            }
            fn provider(&self) -> &'static str {
                "ollama"
            }
            fn model(&self) -> &'static str {
                "zero-model"
            }
        }

        for value in [0.0, f32::NAN] {
            let err = EmbeddingIdentity::measure(&ConstantEmbedder(value)).unwrap_err();
            assert_eq!(
                err.to_string(),
                "embedding model zero-model returned a vector with no direction"
            );
        }
        assert!(EmbeddingIdentity::measure(&ConstantEmbedder(0.5)).is_ok());
    }

    fn recorded(model: &str) -> RecordedModel {
        RecordedModel {
            provider: "ollama".to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn of_model_flags_only_a_recorded_model_that_differs() {
        let embedder = FakeEmbedder::new();
        assert_eq!(VectorMismatch::of_model(None, &embedder), None);
        assert_eq!(
            VectorMismatch::of_model(Some(&recorded("nomic-embed-text")), &embedder),
            None
        );
        let mismatch =
            VectorMismatch::of_model(Some(&recorded("mxbai-embed-large")), &embedder).unwrap();
        assert_eq!(
            mismatch.to_string(),
            "the index was built with mxbai-embed-large but lore is configured for \
             nomic-embed-text, so searches use text only. Run 'lore ingest --force' to \
             rebuild the index"
        );
    }

    #[test]
    fn of_model_reads_an_untagged_ollama_model_as_its_latest_tag() {
        let bare = FakeEmbedder::with_model("nomic-embed-text", 768);
        let latest = FakeEmbedder::with_model("nomic-embed-text:latest", 768);
        assert_eq!(
            VectorMismatch::of_model(Some(&recorded("nomic-embed-text:latest")), &bare),
            None
        );
        assert_eq!(
            VectorMismatch::of_model(Some(&recorded("nomic-embed-text")), &latest),
            None
        );
        assert!(
            VectorMismatch::of_model(Some(&recorded("nomic-embed-text:v1.5")), &bare).is_some()
        );
        let tagged = FakeEmbedder::with_model("nomic-embed-text:v1.5", 768);
        assert!(VectorMismatch::of_model(Some(&recorded("nomic-embed-text")), &tagged).is_some());
    }

    #[test]
    fn of_model_flags_a_different_provider() {
        let other = RecordedModel {
            provider: "other".to_string(),
            model: "nomic-embed-text".to_string(),
        };
        assert!(VectorMismatch::of_model(Some(&other), &FakeEmbedder::new()).is_some());
    }

    #[test]
    fn of_width_flags_only_a_width_that_differs() {
        let embedder = FakeEmbedder::new();
        assert_eq!(VectorMismatch::of_width(None, &embedder, 768, 768), None);
        assert_eq!(
            VectorMismatch::of_width(None, &embedder, 1024, 768)
                .unwrap()
                .to_string(),
            "the index holds 1024-dimension vectors from an unrecorded model but \
             nomic-embed-text returns 768 dimensions, so searches use text only. Run \
             'lore ingest --force' to rebuild the index"
        );
        assert!(
            VectorMismatch::of_width(Some(&recorded("nomic-embed-text")), &embedder, 4, 768)
                .unwrap()
                .to_string()
                .starts_with("the index holds 4-dimension vectors from nomic-embed-text but")
        );
    }

    #[test]
    fn of_probe_flags_a_probe_vector_below_the_similarity_floor() {
        let embedder = FakeEmbedder::new();
        let recorded = [1.0, 0.0, 0.0, 0.0];
        assert_eq!(
            VectorMismatch::of_probe(&embedder, &recorded, &[2.0, 0.1, 0.0, 0.0]),
            None
        );
        let mismatch =
            VectorMismatch::of_probe(&embedder, &recorded, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        assert_eq!(
            mismatch.cause(),
            "nomic-embed-text is not the model that built the index under that name: its \
             vectors match the recorded ones at cosine similarity 0.000, below 0.99"
        );
        assert!(VectorMismatch::of_probe(&embedder, &recorded, &[1.0, 0.0, 0.0]).is_some());
        assert!(VectorMismatch::of_probe(&embedder, &recorded, &[f32::NAN; 4]).is_some());

        let just_below =
            VectorMismatch::of_probe(&embedder, &recorded, &[0.98996, 0.14135, 0.0, 0.0]).unwrap();
        assert!(
            just_below
                .cause()
                .ends_with("cosine similarity 0.989, below 0.99"),
            "{}",
            just_below.cause()
        );
    }

    #[test]
    fn ollama_client_reports_its_configured_model() {
        let client = OllamaClient::new("http://localhost:11434", "some-unlisted-model");
        assert_eq!(client.provider(), "ollama");
        assert_eq!(client.model(), "some-unlisted-model");
    }

    #[test]
    fn configured_dimensions_are_checked_by_lore_and_never_sent_to_ollama() {
        let client = OllamaClient::new("http://localhost:11434", "some-unlisted-model")
            .with_dimensions(Some(1024));
        assert_eq!(client.configured_dimensions(), Some(1024));

        let request = serde_json::to_value(client.embed_request("hello")).unwrap();
        assert_eq!(
            request,
            serde_json::json!({ "model": "some-unlisted-model", "input": ["hello"] })
        );
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

    // ---- extract_body_message -----------------------------------------

    #[test]
    fn extract_body_message_returns_none_for_empty_input() {
        assert_eq!(extract_body_message(""), None);
        assert_eq!(extract_body_message("   \n\t"), None);
    }

    #[test]
    fn extract_body_message_extracts_json_error_field() {
        let body = r#"{"error":"llama runner process has terminated"}"#;
        assert_eq!(
            extract_body_message(body).as_deref(),
            Some("llama runner process has terminated")
        );
    }

    #[test]
    fn extract_body_message_falls_back_to_first_line_for_non_json() {
        let body = "first line\nsecond line\nthird";
        assert_eq!(extract_body_message(body).as_deref(), Some("first line"));
    }

    #[test]
    fn extract_body_message_replaces_control_chars_with_spaces() {
        let body = "before\x07after";
        assert_eq!(extract_body_message(body).as_deref(), Some("before after"));
    }

    #[test]
    fn extract_body_message_truncates_with_ellipsis() {
        let body = "x".repeat(300);
        let msg = extract_body_message(&body).unwrap();
        assert!(msg.chars().count() <= 240);
        assert!(msg.ends_with('…'));
    }

    /// The truncation must land on a char boundary. Multi-byte codepoints
    /// (CJK, emoji) sit at byte offsets that don't align with simple byte
    /// slicing — `truncate_chars` uses `chars().take()` to avoid the
    /// `byte_is_char_boundary` panic.
    #[test]
    fn extract_body_message_handles_multibyte_chars_at_truncation_boundary() {
        let body = "あ".repeat(300);
        let msg = extract_body_message(&body).unwrap();
        assert!(msg.chars().count() <= 240);
        // No panic. The byte length is ~720 (3 bytes per char) but the
        // char count is bounded.
    }

    /// `error: null` is a real Ollama response shape (no error). Should
    /// fall through to the line-based fallback rather than the JSON path.
    #[test]
    fn extract_body_message_json_with_null_error_falls_through() {
        let body = r#"{"error":null}"#;
        // The ErrorBody struct requires a `String`, so deserialise fails
        // and we take the first-line path — the entire body becomes the
        // message.
        let msg = extract_body_message(body).unwrap();
        assert_eq!(msg, r#"{"error":null}"#);
    }

    // ---- render_failure -----------------------------------------------

    #[test]
    fn render_failure_runner_failed_includes_body_in_status_line() {
        let err = ProbeError::RunnerFailed {
            status: 500,
            body: r#"{"error":"runner crashed"}"#.to_string(),
        };
        let r = render_failure(&err);
        assert_eq!(r.short_reason, "inference error");
        assert!(r.status_line.contains("inference failed"));
        assert!(r.status_line.contains("runner crashed"));
    }

    #[test]
    fn render_failure_timeout_short_reason() {
        let r = render_failure(&ProbeError::Timeout);
        assert_eq!(r.short_reason, "timed out");
        assert!(r.status_line.starts_with("inference timed out"));
    }

    #[test]
    fn render_failure_transport_short_reason() {
        let r = render_failure(&ProbeError::Transport("connection refused".to_string()));
        assert_eq!(r.short_reason, "transport error");
        assert!(r.status_line.contains("connection refused"));
    }

    #[test]
    fn render_failure_http_status_short_reason() {
        let r = render_failure(&ProbeError::HttpStatus {
            status: 418,
            body: String::new(),
        });
        assert_eq!(r.short_reason, "HTTP 418");
        assert!(r.status_line.contains("418"));
    }

    #[test]
    fn render_failure_inference_error_includes_body() {
        let err = ProbeError::InferenceError {
            body: r#"{"error":"context length exceeded"}"#.to_string(),
        };
        let r = render_failure(&err);
        // InferenceError shares the rendering path with RunnerFailed.
        assert_eq!(r.short_reason, "inference error");
        assert!(r.status_line.contains("context length exceeded"));
    }

    /// `ProbeError` implements `std::error::Error` so it can be wrapped in
    /// `anyhow::Error::new(...)`. The hook path depends on
    /// `downcast_ref::<ProbeError>()` reaching the wrapped variant.
    #[test]
    fn probe_error_can_be_downcast_through_anyhow() {
        let err = anyhow::Error::new(ProbeError::Timeout);
        assert!(matches!(
            err.downcast_ref::<ProbeError>(),
            Some(ProbeError::Timeout)
        ));
    }
}
