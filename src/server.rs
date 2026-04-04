// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hand-rolled MCP JSON-RPC server over stdio.
//!
//! Reads newline-delimited JSON-RPC requests from stdin, dispatches them to
//! the appropriate handler, and writes responses to stdout. The server is
//! single-threaded and synchronous.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

use crate::config::Config;
use crate::database::KnowledgeDB;
use crate::embeddings::Embedder;
use crate::ingest;
use crate::ingest::CommitStatus;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Shared context threaded through every handler.
struct ServerContext<'a> {
    db: &'a KnowledgeDB,
    embedder: &'a dyn Embedder,
    config: &'a Config,
}

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

// ---------------------------------------------------------------------------
// Stdio loop
// ---------------------------------------------------------------------------

/// Start the MCP server on stdin/stdout.
///
/// Opens the knowledge database using dimensions from `embedder`, then enters
/// a line-oriented JSON-RPC read loop.
pub fn start_mcp_server(config: &Config, embedder: &dyn Embedder) -> anyhow::Result<()> {
    let db = KnowledgeDB::open(&config.database, embedder.dimensions())?;
    db.init()?;

    let mode = if config.search.hybrid {
        "hybrid"
    } else {
        "fts5"
    };
    eprintln!("[lore] MCP server started (search mode: {mode})");
    eprintln!("[lore] Database: {}", config.database.display());

    if let Ok(stats) = db.stats() {
        eprintln!(
            "[lore] {} chunks from {} sources",
            stats.chunks, stats.sources
        );
    }

    let ctx = ServerContext {
        db: &db,
        embedder,
        config,
    };

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[lore] Failed to parse request: {e}");
                continue;
            }
        };

        let response = handle_request(&request, &ctx);

        if let Some(resp) = response {
            let json = serde_json::to_string(&resp)?;
            writeln!(out, "{json}")?;
            out.flush()?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn handle_request(req: &JsonRpcRequest, ctx: &ServerContext<'_>) -> Option<JsonRpcResponse> {
    // Validate JSON-RPC version.
    if req.jsonrpc != "2.0" {
        return Some(JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id.clone(),
            result: None,
            error: Some(json!({
                "code": -32600,
                "message": format!("Invalid jsonrpc version: {}", req.jsonrpc)
            })),
        });
    }

    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id.clone(),
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "lore",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            })),
            error: None,
        }),

        "notifications/initialized" => None,

        "tools/list" => Some(JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id.clone(),
            result: Some(json!({ "tools": tool_definitions() })),
            error: None,
        }),

        "tools/call" => Some(handle_tool_call(req, ctx)),

        _ => Some(JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id.clone(),
            result: None,
            error: Some(json!({
                "code": -32601,
                "message": format!("Unknown method: {}", req.method)
            })),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_lines)]
fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_patterns",
            "description":
                "Search the knowledge base for software patterns, conventions, and preferences. \
                 Use this before implementing new code to check for established patterns. \
                 Returns ranked results with source provenance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query"
                    },
                    "top_k": {
                        "type": "number",
                        "description": "Number of results to return (default: from config)"
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "add_pattern",
            "description":
                "Create a new pattern in the knowledge base. Use only when the user explicitly \
                 asks to save, record, or document a pattern. Creates a markdown file, indexes it, \
                 and commits to git.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Pattern title (used as filename and heading)"
                    },
                    "body": {
                        "type": "string",
                        "description": "Pattern content in markdown"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for categorisation"
                    }
                },
                "required": ["title", "body"]
            }
        },
        {
            "name": "update_pattern",
            "description":
                "Replace the content of an existing pattern. Use only when the user explicitly \
                 asks to update or rewrite a pattern. Overwrites the file, re-indexes, and commits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_file": {
                        "type": "string",
                        "description":
                            "Relative path of the file to update (from search results)"
                    },
                    "body": {
                        "type": "string",
                        "description":
                            "New pattern content in markdown (replaces existing body)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional updated tags"
                    }
                },
                "required": ["source_file", "body"]
            }
        },
        {
            "name": "append_to_pattern",
            "description":
                "Add a new section to an existing pattern without replacing it. Use when the user \
                 wants to add examples, edge cases, or notes to an existing pattern. Appends a \
                 heading and body, re-indexes, and commits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_file": {
                        "type": "string",
                        "description":
                            "Relative path of the file to append to (from search results)"
                    },
                    "heading": {
                        "type": "string",
                        "description": "Heading for the new section"
                    },
                    "body": {
                        "type": "string",
                        "description": "Content to append under the heading"
                    }
                },
                "required": ["source_file", "heading", "body"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

fn handle_tool_call(req: &JsonRpcRequest, ctx: &ServerContext<'_>) -> JsonRpcResponse {
    let params = req.params.as_ref();
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let arguments = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(json!({}));

    match name {
        "search_patterns" => handle_search(req, ctx, &arguments),
        "add_pattern" => handle_add(req, ctx, &arguments),
        "update_pattern" => handle_update(req, ctx, &arguments),
        "append_to_pattern" => handle_append(req, ctx, &arguments),
        _ => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id.clone(),
            result: None,
            error: Some(json!({
                "code": -32602,
                "message": format!("Unknown tool: {name}")
            })),
        },
    }
}

// ---------------------------------------------------------------------------
// Input-length validation
// ---------------------------------------------------------------------------

/// Maximum allowed values for MCP tool string inputs (in bytes).
const MAX_QUERY_BYTES: usize = 1024;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SOURCE_FILE_BYTES: usize = 512;
const MAX_HEADING_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 262_144; // 256 KB
const MAX_TAGS_BYTES: usize = 8192; // 8 KB serialised JSON
const MAX_TOP_K: u64 = 100;

/// Return an error response if `value` exceeds `max_bytes`.
fn check_limit(
    req: &JsonRpcRequest,
    value: &str,
    field: &str,
    max_bytes: usize,
) -> Option<JsonRpcResponse> {
    if value.len() > max_bytes {
        Some(error_response(
            req,
            &format!("{field} exceeds maximum length of {max_bytes} bytes"),
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

fn handle_search(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");

    if query.is_empty() {
        return text_response(req, "Please provide a search query.");
    }

    if let Some(err) = check_limit(req, query, "query", MAX_QUERY_BYTES) {
        return err;
    }

    let raw_top_k = args.get("top_k").and_then(Value::as_u64);
    if let Some(k) = raw_top_k
        && k > MAX_TOP_K
    {
        return error_response(
            req,
            &format!("top_k exceeds maximum allowed value of {MAX_TOP_K}"),
        );
    }

    #[allow(clippy::cast_possible_truncation)]
    let top_k = raw_top_k.map_or(ctx.config.search.top_k, |k| k as usize);

    eprintln!("[lore] Search: \"{query}\" (top_k={top_k})");

    let mut embed_failed = false;

    let results = if ctx.config.search.hybrid {
        let query_embedding = match ctx.embedder.embed(query) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("[lore] Embedding failed, falling back to text search: {e}");
                embed_failed = true;
                None
            }
        };
        ctx.db
            .search_hybrid(query, query_embedding.as_deref(), top_k)
    } else {
        ctx.db
            .search_fts(query, top_k)
            .map(|r| r.into_iter().take(top_k).collect())
    };

    match results {
        Ok(results) => {
            // Apply relevance threshold only when hybrid search with
            // successful embedding was used (RRF scores). FTS-only results
            // use a different scale (negative BM25 rank) and bypass filtering.
            let apply_threshold =
                ctx.config.search.hybrid && !embed_failed && ctx.config.search.min_relevance > 0.0;
            let results: Vec<_> = if apply_threshold {
                results
                    .into_iter()
                    .filter(|r| r.score >= ctx.config.search.min_relevance)
                    .collect()
            } else {
                results
            };

            eprintln!("[lore] Found {} results", results.len());

            let formatted: String = results
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let mut lines = vec![format!(
                        "[{}] {} (source: {})",
                        i + 1,
                        r.title,
                        r.source_file
                    )];
                    if !r.heading_path.is_empty() {
                        lines.push(format!("  path: {}", r.heading_path));
                    }
                    if !r.tags.is_empty() {
                        lines.push(format!("  tags: {}", r.tags));
                    }
                    lines.push(format!("  relevance: {:.4}", r.score));
                    lines.push(String::new());
                    lines.push(r.body.clone());
                    lines.join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n\n---\n\n");

            let summary = if results.is_empty() {
                "No matching patterns found in the knowledge base."
            } else {
                "Found matching patterns."
            };

            let response = if embed_failed {
                format!(
                    "Note: Ollama unreachable — results are from text search only.\n\n{summary}\n\n{formatted}"
                )
            } else {
                format!("{summary}\n\n{formatted}")
            };

            text_response(req, &response)
        }
        Err(e) => error_response(req, &format!("Search failed: {e}")),
    }
}

fn handle_add(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let Some(title) = args.get("title").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: title");
    };
    let Some(body) = args.get("body").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: body");
    };

    if let Some(err) = check_limit(req, title, "title", MAX_TITLE_BYTES) {
        return err;
    }
    if let Some(err) = check_limit(req, body, "body", MAX_BODY_BYTES) {
        return err;
    }

    let tags: Vec<&str> = args
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if let Some(tags_val) = args.get("tags") {
        let serialised = serde_json::to_string(tags_val).unwrap_or_default();
        if serialised.len() > MAX_TAGS_BYTES {
            return error_response(
                req,
                &format!("tags exceeds maximum serialised size of {MAX_TAGS_BYTES} bytes"),
            );
        }
    }

    eprintln!("[lore] Add pattern: \"{title}\"");

    match ingest::add_pattern(
        ctx.db,
        ctx.embedder,
        &ctx.config.knowledge_dir,
        title,
        body,
        &tags,
        ctx.config.inbox_branch_prefix(),
    ) {
        Ok(result) => {
            let cn = commit_note(&result.commit_status);
            let embed_note = embedding_note(result.embedding_failures);
            text_response(
                req,
                &format!(
                    "Pattern \"{}\" saved to {} ({} chunks indexed{}{embed_note}).",
                    title, result.file_path, result.chunks_indexed, cn
                ),
            )
        }
        Err(e) => error_response(req, &format!("Failed to add pattern: {e}")),
    }
}

fn handle_update(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let Some(source_file) = args.get("source_file").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: source_file");
    };
    let Some(body) = args.get("body").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: body");
    };

    if let Some(err) = check_limit(req, source_file, "source_file", MAX_SOURCE_FILE_BYTES) {
        return err;
    }
    if let Some(err) = check_limit(req, body, "body", MAX_BODY_BYTES) {
        return err;
    }

    let tags: Vec<&str> = args
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if let Some(tags_val) = args.get("tags") {
        let serialised = serde_json::to_string(tags_val).unwrap_or_default();
        if serialised.len() > MAX_TAGS_BYTES {
            return error_response(
                req,
                &format!("tags exceeds maximum serialised size of {MAX_TAGS_BYTES} bytes"),
            );
        }
    }

    eprintln!("[lore] Update pattern: \"{source_file}\"");

    match ingest::update_pattern(
        ctx.db,
        ctx.embedder,
        &ctx.config.knowledge_dir,
        source_file,
        body,
        &tags,
        ctx.config.inbox_branch_prefix(),
    ) {
        Ok(result) => {
            let cn = commit_note(&result.commit_status);
            let embed_note = embedding_note(result.embedding_failures);
            text_response(
                req,
                &format!(
                    "Pattern {} updated ({} chunks re-indexed{}{embed_note}).",
                    result.file_path, result.chunks_indexed, cn
                ),
            )
        }
        Err(e) => error_response(req, &format!("Failed to update pattern: {e}")),
    }
}

fn handle_append(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let Some(source_file) = args.get("source_file").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: source_file");
    };
    let Some(heading) = args.get("heading").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: heading");
    };
    let Some(body) = args.get("body").and_then(Value::as_str) else {
        return error_response(req, "Missing required field: body");
    };

    if let Some(err) = check_limit(req, source_file, "source_file", MAX_SOURCE_FILE_BYTES) {
        return err;
    }
    if let Some(err) = check_limit(req, heading, "heading", MAX_HEADING_BYTES) {
        return err;
    }
    if let Some(err) = check_limit(req, body, "body", MAX_BODY_BYTES) {
        return err;
    }

    eprintln!("[lore] Append to: \"{source_file}\" -- {heading}");

    match ingest::append_to_pattern(
        ctx.db,
        ctx.embedder,
        &ctx.config.knowledge_dir,
        source_file,
        heading,
        body,
        ctx.config.inbox_branch_prefix(),
    ) {
        Ok(result) => {
            let cn = commit_note(&result.commit_status);
            let embed_note = embedding_note(result.embedding_failures);
            text_response(
                req,
                &format!(
                    "Section \"{}\" appended to {} ({} chunks re-indexed{}{embed_note}).",
                    heading, result.file_path, result.chunks_indexed, cn
                ),
            )
        }
        Err(e) => error_response(req, &format!("Failed to append: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

fn embedding_note(failures: usize) -> String {
    if failures > 0 {
        format!(
            " ({failures} embedding{} failed)",
            if failures == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    }
}

fn commit_note(status: &CommitStatus) -> String {
    match status {
        CommitStatus::NotCommitted => String::new(),
        CommitStatus::Committed => ", committed to git".to_string(),
        CommitStatus::Pushed { branch } => {
            format!(", pushed to {branch} — pending review")
        }
    }
}

fn text_response(req: &JsonRpcRequest, text: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: Some(json!({
            "content": [{ "type": "text", "text": text }]
        })),
        error: None,
    }
}

fn error_response(req: &JsonRpcRequest, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: None,
        error: Some(json!({ "code": -32000, "message": message })),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;
    use crate::config::Config;
    use crate::database::KnowledgeDB;
    use crate::embeddings::FakeEmbedder;

    /// Build an in-memory test context: DB, embedder, config, and tempdir.
    struct TestHarness {
        db: KnowledgeDB,
        embedder: FakeEmbedder,
        config: Config,
        _tmp: tempfile::TempDir,
    }

    impl TestHarness {
        fn new() -> Self {
            let tmp = tempdir().unwrap();
            let embedder = FakeEmbedder::with_dimensions(4);
            let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
            db.init().unwrap();
            let config = Config::default_with(
                tmp.path().to_path_buf(),
                PathBuf::from(":memory:"),
                "test-model",
            );
            Self {
                db,
                embedder,
                config,
                _tmp: tmp,
            }
        }

        fn ctx(&self) -> ServerContext<'_> {
            ServerContext {
                db: &self.db,
                embedder: &self.embedder,
                config: &self.config,
            }
        }

        /// Send a JSON string through `handle_request` and return the response.
        fn request(&self, json: &str) -> Option<JsonRpcResponse> {
            let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
            handle_request(&req, &self.ctx())
        }

        /// Send a request and return the response as a `serde_json::Value`.
        fn request_value(&self, json: &str) -> Value {
            let resp = self.request(json).expect("expected a response");
            serde_json::to_value(&resp).unwrap()
        }
    }

    // -- initialize --------------------------------------------------------

    #[test]
    fn initialize_response() {
        let h = TestHarness::new();
        let resp = h.request_value(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        insta::assert_json_snapshot!(resp, @r#"
        {
          "id": 1,
          "jsonrpc": "2.0",
          "result": {
            "capabilities": {
              "tools": {}
            },
            "protocolVersion": "2024-11-05",
            "serverInfo": {
              "name": "lore",
              "version": "0.1.0"
            }
          }
        }
        "#);
    }

    // -- tools/list --------------------------------------------------------

    #[test]
    fn tools_list_returns_all_four_tools() {
        let h = TestHarness::new();
        let resp = h.request_value(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#);

        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            vec![
                "search_patterns",
                "add_pattern",
                "update_pattern",
                "append_to_pattern"
            ]
        );

        insta::assert_json_snapshot!(resp);
    }

    // -- search_patterns ---------------------------------------------------

    #[test]
    fn search_patterns_returns_results() {
        let h = TestHarness::new();

        // Insert a chunk so there is something to find.
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Error Handling".into(),
            body: "Always use anyhow for application errors".into(),
            tags: "rust".into(),
            source_file: "patterns.md".into(),
            heading_path: String::new(),
        };
        let emb = h.embedder.embed(&chunk.body).unwrap();
        h.db.insert_chunk(&chunk, Some(&emb)).unwrap();

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"error handling"}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Error Handling"),
            "search result should contain the title, got: {text}"
        );
    }

    // -- add_pattern -------------------------------------------------------

    #[test]
    fn add_pattern_creates_pattern() {
        let h = TestHarness::new();
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":4,"method":"tools/call",
                "params":{
                    "name":"add_pattern",
                    "arguments":{
                        "title":"Test Pattern",
                        "body":"Body text that is long enough for a chunk."
                    }
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Test Pattern"),
            "response should mention the pattern title, got: {text}"
        );
        assert!(
            text.contains("saved to"),
            "response should confirm save, got: {text}"
        );
    }

    // -- unknown method ----------------------------------------------------

    #[test]
    fn unknown_method_returns_error_32601() {
        let h = TestHarness::new();
        let resp =
            h.request_value(r#"{"jsonrpc":"2.0","id":5,"method":"bogus/method","params":{}}"#);

        assert_eq!(resp["error"]["code"], -32601);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("bogus/method")
        );
    }

    // -- unknown tool ------------------------------------------------------

    #[test]
    fn unknown_tool_returns_error_32602() {
        let h = TestHarness::new();
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":6,"method":"tools/call",
                "params":{"name":"nonexistent_tool","arguments":{}}
            }"#,
        );

        assert_eq!(resp["error"]["code"], -32602);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("nonexistent_tool")
        );
    }

    // -- jsonrpc version validation ----------------------------------------

    #[test]
    fn invalid_jsonrpc_version_returns_error() {
        let h = TestHarness::new();
        let resp = h.request_value(r#"{"jsonrpc":"1.0","id":7,"method":"initialize","params":{}}"#);

        assert_eq!(resp["error"]["code"], -32600);
        assert!(resp["error"]["message"].as_str().unwrap().contains("1.0"));
    }

    // -- notifications/initialized returns None ----------------------------

    #[test]
    fn notifications_initialized_returns_none() {
        let h = TestHarness::new();
        let resp = h.request(
            r#"{"jsonrpc":"2.0","id":null,"method":"notifications/initialized","params":{}}"#,
        );
        assert!(resp.is_none());
    }

    // -- search_empty_query_returns_message --------------------------------

    #[test]
    fn search_empty_query_returns_message() {
        let h = TestHarness::new();
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":10,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":""}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Please provide a search query"),
            "expected helpful message for empty query, got: {text}"
        );
    }

    // -- search_empty_results ---------------------------------------------

    #[test]
    fn search_empty_results() {
        let h = TestHarness::new();
        // DB is empty, search should say no patterns found.
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":11,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"nonexistent topic"}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("No matching patterns"),
            "expected 'No matching patterns' message, got: {text}"
        );
    }

    // -- search_fts_only_path ---------------------------------------------

    #[test]
    fn search_fts_only_path() {
        let tmp = tempdir().unwrap();
        let embedder = FakeEmbedder::with_dimensions(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
        db.init().unwrap();
        let mut config = Config::default_with(
            tmp.path().to_path_buf(),
            PathBuf::from(":memory:"),
            "test-model",
        );
        // Disable hybrid search.
        config.search.hybrid = false;

        let ctx = ServerContext {
            db: &db,
            embedder: &embedder,
            config: &config,
        };

        // Insert a chunk.
        let chunk = crate::chunking::Chunk {
            id: "fts1".into(),
            title: "FTS Only Test".into(),
            body: "Testing full text search only mode works correctly".into(),
            tags: String::new(),
            source_file: "fts-test.md".into(),
            heading_path: String::new(),
        };
        db.insert_chunk(&chunk, None).unwrap();

        let req: JsonRpcRequest = serde_json::from_str(
            r#"{
                "jsonrpc":"2.0","id":12,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"full text search"}
                }
            }"#,
        )
        .unwrap();

        let resp = handle_request(&req, &ctx).expect("expected a response");
        let val = serde_json::to_value(&resp).unwrap();
        assert!(val["error"].is_null());
        let text = val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("FTS Only Test"),
            "FTS-only search should find the chunk, got: {text}"
        );
    }

    // -- update_pattern_updates_pattern ------------------------------------

    #[test]
    fn update_pattern_updates_pattern() {
        let h = TestHarness::new();

        // Pre-create a file in the knowledge dir.
        let file = h.config.knowledge_dir.join("existing-pattern.md");
        std::fs::write(&file, "# Existing Pattern\n\nOld body content.\n").unwrap();

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":13,"method":"tools/call",
                "params":{
                    "name":"update_pattern",
                    "arguments":{
                        "source_file":"existing-pattern.md",
                        "body":"Brand new updated body content for the pattern."
                    }
                }
            }"#,
        );

        assert!(
            resp["error"].is_null(),
            "update should succeed, got error: {:?}",
            resp["error"]
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("updated"),
            "response should confirm update, got: {text}"
        );

        // Verify file content was updated.
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("Brand new updated body"));
        assert!(content.contains("# Existing Pattern"));
    }

    // -- append_to_pattern_appends_section ---------------------------------

    #[test]
    fn append_to_pattern_appends_section() {
        let h = TestHarness::new();

        // Pre-create a file.
        let file = h.config.knowledge_dir.join("appendable.md");
        std::fs::write(&file, "# Appendable\n\nOriginal body content.\n").unwrap();

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":14,"method":"tools/call",
                "params":{
                    "name":"append_to_pattern",
                    "arguments":{
                        "source_file":"appendable.md",
                        "heading":"New Section",
                        "body":"Appended section body content that is long enough."
                    }
                }
            }"#,
        );

        assert!(
            resp["error"].is_null(),
            "append should succeed, got error: {:?}",
            resp["error"]
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("appended"),
            "response should confirm append, got: {text}"
        );

        // Verify file content was appended.
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("# Appendable"));
        assert!(content.contains("Original body"));
        assert!(content.contains("## New Section"));
        assert!(content.contains("Appended section body"));
    }

    // -- missing_required_field_returns_error ------------------------------

    #[test]
    fn missing_required_field_returns_error() {
        let h = TestHarness::new();

        // Call add_pattern with empty arguments (no title or body).
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":15,"method":"tools/call",
                "params":{
                    "name":"add_pattern",
                    "arguments":{}
                }
            }"#,
        );

        assert!(!resp["error"].is_null());
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("Missing required field"),
            "expected missing field error, got: {msg}"
        );
    }

    // -- MCP round-trip (chained operations) ---------------------------------

    #[test]
    #[allow(clippy::too_many_lines)]
    fn mcp_round_trip() {
        let h = TestHarness::new();

        // -- initialize -------------------------------------------------------
        let resp =
            h.request_value(r#"{"jsonrpc":"2.0","id":100,"method":"initialize","params":{}}"#);
        assert!(resp["error"].is_null());
        assert_eq!(resp["result"]["serverInfo"]["name"], "lore");

        // -- tools/list -------------------------------------------------------
        let resp =
            h.request_value(r#"{"jsonrpc":"2.0","id":101,"method":"tools/list","params":{}}"#);
        assert!(resp["error"].is_null());
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);

        // -- add_pattern ------------------------------------------------------
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":102,"method":"tools/call",
                "params":{
                    "name":"add_pattern",
                    "arguments":{
                        "title":"Concurrency Guidelines",
                        "body":"Use tokio for async runtime. Prefer channels over shared mutable state. Always use Arc for cross-task ownership."
                    }
                }
            }"#,
        );
        assert!(
            resp["error"].is_null(),
            "add_pattern should succeed, got: {:?}",
            resp["error"]
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("saved to"),
            "response should confirm save, got: {text}"
        );

        // -- search_patterns finds the added pattern --------------------------
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":103,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"tokio channels"}
                }
            }"#,
        );
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Concurrency Guidelines"),
            "search should find added pattern, got: {text}"
        );

        // -- update_pattern ---------------------------------------------------
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":104,"method":"tools/call",
                "params":{
                    "name":"update_pattern",
                    "arguments":{
                        "source_file":"concurrency-guidelines.md",
                        "body":"Use tokio as the async runtime. Prefer message passing via flume channels. Avoid blocking the executor with synchronous IO."
                    }
                }
            }"#,
        );
        assert!(
            resp["error"].is_null(),
            "update_pattern should succeed, got: {:?}",
            resp["error"]
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("updated"),
            "response should confirm update, got: {text}"
        );

        // -- search_patterns reflects update ----------------------------------
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":105,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"flume channels"}
                }
            }"#,
        );
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("flume"),
            "search should find updated content, got: {text}"
        );

        // -- append_to_pattern ------------------------------------------------
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":106,"method":"tools/call",
                "params":{
                    "name":"append_to_pattern",
                    "arguments":{
                        "source_file":"concurrency-guidelines.md",
                        "heading":"Synchronisation Primitives",
                        "body":"Use RwLock for read-heavy workloads. Prefer parking_lot over std locks for better performance."
                    }
                }
            }"#,
        );
        assert!(
            resp["error"].is_null(),
            "append should succeed, got: {:?}",
            resp["error"]
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("appended"),
            "response should confirm append, got: {text}"
        );

        // -- search_patterns finds appended content ---------------------------
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":107,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"parking_lot"}
                }
            }"#,
        );
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("parking_lot"),
            "search should find appended content, got: {text}"
        );
    }

    // -- inbox branch response formatting ---------------------------------

    /// Build a test harness with a git repo, bare remote, and inbox config.
    fn harness_with_inbox() -> TestHarness {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // Init git repo with remote.
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git command failed");
        }

        let bare = tempdir().unwrap();
        Command::new("git")
            .args(["init", "--bare"])
            .current_dir(bare.path())
            .output()
            .expect("bare init failed");
        Command::new("git")
            .args(["remote", "add", "origin", &bare.path().to_string_lossy()])
            .current_dir(dir)
            .output()
            .expect("remote add failed");

        // Initial commit so HEAD exists.
        std::fs::write(dir.join("README"), "init\n").unwrap();
        Command::new("git")
            .args(["add", "README"])
            .current_dir(dir)
            .output()
            .expect("add failed");
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir)
            .output()
            .expect("commit failed");
        Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(dir)
            .output()
            .expect("push failed");

        let embedder = FakeEmbedder::with_dimensions(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
        db.init().unwrap();
        let mut config =
            Config::default_with(dir.to_path_buf(), PathBuf::from(":memory:"), "test-model");
        config.git = Some(crate::config::GitConfig {
            inbox_branch_prefix: "inbox/".to_string(),
        });

        // Keep both tempdirs alive via _tmp (bare is moved into the struct
        // indirectly — it will be dropped when the test ends since we leak
        // it here to keep the remote alive).
        std::mem::forget(bare);

        TestHarness {
            db,
            embedder,
            config,
            _tmp: tmp,
        }
    }

    #[test]
    fn add_pattern_with_inbox_returns_pending_review() {
        let h = harness_with_inbox();
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":200,"method":"tools/call",
                "params":{
                    "name":"add_pattern",
                    "arguments":{
                        "title":"Inbox Test",
                        "body":"Body for inbox testing.",
                        "tags":["test"]
                    }
                }
            }"#,
        );
        assert!(
            resp["error"].is_null(),
            "add_pattern should succeed, got: {:?}",
            resp["error"]
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("pending review"),
            "response should indicate pending review, got: {text}"
        );
        assert!(
            text.contains("inbox/"),
            "response should include branch name, got: {text}"
        );
    }

    // -- embed failure warning ------------------------------------------------

    #[test]
    fn search_with_failing_embedder_shows_warning() {
        let tmp = tempdir().unwrap();
        let failing = crate::embeddings::FailingEmbedder::new(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), failing.dimensions()).unwrap();
        db.init().unwrap();
        let config = Config::default_with(
            tmp.path().to_path_buf(),
            PathBuf::from(":memory:"),
            "test-model",
        );

        // Insert a chunk so FTS has something to return.
        let chunk = crate::chunking::Chunk {
            id: "warn1".into(),
            title: "Warning Test".into(),
            body: "Content for the embed failure warning test".into(),
            tags: String::new(),
            source_file: "warn.md".into(),
            heading_path: String::new(),
        };
        db.insert_chunk(&chunk, None).unwrap();

        let ctx = ServerContext {
            db: &db,
            embedder: &failing,
            config: &config,
        };

        let req: JsonRpcRequest = serde_json::from_str(
            r#"{
                "jsonrpc":"2.0","id":20,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"embed failure"}
                }
            }"#,
        )
        .unwrap();

        let resp = handle_request(&req, &ctx).expect("expected a response");
        let val = serde_json::to_value(&resp).unwrap();
        assert!(val["error"].is_null(), "should not be a JSON-RPC error");
        let text = val["result"]["content"][0]["text"].as_str().unwrap();

        assert!(
            text.contains("Ollama unreachable"),
            "response should contain embed failure warning, got: {text}"
        );
        assert!(
            text.contains("text search only"),
            "warning should mention text search fallback, got: {text}"
        );
        assert!(
            text.contains("Warning Test"),
            "FTS results should still be returned, got: {text}"
        );
    }

    #[test]
    fn search_with_working_embedder_has_no_warning() {
        let h = TestHarness::new();

        let chunk = crate::chunking::Chunk {
            id: "nowarn1".into(),
            title: "No Warning Test".into(),
            body: "Content that should be found without any warning".into(),
            tags: String::new(),
            source_file: "nowarn.md".into(),
            heading_path: String::new(),
        };
        let emb = h.embedder.embed(&chunk.body).unwrap();
        h.db.insert_chunk(&chunk, Some(&emb)).unwrap();

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":21,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"content found"}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains("Ollama unreachable"),
            "should not contain warning when embedder works, got: {text}"
        );
    }

    // -- relevance threshold filtering ----------------------------------------

    #[test]
    fn search_filters_low_relevance_results() {
        let mut h = TestHarness::new();
        h.config.search.min_relevance = 0.6;

        // Insert a chunk with embedding so it can participate in hybrid search.
        let chunk = crate::chunking::Chunk {
            id: "rel1".into(),
            title: "Relevant Result".into(),
            body: "Highly relevant content about specific unique topic xylophone".into(),
            tags: String::new(),
            source_file: "relevant.md".into(),
            heading_path: String::new(),
        };
        let emb = h.embedder.embed(&chunk.body).unwrap();
        h.db.insert_chunk(&chunk, Some(&emb)).unwrap();

        // Search for something completely unrelated — the chunk may appear
        // in one list but with a low RRF score below the threshold.
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":22,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"completely unrelated basketball"}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("No matching patterns"),
            "low-relevance results should be filtered, got: {text}"
        );
    }

    #[test]
    fn search_with_zero_threshold_returns_all() {
        let mut h = TestHarness::new();
        h.config.search.min_relevance = 0.0;

        let chunk = crate::chunking::Chunk {
            id: "all1".into(),
            title: "Always Visible".into(),
            body: "Content that should always appear with zero threshold".into(),
            tags: String::new(),
            source_file: "always.md".into(),
            heading_path: String::new(),
        };
        let emb = h.embedder.embed(&chunk.body).unwrap();
        h.db.insert_chunk(&chunk, Some(&emb)).unwrap();

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":23,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"unrelated query"}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Always Visible"),
            "zero threshold should return all results, got: {text}"
        );
    }

    #[test]
    fn search_fts_only_bypasses_threshold() {
        let tmp = tempdir().unwrap();
        let embedder = FakeEmbedder::with_dimensions(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
        db.init().unwrap();
        let mut config = Config::default_with(
            tmp.path().to_path_buf(),
            PathBuf::from(":memory:"),
            "test-model",
        );
        config.search.hybrid = false;
        config.search.min_relevance = 0.6;

        let chunk = crate::chunking::Chunk {
            id: "fts2".into(),
            title: "FTS Bypass".into(),
            body: "FTS results should bypass threshold filtering entirely".into(),
            tags: String::new(),
            source_file: "fts-bypass.md".into(),
            heading_path: String::new(),
        };
        db.insert_chunk(&chunk, None).unwrap();

        let ctx = ServerContext {
            db: &db,
            embedder: &embedder,
            config: &config,
        };

        let req: JsonRpcRequest = serde_json::from_str(
            r#"{
                "jsonrpc":"2.0","id":24,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"FTS results bypass"}
                }
            }"#,
        )
        .unwrap();

        let resp = handle_request(&req, &ctx).expect("expected a response");
        let val = serde_json::to_value(&resp).unwrap();
        assert!(val["error"].is_null());
        let text = val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("FTS Bypass"),
            "FTS-only mode should bypass threshold, got: {text}"
        );
    }

    #[test]
    fn search_embed_failure_bypasses_threshold() {
        let tmp = tempdir().unwrap();
        let failing = crate::embeddings::FailingEmbedder::new(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), failing.dimensions()).unwrap();
        db.init().unwrap();
        let mut config = Config::default_with(
            tmp.path().to_path_buf(),
            PathBuf::from(":memory:"),
            "test-model",
        );
        config.search.min_relevance = 0.6;

        let chunk = crate::chunking::Chunk {
            id: "fallback1".into(),
            title: "Fallback Result".into(),
            body: "This should appear even with threshold when embed fails".into(),
            tags: String::new(),
            source_file: "fallback.md".into(),
            heading_path: String::new(),
        };
        db.insert_chunk(&chunk, None).unwrap();

        let ctx = ServerContext {
            db: &db,
            embedder: &failing,
            config: &config,
        };

        let req: JsonRpcRequest = serde_json::from_str(
            r#"{
                "jsonrpc":"2.0","id":25,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"fallback result"}
                }
            }"#,
        )
        .unwrap();

        let resp = handle_request(&req, &ctx).expect("expected a response");
        let val = serde_json::to_value(&resp).unwrap();
        assert!(val["error"].is_null());
        let text = val["result"]["content"][0]["text"].as_str().unwrap();

        assert!(
            text.contains("Ollama unreachable"),
            "should show embed failure warning, got: {text}"
        );
        assert!(
            text.contains("Fallback Result"),
            "FTS fallback should bypass threshold, got: {text}"
        );
    }

    // -- input length validation tests ----------------------------------------

    #[test]
    fn search_query_at_max_length_succeeds() {
        let h = TestHarness::new();

        // Insert a chunk so FTS has something to scan.
        let chunk = crate::chunking::Chunk {
            id: "lim1".into(),
            title: "Limit Test".into(),
            body: "Content for limit boundary testing".into(),
            tags: String::new(),
            source_file: "limit.md".into(),
            heading_path: String::new(),
        };
        h.db.insert_chunk(&chunk, None).unwrap();

        // Exactly 1024 bytes — should succeed.
        let query = "a".repeat(1024);
        let req_json = format!(
            r#"{{"jsonrpc":"2.0","id":300,"method":"tools/call","params":{{"name":"search_patterns","arguments":{{"query":"{query}"}}}}}}"#
        );
        let resp = h.request_value(&req_json);
        assert!(
            resp["error"].is_null(),
            "query at exactly 1024 bytes should succeed, got error: {:?}",
            resp["error"]
        );
    }

    #[test]
    fn search_query_over_max_length_returns_error() {
        let h = TestHarness::new();

        // 1025 bytes — should be rejected.
        let query = "a".repeat(1025);
        let req_json = format!(
            r#"{{"jsonrpc":"2.0","id":301,"method":"tools/call","params":{{"name":"search_patterns","arguments":{{"query":"{query}"}}}}}}"#
        );
        let resp = h.request_value(&req_json);
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("query") && msg.contains("1024"),
            "error should mention field and limit, got: {msg}"
        );
    }

    #[test]
    fn search_top_k_at_max_succeeds() {
        let h = TestHarness::new();
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":302,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"test","top_k":100}
                }
            }"#,
        );
        assert!(
            resp["error"].is_null(),
            "top_k=100 should succeed, got error: {:?}",
            resp["error"]
        );
    }

    #[test]
    fn search_top_k_over_max_returns_error() {
        let h = TestHarness::new();
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":303,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"test","top_k":101}
                }
            }"#,
        );
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("top_k") && msg.contains("100"),
            "error should mention top_k limit, got: {msg}"
        );
    }

    #[test]
    fn add_body_at_max_length_succeeds() {
        let h = TestHarness::new();

        // Exactly 256 KB body.
        let body = "x".repeat(262_144);
        let args = serde_json::json!({
            "title": "Big Body Test",
            "body": body
        });
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 304,
            "method": "tools/call",
            "params": { "name": "add_pattern", "arguments": args }
        });
        let resp = h.request_value(&req_json.to_string());
        assert!(
            resp["error"].is_null(),
            "body at exactly 256KB should succeed, got error: {:?}",
            resp["error"]
        );
    }

    #[test]
    fn add_body_over_max_length_returns_error() {
        let h = TestHarness::new();

        // 256 KB + 1 byte.
        let body = "x".repeat(262_145);
        let args = serde_json::json!({
            "title": "Too Big Body",
            "body": body
        });
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 305,
            "method": "tools/call",
            "params": { "name": "add_pattern", "arguments": args }
        });
        let resp = h.request_value(&req_json.to_string());
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("body") && msg.contains("262144"),
            "error should mention body and limit, got: {msg}"
        );
    }

    #[test]
    fn add_oversized_title_returns_error_no_disk_write() {
        let h = TestHarness::new();

        let title = "t".repeat(513);
        let args = serde_json::json!({
            "title": title,
            "body": "Some body content for the pattern."
        });
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 306,
            "method": "tools/call",
            "params": { "name": "add_pattern", "arguments": args }
        });
        let resp = h.request_value(&req_json.to_string());
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("title") && msg.contains("512"),
            "error should mention title and limit, got: {msg}"
        );

        // Verify no file was written to knowledge dir.
        let entries: Vec<_> = std::fs::read_dir(&h.config.knowledge_dir)
            .unwrap()
            .collect();
        assert!(
            entries.is_empty(),
            "no file should be written for oversized title"
        );
    }

    #[test]
    fn update_oversized_source_file_returns_error() {
        let h = TestHarness::new();

        let source = "s".repeat(513);
        let args = serde_json::json!({
            "source_file": source,
            "body": "New body."
        });
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 307,
            "method": "tools/call",
            "params": { "name": "update_pattern", "arguments": args }
        });
        let resp = h.request_value(&req_json.to_string());
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("source_file") && msg.contains("512"),
            "error should mention source_file and limit, got: {msg}"
        );
    }

    #[test]
    fn append_oversized_heading_returns_error() {
        let h = TestHarness::new();

        let heading = "h".repeat(513);
        let args = serde_json::json!({
            "source_file": "some-file.md",
            "heading": heading,
            "body": "Appended content."
        });
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 308,
            "method": "tools/call",
            "params": { "name": "append_to_pattern", "arguments": args }
        });
        let resp = h.request_value(&req_json.to_string());
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("heading") && msg.contains("512"),
            "error should mention heading and limit, got: {msg}"
        );
    }

    #[test]
    fn add_oversized_tags_returns_error() {
        let h = TestHarness::new();

        // Build a tags array whose serialised JSON exceeds 8 KB.
        let big_tag = "t".repeat(4096);
        let args = serde_json::json!({
            "title": "Tags Test",
            "body": "Body for tags test.",
            "tags": [big_tag.clone(), big_tag]
        });
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 309,
            "method": "tools/call",
            "params": { "name": "add_pattern", "arguments": args }
        });
        let resp = h.request_value(&req_json.to_string());
        assert_eq!(resp["error"]["code"], -32000);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("tags") && msg.contains("8192"),
            "error should mention tags and limit, got: {msg}"
        );
    }
}
