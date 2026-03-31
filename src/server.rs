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
// Tool handlers
// ---------------------------------------------------------------------------

fn handle_search(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");

    if query.is_empty() {
        return text_response(req, "Please provide a search query.");
    }

    #[allow(clippy::cast_possible_truncation)]
    let top_k = args
        .get("top_k")
        .and_then(Value::as_u64)
        .map_or(ctx.config.search.top_k, |k| k as usize);

    eprintln!("[lore] Search: \"{query}\" (top_k={top_k})");

    let results = if ctx.config.search.hybrid {
        let query_embedding = ctx.embedder.embed(query).ok();
        ctx.db
            .search_hybrid(query, query_embedding.as_deref(), top_k)
    } else {
        ctx.db
            .search_fts(query, top_k)
            .map(|r| r.into_iter().take(top_k).collect())
    };

    match results {
        Ok(results) => {
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

            text_response(req, &format!("{summary}\n\n{formatted}"))
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
    let tags: Vec<&str> = args
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    eprintln!("[lore] Add pattern: \"{title}\"");

    match ingest::add_pattern(
        ctx.db,
        ctx.embedder,
        &ctx.config.knowledge_dir,
        title,
        body,
        &tags,
    ) {
        Ok(result) => {
            let commit_note = if result.committed {
                ", committed to git"
            } else {
                ""
            };
            let embed_note = if result.embedding_failures > 0 {
                format!(
                    " ({} embedding{} failed)",
                    result.embedding_failures,
                    if result.embedding_failures == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            } else {
                String::new()
            };
            text_response(
                req,
                &format!(
                    "Pattern \"{}\" saved to {} ({} chunks indexed{}{embed_note}).",
                    title, result.file_path, result.chunks_indexed, commit_note
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
    let tags: Vec<&str> = args
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    eprintln!("[lore] Update pattern: \"{source_file}\"");

    match ingest::update_pattern(
        ctx.db,
        ctx.embedder,
        &ctx.config.knowledge_dir,
        source_file,
        body,
        &tags,
    ) {
        Ok(result) => {
            let commit_note = if result.committed {
                ", committed to git"
            } else {
                ""
            };
            let embed_note = if result.embedding_failures > 0 {
                format!(
                    " ({} embedding{} failed)",
                    result.embedding_failures,
                    if result.embedding_failures == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            } else {
                String::new()
            };
            text_response(
                req,
                &format!(
                    "Pattern {} updated ({} chunks re-indexed{}{embed_note}).",
                    result.file_path, result.chunks_indexed, commit_note
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

    eprintln!("[lore] Append to: \"{source_file}\" -- {heading}");

    match ingest::append_to_pattern(
        ctx.db,
        ctx.embedder,
        &ctx.config.knowledge_dir,
        source_file,
        heading,
        body,
    ) {
        Ok(result) => {
            let commit_note = if result.committed {
                ", committed to git"
            } else {
                ""
            };
            let embed_note = if result.embedding_failures > 0 {
                format!(
                    " ({} embedding{} failed)",
                    result.embedding_failures,
                    if result.embedding_failures == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            } else {
                String::new()
            };
            text_response(
                req,
                &format!(
                    "Section \"{}\" appended to {} ({} chunks re-indexed{}{embed_note}).",
                    heading, result.file_path, result.chunks_indexed, commit_note
                ),
            )
        }
        Err(e) => error_response(req, &format!("Failed to append: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

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
        let resp = h.request_value(
            r#"{"jsonrpc":"2.0","id":100,"method":"initialize","params":{}}"#,
        );
        assert!(resp["error"].is_null());
        assert_eq!(resp["result"]["serverInfo"]["name"], "lore");

        // -- tools/list -------------------------------------------------------
        let resp = h.request_value(
            r#"{"jsonrpc":"2.0","id":101,"method":"tools/list","params":{}}"#,
        );
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
}
