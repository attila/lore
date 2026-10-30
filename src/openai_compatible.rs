use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::embeddings::Embedder;

const TIMEOUT_SECS: u64 = 30;

/// An API key read from the environment.
#[derive(Clone)]
struct ApiKey(String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// The variable that holds the API key, as far as a message may name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyVariable {
    /// The configuration names no variable.
    Unnamed,
    /// The variable is unset or empty, so no key is sent.
    Unset(String),
    /// The variable holds the key that is sent.
    Set(String),
}

/// What went wrong when the embedding server was asked for a vector.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServerErrorKind {
    /// HTTP 401 or 403.
    Unauthorized {
        status: u16,
    },
    /// The server rejected the input as longer than its batch.
    InputTooLarge,
    /// Any other non-2xx answer, with the message read from its body.
    Status {
        status: u16,
        message: Option<String>,
    },
    Timeout,
    Transport(String),
    /// A 2xx answer without a vector that has a direction.
    BadResponse,
}

/// A failed request to an OpenAI-compatible embedding server, with the
/// context its messages name.
#[derive(Debug, Clone)]
pub struct ServerError {
    pub kind: ServerErrorKind,
    url: String,
    key: KeyVariable,
    max_tokens: usize,
}

impl ServerError {
    /// Reason in the search fallback warning.
    pub fn short(&self) -> String {
        match &self.kind {
            ServerErrorKind::Unauthorized { .. } => "API key refused".to_string(),
            ServerErrorKind::InputTooLarge => "input too large".to_string(),
            ServerErrorKind::Status { status, .. } => format!("HTTP {status}"),
            ServerErrorKind::Timeout => "timed out".to_string(),
            ServerErrorKind::Transport(_) => "unreachable".to_string(),
            ServerErrorKind::BadResponse => "unexpected answer".to_string(),
        }
    }

    /// `lore status --full` line body.
    pub fn status_line(&self) -> String {
        match &self.kind {
            ServerErrorKind::Unauthorized { status } => {
                format!("the server refused the API key (HTTP {status})")
            }
            ServerErrorKind::InputTooLarge => "a section was too long for the server".to_string(),
            ServerErrorKind::Status { status, message } => {
                format!(
                    "the server answered HTTP {status}{}",
                    suffix(message.as_deref())
                )
            }
            ServerErrorKind::Timeout => format!("no answer within {TIMEOUT_SECS} seconds"),
            ServerErrorKind::Transport(msg) => format!("cannot reach the server: {msg}"),
            ServerErrorKind::BadResponse => "the answer held no usable embedding".to_string(),
        }
    }

    /// What went wrong, naming the server.
    pub fn error_line(&self) -> String {
        let url = &self.url;
        match &self.kind {
            ServerErrorKind::Unauthorized { status } => {
                format!("The embedding server at {url} refused the request with HTTP {status}")
            }
            ServerErrorKind::InputTooLarge => {
                format!("The embedding server at {url} rejected a section as too long")
            }
            ServerErrorKind::Status { status, message } => format!(
                "The embedding server at {url} answered HTTP {status}{}",
                suffix(message.as_deref())
            ),
            ServerErrorKind::Timeout => format!(
                "The embedding server at {url} did not answer within {TIMEOUT_SECS} seconds"
            ),
            ServerErrorKind::Transport(msg) => {
                format!("Cannot reach the embedding server at {url}: {msg}")
            }
            ServerErrorKind::BadResponse => {
                format!("The embedding server at {url} answered without a usable embedding")
            }
        }
    }

    /// What the user can do about it.
    pub fn action_line(&self) -> String {
        match &self.kind {
            ServerErrorKind::Unauthorized { .. } => match &self.key {
                KeyVariable::Set(var) => format!("Check the key in {var}."),
                KeyVariable::Unset(var) => format!("Set {var} to the server's API key."),
                KeyVariable::Unnamed => "Set [embedding] api_key_environment_variable_name to \
                                         the variable that holds the server's API key."
                    .to_string(),
            },
            ServerErrorKind::InputTooLarge => format!(
                "Start 'llama-server' with '-ub' and '-b' of at least {}, or lower [chunking] \
                 max_tokens.",
                self.max_tokens
            ),
            ServerErrorKind::Status { .. } => {
                "Check that the url in [embedding] ends in the server's API path, such as /v1."
                    .to_string()
            }
            ServerErrorKind::Timeout => {
                "Check that the server is running and has the model loaded.".to_string()
            }
            ServerErrorKind::Transport(_) => "Check that the server runs at that address. If a \
                                              proxy is set, check that NO_PROXY covers it."
                .to_string(),
            ServerErrorKind::BadResponse => {
                "Check that the url in [embedding] ends in /v1 and names an embedding model."
                    .to_string()
            }
        }
    }

    /// `lore status` server line body, after the `✗`.
    pub fn reachability_line(&self) -> String {
        let url = &self.url;
        match &self.kind {
            ServerErrorKind::Unauthorized { status } | ServerErrorKind::Status { status, .. } => {
                format!("HTTP {status} at {url}")
            }
            ServerErrorKind::Timeout => {
                format!("cannot reach {url}: no answer within {TIMEOUT_SECS} seconds")
            }
            ServerErrorKind::Transport(msg) => format!("cannot reach {url}: {msg}"),
            ServerErrorKind::InputTooLarge | ServerErrorKind::BadResponse => {
                format!("{} at {url}", self.short())
            }
        }
    }
}

fn suffix(message: Option<&str>) -> String {
    message.map(|m| format!(": {m}")).unwrap_or_default()
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}. {}", self.error_line(), self.action_line())
    }
}

impl std::error::Error for ServerError {}

/// The URL without its userinfo, so a password in it never reaches a
/// message or a trace.
pub fn redact_url(url: &str) -> String {
    let start = url.find("://").map_or(0, |i| i + 3);
    // A password may hold a `/`, and the HTTP parser then reads part of it as
    // the host, so the userinfo ends at the last `@` before the query.
    let end = url[start..]
        .find(['?', '#'])
        .map_or(url.len(), |i| start + i);
    match url[start..end].rfind('@') {
        Some(at) => format!("{}{}", &url[..start], &url[start + at + 1..]),
        None => url.to_string(),
    }
}

/// One line from an error body: the `error` string, or the `message` of an
/// `error` object, or else the body's first non-empty line.
fn error_message(body: &str) -> Option<String> {
    const MAX_CHARS: usize = 240;

    let trimmed = body.trim();
    let message = serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .and_then(|value| {
            let error = value.get("error")?;
            error
                .as_str()
                .or_else(|| error.get("message")?.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            trimmed
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(str::to_string)
        })?;
    let cleaned: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.');
    if cleaned.is_empty() {
        None
    } else if cleaned.chars().count() > MAX_CHARS {
        let kept: String = cleaned.chars().take(MAX_CHARS - 1).collect();
        Some(format!("{kept}…"))
    } else {
        Some(cleaned.to_string())
    }
}

fn failure_kind(e: ureq::Error) -> ServerErrorKind {
    match e {
        ureq::Error::Timeout(_) => ServerErrorKind::Timeout,
        e => ServerErrorKind::Transport(e.to_string()),
    }
}

/// The longest prefix of `s` with at most `max` characters.
fn cut_chars(s: &str, max: usize) -> &str {
    s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}

fn unit_length(mut vector: Vec<f32>) -> Option<Vec<f32>> {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !(norm.is_finite() && norm > 0.0) {
        return None;
    }
    for x in &mut vector {
        *x /= norm;
    }
    Some(vector)
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
    encoding_format: &'static str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// Client for an OpenAI-compatible embedding server, such as `llama-server`.
pub struct OpenAiCompatibleClient {
    url: String,
    model: String,
    max_tokens: usize,
    dimensions: Option<usize>,
    key_variable: KeyVariable,
    key: Option<ApiKey>,
    agent: ureq::Agent,
}

impl OpenAiCompatibleClient {
    /// `url` is the base URL, such as `http://127.0.0.1:8080/v1`; requests go
    /// to `{url}/embeddings`. `max_tokens` bounds how much of each input is
    /// sent.
    pub fn new(url: &str, model: &str, max_tokens: usize) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
            .http_status_as_error(false)
            .build();
        Self {
            url: url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            max_tokens,
            dimensions: None,
            key_variable: KeyVariable::Unnamed,
            key: None,
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Send the key held in the environment variable `name`, if it is set
    /// and not empty.
    #[must_use]
    pub fn with_api_key_variable(mut self, name: Option<&str>) -> Self {
        self.key = name
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
            .map(ApiKey);
        self.key_variable = match (name, &self.key) {
            (None, _) => KeyVariable::Unnamed,
            (Some(name), None) => KeyVariable::Unset(name.to_string()),
            (Some(name), Some(_)) => KeyVariable::Set(name.to_string()),
        };
        self
    }

    /// Require the model's vectors to be `dimensions` wide before any of
    /// them is written. The server is never asked for a number of dimensions.
    #[must_use]
    pub fn with_dimensions(mut self, dimensions: Option<usize>) -> Self {
        self.dimensions = dimensions;
        self
    }

    /// The base URL without its userinfo.
    pub fn url(&self) -> String {
        redact_url(&self.url)
    }

    pub fn key_variable(&self) -> &KeyVariable {
        &self.key_variable
    }

    /// Ask `{url}/models` whether the server answers, without embedding.
    pub fn reachability(&self) -> Result<(), ServerError> {
        let mut request = self.agent.get(format!("{}/models", self.url));
        if let Some(ApiKey(key)) = &self.key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let response = request.call().map_err(|e| self.error(failure_kind(e)))?;
        let status = response.status().as_u16();
        match status {
            200..=299 => Ok(()),
            401 | 403 => Err(self.error(ServerErrorKind::Unauthorized { status })),
            _ => Err(self.error(ServerErrorKind::Status {
                status,
                message: None,
            })),
        }
    }

    /// Embed a one-character text and return the vector's number of
    /// dimensions.
    pub fn probe(&self) -> Result<usize, ServerError> {
        self.request(".").map(|vector| vector.len())
    }

    fn error(&self, kind: ServerErrorKind) -> ServerError {
        ServerError {
            kind,
            url: redact_url(&self.url),
            key: self.key_variable.clone(),
            max_tokens: self.max_tokens,
        }
    }

    fn request(&self, input: &str) -> Result<Vec<f32>, ServerError> {
        let mut request = self.agent.post(format!("{}/embeddings", self.url));
        if let Some(ApiKey(key)) = &self.key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let mut response = request
            .send_json(EmbeddingRequest {
                model: &self.model,
                input,
                encoding_format: "float",
            })
            .map_err(|e| self.error(failure_kind(e)))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_vec()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| self.error(failure_kind(e)))?;
        if !(200..300).contains(&status) {
            return Err(self.error(if status == 401 || status == 403 {
                ServerErrorKind::Unauthorized { status }
            } else if body.contains("physical batch size") {
                ServerErrorKind::InputTooLarge
            } else {
                ServerErrorKind::Status {
                    status,
                    message: error_message(&body),
                }
            }));
        }
        serde_json::from_str::<EmbeddingResponse>(&body)
            .ok()
            .and_then(|response| response.data.into_iter().next())
            .and_then(|data| unit_length(data.embedding))
            .ok_or_else(|| self.error(ServerErrorKind::BadResponse))
    }
}

impl Embedder for OpenAiCompatibleClient {
    /// Sends at most `max_tokens × 3` characters. A server whose batch is too
    /// small for that gets one more try at `max_tokens × 2`.
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        let first = cut_chars(input, self.max_tokens.saturating_mul(3));
        let result = match self.request(first) {
            Err(e) if e.kind == ServerErrorKind::InputTooLarge => {
                let second = cut_chars(input, self.max_tokens.saturating_mul(2));
                if second.len() < first.len() {
                    self.request(second)
                } else {
                    Err(e)
                }
            }
            result => result,
        };
        result.map_err(anyhow::Error::new)
    }

    fn provider(&self) -> &'static str {
        "openai-compatible"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn configured_dimensions(&self) -> Option<usize> {
        self.dimensions
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use super::*;

    const BATCH_TOO_SMALL: &str = r#"{"error":{"code":500,"message":"input (962 tokens) is too large to process. increase the physical batch size (current batch size: 512)","type":"server_error"}}"#;
    const INVALID_KEY: &str =
        r#"{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}"#;

    /// One request as the stub server read it.
    #[derive(Debug)]
    struct Received {
        method: String,
        path: String,
        authorization: Option<String>,
        body: serde_json::Value,
    }

    impl Received {
        fn input(&self) -> &str {
            self.body["input"].as_str().unwrap()
        }
    }

    fn read_request(stream: &TcpStream) -> Received {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let mut content_length = 0;
        let mut authorization = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap();
                } else if name.eq_ignore_ascii_case("authorization") {
                    authorization = Some(value.trim().to_string());
                }
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        Received {
            method,
            path,
            authorization,
            body: serde_json::from_slice(&body).unwrap_or_default(),
        }
    }

    /// Serve each request with `answer`, which returns the status line, with
    /// any extra header lines after it, and the body. Returns the base URL
    /// and every request received.
    fn serve(
        answer: impl Fn(&Received) -> (String, String) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<Received>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let request = read_request(&stream);
                let (status, response) = answer(&request);
                log.lock().unwrap().push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                )
                .unwrap();
            }
        });
        (url, received)
    }

    fn vector(values: &[f32]) -> (String, String) {
        (
            "200 OK".to_string(),
            serde_json::json!({ "data": [{ "embedding": values, "index": 0 }] }).to_string(),
        )
    }

    fn reject(status: &str, body: &str) -> (String, String) {
        (status.to_string(), body.to_string())
    }

    fn server_error(client: &OpenAiCompatibleClient, input: &str) -> ServerError {
        client
            .embed(input)
            .unwrap_err()
            .downcast::<ServerError>()
            .unwrap()
    }

    #[test]
    fn sends_model_input_and_float_encoding_but_never_dimensions() {
        let (url, received) = serve(|_| vector(&[1.0, 0.0]));
        let client = OpenAiCompatibleClient::new(&format!("{url}/v1/"), "nomic", 1024)
            .with_dimensions(Some(768));
        client.embed("hello").unwrap();

        let received = received.lock().unwrap();
        assert_eq!(received[0].method, "POST");
        assert_eq!(received[0].path, "/v1/embeddings");
        assert_eq!(
            received[0].body,
            serde_json::json!({ "model": "nomic", "input": "hello", "encoding_format": "float" })
        );
        assert_eq!(client.configured_dimensions(), Some(768));
        assert_eq!(client.provider(), "openai-compatible");
        assert_eq!(client.model(), "nomic");
    }

    #[test]
    fn sends_the_key_only_when_its_variable_holds_one() {
        let (url, received) = serve(|_| vector(&[1.0]));
        let var = "LORE_TEST_OPENAI_COMPATIBLE_KEY";
        let key = "sk-test key/+=";
        temp_env::with_var(var, Some(key), || {
            OpenAiCompatibleClient::new(&url, "m", 8)
                .with_api_key_variable(Some(var))
                .embed("a")
                .unwrap();
        });
        temp_env::with_var(var, Some(""), || {
            OpenAiCompatibleClient::new(&url, "m", 8)
                .with_api_key_variable(Some(var))
                .embed("a")
                .unwrap();
        });
        temp_env::with_var(var, None::<&str>, || {
            OpenAiCompatibleClient::new(&url, "m", 8)
                .with_api_key_variable(Some(var))
                .embed("a")
                .unwrap();
        });
        OpenAiCompatibleClient::new(&url, "m", 8)
            .embed("a")
            .unwrap();

        let headers: Vec<_> = received
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.authorization.clone())
            .collect();
        assert_eq!(headers, [Some(format!("Bearer {key}")), None, None, None]);
    }

    #[test]
    fn a_refused_key_names_the_variable_to_fix() {
        let (url, _) = serve(|r| {
            if r.authorization.is_some() {
                reject("403 Forbidden", "")
            } else {
                reject("401 Unauthorized", INVALID_KEY)
            }
        });
        let var = "LORE_TEST_OPENAI_COMPATIBLE_REFUSED_KEY";
        let set = temp_env::with_var(var, Some("wrong"), || {
            server_error(
                &OpenAiCompatibleClient::new(&url, "m", 8).with_api_key_variable(Some(var)),
                "a",
            )
        });
        assert_eq!(set.kind, ServerErrorKind::Unauthorized { status: 403 });
        assert_eq!(set.action_line(), format!("Check the key in {var}."));

        let unset = temp_env::with_var(var, None::<&str>, || {
            server_error(
                &OpenAiCompatibleClient::new(&url, "m", 8).with_api_key_variable(Some(var)),
                "a",
            )
        });
        assert_eq!(unset.kind, ServerErrorKind::Unauthorized { status: 401 });
        assert_eq!(
            unset.action_line(),
            format!("Set {var} to the server's API key.")
        );

        let unnamed = server_error(&OpenAiCompatibleClient::new(&url, "m", 8), "a");
        assert_eq!(
            unnamed.action_line(),
            "Set [embedding] api_key_environment_variable_name to the variable that holds the \
             server's API key."
        );
    }

    #[test]
    fn a_batch_too_small_for_the_first_cut_gets_one_shorter_retry() {
        let (url, received) = serve(|r| {
            if r.input().chars().count() > 25 {
                reject("500 Internal Server Error", BATCH_TOO_SMALL)
            } else {
                vector(&[0.0, 2.0])
            }
        });
        let client = OpenAiCompatibleClient::new(&url, "m", 10);
        assert_eq!(client.embed(&"x".repeat(50)).unwrap(), [0.0, 1.0]);

        let lengths: Vec<_> = received
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.input().len())
            .collect();
        assert_eq!(lengths, [30, 20]);
    }

    #[test]
    fn a_batch_too_small_for_the_retry_gives_the_batch_guidance() {
        let (url, received) = serve(|r| {
            if r.input().chars().count() > 15 {
                reject("500 Internal Server Error", BATCH_TOO_SMALL)
            } else {
                vector(&[1.0])
            }
        });
        let client = OpenAiCompatibleClient::new(&url, "m", 10);
        let err = server_error(&client, &"x".repeat(50));
        assert_eq!(err.kind, ServerErrorKind::InputTooLarge);
        assert_eq!(
            err.to_string(),
            format!(
                "The embedding server at {url} rejected a section as too long. Start \
                 'llama-server' with '-ub' and '-b' of at least 10, or lower [chunking] \
                 max_tokens."
            )
        );
        assert_eq!(received.lock().unwrap().len(), 2);

        // An input the retry would not shorten is sent once.
        received.lock().unwrap().clear();
        let err = server_error(&client, &"x".repeat(18));
        assert_eq!(err.kind, ServerErrorKind::InputTooLarge);
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[test]
    fn cuts_the_input_on_a_char_boundary() {
        let (url, received) = serve(|_| vector(&[1.0]));
        OpenAiCompatibleClient::new(&url, "m", 4)
            .embed(&"é".repeat(20))
            .unwrap();
        assert_eq!(received.lock().unwrap()[0].input(), "é".repeat(12));
    }

    #[test]
    fn reads_the_error_message_from_each_body_shape() {
        for (body, message) in [
            (r#"{"error":"model not found"}"#, Some("model not found")),
            (
                r#"{"error":"you do not have access to it."}"#,
                Some("you do not have access to it"),
            ),
            (INVALID_KEY, Some("Invalid API Key")),
            (
                r#"{"error":{"message":"No such model","type":"invalid_request_error","code":"model_not_found"}}"#,
                Some("No such model"),
            ),
            ("\nnot found\nsecond line", Some("not found")),
            ("  ", None),
        ] {
            assert_eq!(error_message(body).as_deref(), message, "{body}");
        }
    }

    #[test]
    fn another_status_names_it_with_the_body_message() {
        let (url, _) = serve(|_| {
            reject(
                "404 Not Found",
                r#"{"error":{"code":404,"message":"File Not Found","type":"not_found_error"}}"#,
            )
        });
        let err = server_error(&OpenAiCompatibleClient::new(&url, "m", 8), "a");
        assert_eq!(
            err.kind,
            ServerErrorKind::Status {
                status: 404,
                message: Some("File Not Found".to_string())
            }
        );
        assert_eq!(err.short(), "HTTP 404");
        assert_eq!(
            err.status_line(),
            "the server answered HTTP 404: File Not Found"
        );
        assert_eq!(
            err.to_string(),
            format!(
                "The embedding server at {url} answered HTTP 404: File Not Found. Check that \
                 the url in [embedding] ends in the server's API path, such as /v1."
            )
        );
    }

    #[test]
    fn scales_the_vector_to_unit_length() {
        let (url, _) = serve(|_| vector(&[3.0, 4.0]));
        let vector = OpenAiCompatibleClient::new(&url, "m", 8)
            .embed("a")
            .unwrap();
        assert_eq!(vector, [0.6, 0.8]);
    }

    #[test]
    fn an_answer_without_a_usable_vector_is_a_bad_response() {
        for body in [
            serde_json::json!({ "data": [{ "embedding": [0.0, 0.0] }] }),
            serde_json::json!({ "data": [{ "embedding": [] }] }),
            serde_json::json!({ "data": [] }),
            serde_json::json!([[0.1, 0.2]]),
        ] {
            let (url, _) = serve(move |_| ("200 OK".to_string(), body.to_string()));
            let err = server_error(&OpenAiCompatibleClient::new(&url, "m", 8), "a");
            assert_eq!(err.kind, ServerErrorKind::BadResponse);
            assert_eq!(
                err.to_string(),
                format!(
                    "The embedding server at {url} answered without a usable embedding. Check \
                     that the url in [embedding] ends in /v1 and names an embedding model."
                )
            );
        }
    }

    #[test]
    fn an_unreachable_server_names_the_address_and_the_proxy() {
        let err = server_error(
            &OpenAiCompatibleClient::new("http://127.0.0.1:1/v1", "m", 8),
            "a",
        );
        assert!(matches!(err.kind, ServerErrorKind::Transport(_)));
        assert_eq!(err.short(), "unreachable");
        assert!(
            err.error_line()
                .starts_with("Cannot reach the embedding server at http://127.0.0.1:1/v1: "),
            "{err}"
        );
        assert_eq!(
            err.action_line(),
            "Check that the server runs at that address. If a proxy is set, check that \
             NO_PROXY covers it."
        );
    }

    #[test]
    fn a_body_cut_short_is_a_transport_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"data\"")
                .unwrap();
        });
        let err = server_error(&OpenAiCompatibleClient::new(&url, "m", 8), "a");
        assert!(matches!(err.kind, ServerErrorKind::Transport(_)), "{err}");
    }

    #[test]
    fn every_kind_renders_its_four_lines() {
        let err = |kind| ServerError {
            kind,
            url: "http://h/v1".to_string(),
            key: KeyVariable::Unnamed,
            max_tokens: 512,
        };
        let lines = |e: &ServerError| [e.short(), e.status_line(), e.error_line(), e.action_line()];

        assert_eq!(
            lines(&err(ServerErrorKind::Unauthorized { status: 401 }))[..3],
            [
                "API key refused",
                "the server refused the API key (HTTP 401)",
                "The embedding server at http://h/v1 refused the request with HTTP 401",
            ]
        );
        assert_eq!(
            lines(&err(ServerErrorKind::InputTooLarge)),
            [
                "input too large",
                "a section was too long for the server",
                "The embedding server at http://h/v1 rejected a section as too long",
                "Start 'llama-server' with '-ub' and '-b' of at least 512, or lower [chunking] \
                 max_tokens.",
            ]
        );
        assert_eq!(
            lines(&err(ServerErrorKind::Status {
                status: 502,
                message: None
            }))[..3],
            [
                "HTTP 502",
                "the server answered HTTP 502",
                "The embedding server at http://h/v1 answered HTTP 502",
            ]
        );
        assert_eq!(
            lines(&err(ServerErrorKind::Timeout)),
            [
                "timed out",
                "no answer within 30 seconds",
                "The embedding server at http://h/v1 did not answer within 30 seconds",
                "Check that the server is running and has the model loaded.",
            ]
        );
        assert_eq!(
            lines(&err(ServerErrorKind::Transport("refused".to_string())))[..3],
            [
                "unreachable",
                "cannot reach the server: refused",
                "Cannot reach the embedding server at http://h/v1: refused",
            ]
        );
        assert_eq!(
            lines(&err(ServerErrorKind::BadResponse))[..3],
            [
                "unexpected answer",
                "the answer held no usable embedding",
                "The embedding server at http://h/v1 answered without a usable embedding",
            ]
        );
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let debug = format!("{:?}", Some(ApiKey("sk-secret-value".to_string())));
        assert_eq!(debug, "Some(ApiKey(<redacted>))");
    }

    #[test]
    fn a_redirect_never_carries_the_key() {
        let (target, at_target) = serve(|_| vector(&[1.0]));
        let (url, at_origin) = serve(move |_| {
            (
                format!("302 Found\r\nLocation: {target}/v1/embeddings"),
                String::new(),
            )
        });
        let var = "LORE_TEST_OPENAI_COMPATIBLE_REDIRECT_KEY";
        temp_env::with_var(var, Some("sk-redirect"), || {
            OpenAiCompatibleClient::new(&url, "m", 8)
                .with_api_key_variable(Some(var))
                .embed("a")
                .unwrap();
        });

        assert_eq!(
            at_origin.lock().unwrap()[0].authorization.as_deref(),
            Some("Bearer sk-redirect")
        );
        let at_target = at_target.lock().unwrap();
        assert_eq!(at_target.len(), 1, "the redirect was not followed");
        assert_eq!(at_target[0].authorization, None);
    }

    #[test]
    fn redact_url_drops_only_the_userinfo() {
        for (url, redacted) in [
            ("http://user:pw@host:8080/v1", "http://host:8080/v1"),
            ("https://token@host/v1?a=b#c", "https://host/v1?a=b#c"),
            ("http://a@b@host/v1", "http://host/v1"),
            ("http://u:abc/def@host/v1", "http://host/v1"),
            ("http://u:123/def@host/v1", "http://host/v1"),
            ("http://tok/en@host/v1", "http://host/v1"),
            ("http://host:8080/v1", "http://host:8080/v1"),
            ("user:pw@host", "host"),
        ] {
            assert_eq!(redact_url(url), redacted, "{url}");
        }
    }

    #[test]
    fn errors_name_the_redacted_url() {
        let err = server_error(
            &OpenAiCompatibleClient::new("http://user:pw@127.0.0.1:1/v1/", "m", 8),
            "a",
        );
        assert!(
            err.error_line()
                .starts_with("Cannot reach the embedding server at http://127.0.0.1:1/v1: "),
            "{err}"
        );
        assert!(!err.to_string().contains("pw"), "{err}");
    }

    #[test]
    fn reachability_asks_for_the_models_with_the_key() {
        let (url, received) = serve(|r| {
            if r.authorization.is_some() {
                ("200 OK".to_string(), r#"{"data":[]}"#.to_string())
            } else {
                reject("401 Unauthorized", INVALID_KEY)
            }
        });
        let var = "LORE_TEST_OPENAI_COMPATIBLE_MODELS_KEY";
        temp_env::with_var(var, Some("sk-models"), || {
            let client = OpenAiCompatibleClient::new(&format!("{url}/v1"), "m", 8)
                .with_api_key_variable(Some(var));
            assert!(client.reachability().is_ok());
            assert_eq!(client.key_variable(), &KeyVariable::Set(var.to_string()));
        });
        let refused = OpenAiCompatibleClient::new(&format!("{url}/v1"), "m", 8)
            .reachability()
            .unwrap_err();
        assert_eq!(refused.kind, ServerErrorKind::Unauthorized { status: 401 });
        assert_eq!(refused.reachability_line(), format!("HTTP 401 at {url}/v1"));

        let received = received.lock().unwrap();
        assert_eq!(received[0].method, "GET");
        assert_eq!(received[0].path, "/v1/models");
        assert_eq!(
            received[0].authorization.as_deref(),
            Some("Bearer sk-models")
        );
        assert_eq!(received[1].authorization, None);
    }

    #[test]
    fn reachability_names_an_address_that_does_not_answer() {
        let client = OpenAiCompatibleClient::new("http://user:pw@127.0.0.1:1/v1", "m", 8);
        assert_eq!(client.url(), "http://127.0.0.1:1/v1");
        let line = client.reachability().unwrap_err().reachability_line();
        assert!(
            line.starts_with("cannot reach http://127.0.0.1:1/v1: "),
            "{line}"
        );
    }

    #[test]
    fn probe_returns_the_number_of_dimensions() {
        let (url, received) = serve(|_| vector(&[1.0, 2.0, 2.0]));
        assert_eq!(
            OpenAiCompatibleClient::new(&url, "m", 8).probe().unwrap(),
            3
        );
        assert_eq!(received.lock().unwrap()[0].input(), ".");
    }
}
