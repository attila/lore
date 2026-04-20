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
use crate::git;
use crate::ingest;
use crate::ingest::CommitStatus;
use crate::lockfile::{WriteLock, lock_path_for};

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
                    },
                    "include_metadata": {
                        "type": "boolean",
                        "description":
                            "When true, appends a `lore-metadata` fenced code block to the \
                             end of the response containing machine-readable JSON with per-row \
                             rank/source_file/score and a top-level mode field \
                             ('hybrid' | 'fts_fallback' | 'fts_only'). Defaults to false. \
                             Opt-in because the fenced block bloats the response and most \
                             callers only need the human-readable prose.",
                        "default": false
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "add_pattern",
            "description":
                "Create a new pattern in the knowledge base. Use only when the user explicitly \
                 asks to save, record, or document a pattern. Creates a markdown file and indexes \
                 it; the change is committed to git when the knowledge base is a git repository, \
                 otherwise the file is written without a commit.",
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
                    },
                    "include_metadata": {
                        "type": "boolean",
                        "description":
                            "When true, appends a `lore-metadata` fenced code block to the \
                             end of the response containing machine-readable JSON with the \
                             written file path, chunk count, and commit status. Defaults to false.",
                        "default": false
                    }
                },
                "required": ["title", "body"]
            }
        },
        {
            "name": "update_pattern",
            "description":
                "Replace the content of an existing pattern. Use only when the user explicitly \
                 asks to update or rewrite a pattern. Overwrites the file and re-indexes; the \
                 change is committed to git when the knowledge base is a git repository, \
                 otherwise the file is written without a commit.",
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
                    },
                    "include_metadata": {
                        "type": "boolean",
                        "description":
                            "When true, appends a `lore-metadata` fenced code block to the \
                             end of the response containing machine-readable JSON with the \
                             updated file path, chunk count, and commit status. Defaults to false.",
                        "default": false
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
                 heading and body and re-indexes; the change is committed to git when the \
                 knowledge base is a git repository, otherwise the file is written without a commit.",
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
                    },
                    "include_metadata": {
                        "type": "boolean",
                        "description":
                            "When true, appends a `lore-metadata` fenced code block to the \
                             end of the response containing machine-readable JSON with the \
                             updated file path, chunk count, and commit status. Defaults to false.",
                        "default": false
                    }
                },
                "required": ["source_file", "heading", "body"]
            }
        },
        {
            "name": "lore_status",
            "description":
                "Report knowledge base health: whether it is a git repository, the indexed \
                 chunk and source counts, the last ingested commit (if any), whether the \
                 inbox branch workflow is configured, and whether a .loreignore file is \
                 active (filtering files out of the index). Use this before write operations \
                 to verify the knowledge base is in the expected state, especially when the \
                 agent needs to know whether changes will be committed to git or whether \
                 files in a particular path are excluded from search.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_metadata": {
                        "type": "boolean",
                        "description":
                            "When true, appends a `lore-metadata` fenced code block to the \
                             end of the response containing machine-readable JSON with \
                             knowledge_dir, git_repository, last_ingested_commit, \
                             chunks_indexed, sources_indexed, inbox_workflow_configured, \
                             delta_ingest_available, and loreignore_active fields. \
                             Defaults to false.",
                        "default": false
                    }
                },
                "required": []
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
        "lore_status" => handle_lore_status(req, ctx, &arguments),
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

/// Return an error response if the serialised `tags` array exceeds the limit.
fn check_tags_limit(req: &JsonRpcRequest, args: &Value) -> Option<JsonRpcResponse> {
    if let Some(tags_val) = args.get("tags") {
        let serialised = serde_json::to_string(tags_val).unwrap_or_default();
        if serialised.len() > MAX_TAGS_BYTES {
            return Some(error_response(
                req,
                &format!("tags exceeds maximum serialised size of {MAX_TAGS_BYTES} bytes"),
            ));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

fn handle_search(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    let include_metadata = include_metadata_arg(args);

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

            let mode = SearchMode::from_search_state(ctx.config.search.hybrid, embed_failed);
            let metadata = build_search_metadata(query, top_k, mode, &results);
            let response_with_fence =
                maybe_append_lore_metadata_fence(response, &metadata, include_metadata);
            text_response(req, &response_with_fence)
        }
        Err(e) => error_response(req, &format!("Search failed: {e}")),
    }
}

fn handle_add(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let include_metadata = include_metadata_arg(args);
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

    if let Some(err) = check_tags_limit(req, args) {
        return err;
    }

    eprintln!("[lore] Add pattern: \"{title}\"");

    let mut write_lock = match WriteLock::open(&lock_path_for(&ctx.config.database)) {
        Ok(l) => l,
        Err(e) => return error_response(req, &format!("Failed to open write lock: {e}")),
    };
    let _guard = match write_lock.acquire() {
        Ok(g) => g,
        Err(e) => return error_response(req, &format!("Failed to acquire write lock: {e}")),
    };

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
            let metadata = json!({
                "file_path": result.file_path,
                "chunks_indexed": result.chunks_indexed,
                "embedding_failures": result.embedding_failures,
                "commit_status": commit_status_metadata(&result.commit_status),
            });
            let prose = format!(
                "Pattern \"{}\" saved to {} ({} chunks indexed{}{embed_note}).",
                title, result.file_path, result.chunks_indexed, cn
            );
            let prose_with_fence =
                maybe_append_lore_metadata_fence(prose, &metadata, include_metadata);
            text_response(req, &prose_with_fence)
        }
        Err(e) => error_response(req, &format!("Failed to add pattern: {e}")),
    }
}

fn handle_update(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let include_metadata = include_metadata_arg(args);
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

    if let Some(err) = check_tags_limit(req, args) {
        return err;
    }

    eprintln!("[lore] Update pattern: \"{source_file}\"");

    let mut write_lock = match WriteLock::open(&lock_path_for(&ctx.config.database)) {
        Ok(l) => l,
        Err(e) => return error_response(req, &format!("Failed to open write lock: {e}")),
    };
    let _guard = match write_lock.acquire() {
        Ok(g) => g,
        Err(e) => return error_response(req, &format!("Failed to acquire write lock: {e}")),
    };

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
            let metadata = json!({
                "file_path": result.file_path,
                "chunks_indexed": result.chunks_indexed,
                "embedding_failures": result.embedding_failures,
                "commit_status": commit_status_metadata(&result.commit_status),
            });
            let prose = format!(
                "Pattern {} updated ({} chunks re-indexed{}{embed_note}).",
                result.file_path, result.chunks_indexed, cn
            );
            let prose_with_fence =
                maybe_append_lore_metadata_fence(prose, &metadata, include_metadata);
            text_response(req, &prose_with_fence)
        }
        Err(e) => error_response(req, &format!("Failed to update pattern: {e}")),
    }
}

fn handle_append(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let include_metadata = include_metadata_arg(args);
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

    let mut write_lock = match WriteLock::open(&lock_path_for(&ctx.config.database)) {
        Ok(l) => l,
        Err(e) => return error_response(req, &format!("Failed to open write lock: {e}")),
    };
    let _guard = match write_lock.acquire() {
        Ok(g) => g,
        Err(e) => return error_response(req, &format!("Failed to acquire write lock: {e}")),
    };

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
            let metadata = json!({
                "file_path": result.file_path,
                "chunks_indexed": result.chunks_indexed,
                "embedding_failures": result.embedding_failures,
                "commit_status": commit_status_metadata(&result.commit_status),
            });
            let prose = format!(
                "Section \"{}\" appended to {} ({} chunks re-indexed{}{embed_note}).",
                heading, result.file_path, result.chunks_indexed, cn
            );
            let prose_with_fence =
                maybe_append_lore_metadata_fence(prose, &metadata, include_metadata);
            text_response(req, &prose_with_fence)
        }
        Err(e) => error_response(req, &format!("Failed to append: {e}")),
    }
}

/// Report knowledge base health: git repository status, indexed counts, and
/// inbox workflow configuration. Designed for agents that need to know whether
/// pending writes will be committed before they call `add_pattern`,
/// `update_pattern`, or `append_to_pattern`.
fn handle_lore_status(
    req: &JsonRpcRequest,
    ctx: &ServerContext<'_>,
    args: &Value,
) -> JsonRpcResponse {
    let include_metadata = include_metadata_arg(args);
    let is_git_repo = git::is_git_repo(&ctx.config.knowledge_dir);
    let last_commit = ctx
        .db
        .get_metadata(crate::ingest::META_LAST_COMMIT)
        .ok()
        .flatten();
    let stats = ctx.db.stats().ok();
    let chunks = stats.as_ref().map(|s| s.chunks);
    let sources = stats.as_ref().map(|s| s.sources);
    let inbox_workflow_configured = ctx.config.inbox_branch_prefix().is_some();
    let delta_ingest_available = is_git_repo && last_commit.is_some();
    // Reflect whether a .loreignore file is currently active so agents
    // inspecting knowledge base health know that what they see in search is
    // a filtered view of the underlying directory.
    let loreignore_active = crate::loreignore::load(&ctx.config.knowledge_dir)
        .matcher
        .is_some();

    let metadata = json!({
        "knowledge_dir": ctx.config.knowledge_dir.display().to_string(),
        "git_repository": is_git_repo,
        "last_ingested_commit": last_commit,
        "chunks_indexed": chunks,
        "sources_indexed": sources,
        "inbox_workflow_configured": inbox_workflow_configured,
        "delta_ingest_available": delta_ingest_available,
        "loreignore_active": loreignore_active,
    });

    let summary = format!(
        "Knowledge base: {} — {} {} across {} {}. Git repository: {}. \
         Delta ingest: {}. Inbox workflow: {}. .loreignore: {}.",
        ctx.config.knowledge_dir.display(),
        chunks.map_or_else(|| "?".into(), |c| c.to_string()),
        if chunks == Some(1) { "chunk" } else { "chunks" },
        sources.map_or_else(|| "?".into(), |s| s.to_string()),
        if sources == Some(1) {
            "source"
        } else {
            "sources"
        },
        if is_git_repo { "yes" } else { "no" },
        if delta_ingest_available {
            "available"
        } else {
            "unavailable (full ingest only)"
        },
        if inbox_workflow_configured {
            "configured"
        } else {
            "not configured"
        },
        if loreignore_active {
            "active"
        } else {
            "absent"
        },
    );

    let summary_with_fence = maybe_append_lore_metadata_fence(summary, &metadata, include_metadata);
    text_response(req, &summary_with_fence)
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

/// The search mode used to produce a `search_patterns` response.
///
/// The mode distinguishes three orthogonal cases that the prose response
/// body cannot expose:
///
/// - `Hybrid`: full hybrid search with embedder + FTS combined via
///   reciprocal-rank fusion. Scores are comparable across queries.
/// - `FtsFallback`: hybrid was attempted but the embedder was unreachable;
///   silently fell back to FTS-only for this query. Scores use BM25
///   rank, not comparable to `Hybrid`.
/// - `FtsOnly`: deployment is configured for FTS-only via
///   `config.search.hybrid = false`. The embedder was never attempted.
///   Scores use BM25 rank, not comparable to `Hybrid`.
///
/// MCP clients (specifically the `coverage-check` Claude Code skill)
/// consume the `mode` field to decide whether aggregate coverage metrics
/// are meaningful. Only `Hybrid` is comparable; both `FtsFallback` and
/// `FtsOnly` must trigger client-side refusal of cross-query
/// aggregation.
///
/// The `as_str` match is intentionally exhaustive (no wildcard arm) so
/// adding a new variant fails to compile until the JSON serialisation is
/// updated. This mirrors the discipline in `commit_status_metadata`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchMode {
    Hybrid,
    FtsFallback,
    FtsOnly,
}

impl SearchMode {
    /// Derive the search mode from `config.search.hybrid` and the
    /// runtime `embed_failed` flag observed by `handle_search`. The two
    /// inputs are orthogonal: `hybrid_enabled` is a configuration choice
    /// (FTS-only is a deliberate deployment decision); `embed_failed`
    /// reflects whether the embedder was reachable for this specific
    /// request.
    fn from_search_state(hybrid_enabled: bool, embed_failed: bool) -> Self {
        if !hybrid_enabled {
            // Configured FTS-only — `handle_search` never attempts the
            // embedder, so `embed_failed` is necessarily false. The
            // debug_assert documents the invariant; in release builds the
            // outer `if` already routes correctly.
            debug_assert!(
                !embed_failed,
                "embed_failed must be false when hybrid is disabled — the embedder is never attempted in the FTS-only code path"
            );
            Self::FtsOnly
        } else if embed_failed {
            Self::FtsFallback
        } else {
            Self::Hybrid
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::FtsFallback => "fts_fallback",
            Self::FtsOnly => "fts_only",
        }
    }
}

/// Build the structured metadata block consumed by `search_patterns` MCP
/// callers (specifically the `coverage-check` Claude Code skill at
/// `integrations/claude-code/skills/coverage-check/SKILL.md`).
///
/// The `mode` field is the load-bearing signal — see `SearchMode` above
/// for the three values and their meaning. The previous version of this
/// helper exposed a separate `embed_failed` boolean alongside `mode`;
/// that field has been removed because it was strictly derivable from
/// `mode` (true iff `mode == "fts_fallback"`) and the redundancy created
/// a maintenance hazard for clients that branched on either field.
///
/// See `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md`
/// (Unit 2, "Approach" and "Test scenarios") for the full contract.
fn build_search_metadata(
    query: &str,
    top_k: usize,
    mode: SearchMode,
    results: &[crate::database::SearchResult],
) -> Value {
    let result_rows: Vec<Value> = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "rank": i + 1,
                "title": r.title,
                "source_file": r.source_file,
                "score": r.score,
            })
        })
        .collect();
    json!({
        "query": query,
        "top_k": top_k,
        "result_count": results.len(),
        "mode": mode.as_str(),
        "results": result_rows,
    })
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

/// The fence language tag for the machine-readable metadata block embedded
/// in tool response prose. Chosen to be unique enough that collision with
/// legitimate pattern-body content is near-zero.
const LORE_METADATA_FENCE_TAG: &str = "lore-metadata";

/// Conditionally append a fenced `lore-metadata` block to a tool response's
/// prose body, returning the (possibly extended) prose.
///
/// Background: Claude Code's MCP client strips the `result.metadata` sibling
/// from tool responses before surfacing them to the agent — only the content
/// array reaches the model. This helper embeds the structured metadata in the
/// prose body instead, where the agent can read it via `content[0].text` (the
/// standard MCP surface every compliant client forwards).
///
/// The fenced block is opt-in via the `include_metadata` tool parameter so
/// default callers (e.g. the `search` skill, hook-injected prose queries) do
/// not pay the token cost of the embedded JSON. Opt-in callers (e.g. the
/// `coverage-check` skill) pass `include_metadata: true` on every call.
///
/// `serde_json::to_string` escapes newlines and control characters, so the
/// serialised JSON contains no real newline characters. This guarantees the
/// closing fence marker (`\n` + triple backtick) after the JSON payload is
/// unambiguous — the client-side extractor can safely look for `\n` + triple
/// backtick as the end-of-fence delimiter without risk of false matches
/// inside the JSON string.
///
/// See `docs/solutions/best-practices/mcp-metadata-via-fenced-content-block-2026-04-07.md`
/// for the design rationale and `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md`
/// § 'Design pivot: layer 2 finding' for the history.
fn maybe_append_lore_metadata_fence(
    prose: String,
    metadata: &Value,
    include_metadata: bool,
) -> String {
    if !include_metadata {
        return prose;
    }
    let json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
    format!("{prose}\n\n```{LORE_METADATA_FENCE_TAG}\n{json}\n```")
}

/// Read the `include_metadata` boolean from a tool-call `arguments` object,
/// defaulting to `false` when the key is absent or not a boolean. Matches the
/// schema declared in `tool_definitions()` for every tool that supports the
/// opt-in fenced-metadata channel.
fn include_metadata_arg(args: &Value) -> bool {
    args.get("include_metadata")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Render a `CommitStatus` as a structured JSON value for MCP response metadata.
///
/// The match is intentionally exhaustive (no wildcard arm) so adding a new
/// `CommitStatus` variant fails to compile until this function is updated.
/// MCP consumers branch on `commit_status.kind`, so a new variant must
/// produce a documented JSON shape rather than silently mapping to "unknown".
fn commit_status_metadata(status: &CommitStatus) -> Value {
    match status {
        CommitStatus::NotCommitted => json!({ "kind": "not_committed" }),
        CommitStatus::Committed => json!({ "kind": "committed" }),
        CommitStatus::Pushed { branch } => json!({ "kind": "pushed", "branch": branch }),
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
    use std::path::Path;
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
                tmp.path().join("lore-test.db"),
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

    /// Extract the JSON payload from a `lore-metadata` fenced code block
    /// embedded at the end of a tool response's prose body. Returns `None`
    /// if no fence is present (e.g. the caller did not pass
    /// `include_metadata: true`).
    ///
    /// The extractor mirrors the skill-side parsing logic: locate the last
    /// occurrence of the opening fence marker, advance past it, then find
    /// the next newline-triple-backtick closing marker. Because
    /// `serde_json::to_string` escapes newlines as `\n`, the serialised
    /// metadata payload contains no real newline characters, so the first
    /// newline followed by triple-backtick after the opening fence is
    /// guaranteed to be our closing marker (not a false match inside the
    /// JSON).
    fn extract_lore_metadata_fence(text: &str) -> Option<Value> {
        let opening = format!("\n\n```{LORE_METADATA_FENCE_TAG}\n");
        let start = text.rfind(&opening)?;
        let after_opening = &text[start + opening.len()..];
        let end = after_opening.find("\n```")?;
        let json_str = &after_opening[..end];
        serde_json::from_str(json_str).ok()
    }

    /// Pull the metadata fence from a successful tool response. Fails loudly
    /// if the fence is absent — tests that use this helper are pinning the
    /// `include_metadata: true` contract.
    fn metadata_from_response(resp: &Value) -> Value {
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("response should have a content[0].text field");
        extract_lore_metadata_fence(text)
            .expect("expected a `lore-metadata` fence in the response prose")
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
    fn tools_list_returns_all_five_tools() {
        let h = TestHarness::new();
        let resp = h.request_value(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#);

        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            vec![
                "search_patterns",
                "add_pattern",
                "update_pattern",
                "append_to_pattern",
                "lore_status"
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
            is_universal: false,
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

    /// Build a `TestHarness` whose embedder always fails. Used to pin the
    /// `mode: "fts_fallback"` branch on the search response metadata.
    fn failing_embedder_harness() -> (
        KnowledgeDB,
        crate::embeddings::FailingEmbedder,
        Config,
        tempfile::TempDir,
    ) {
        let tmp = tempdir().unwrap();
        let embedder = crate::embeddings::FailingEmbedder::new(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
        db.init().unwrap();
        let config = Config::default_with(
            tmp.path().to_path_buf(),
            tmp.path().join("lore-test.db"),
            "test-model",
        );
        (db, embedder, config, tmp)
    }

    fn dispatch_value(
        db: &KnowledgeDB,
        embedder: &dyn Embedder,
        config: &Config,
        json: &str,
    ) -> Value {
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        let ctx = ServerContext {
            db,
            embedder,
            config,
        };
        let resp = handle_request(&req, &ctx).expect("expected a response");
        serde_json::to_value(&resp).unwrap()
    }

    /// Pin the `search_patterns` metadata block contract when the caller
    /// opts in via `include_metadata: true`: hybrid mode, results present,
    /// per-row `rank`/`source_file`/`score`, top-level
    /// `query`/`top_k`/`result_count`/`mode` fields, carried via a
    /// `lore-metadata` fenced code block at the end of the prose body.
    /// The skill that consumes this metadata depends on these field names —
    /// changing the shape requires updating the skill prompt at
    /// `integrations/claude-code/skills/coverage-check/SKILL.md`.
    ///
    /// Channel history: the metadata was previously carried in a sibling
    /// field on `result` (`result.metadata`) alongside `result.content`.
    /// That channel was structurally stripped by Claude Code's MCP client
    /// before reaching the agent, so the design pivoted to embedding the
    /// JSON in a fenced code block inside `content[0].text` — the standard
    /// MCP surface every compliant client forwards. See
    /// `docs/solutions/best-practices/mcp-metadata-via-fenced-content-block-2026-04-07.md`
    /// for the finding and the production pattern.
    #[test]
    fn search_patterns_response_metadata_pins_hybrid_shape() {
        // Arrange
        let h = TestHarness::new();
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Cargo Deny".into(),
            body: "Run cargo deny check before every release.".into(),
            tags: "rust".into(),
            source_file: "rust/cargo-deny.md".into(),
            heading_path: String::new(),
            is_universal: false,
        };
        let emb = h.embedder.embed(&chunk.body).unwrap();
        h.db.insert_chunk(&chunk, Some(&emb)).unwrap();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":10,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"cargo deny check","top_k":5,"include_metadata":true}
                }
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);

        // Top-level fields the skill consumes.
        assert_eq!(metadata["mode"], "hybrid");
        assert_eq!(metadata["query"], "cargo deny check");
        assert_eq!(metadata["top_k"], 5);
        assert!(
            metadata["result_count"].as_u64().unwrap() >= 1,
            "expected at least one hit, got result_count = {}",
            metadata["result_count"]
        );

        // Per-row fields the skill consumes.
        let results = metadata["results"].as_array().unwrap();
        assert!(!results.is_empty(), "results array should not be empty");
        let first = &results[0];
        assert_eq!(first["rank"], 1);
        assert_eq!(first["source_file"], "rust/cargo-deny.md");
        assert_eq!(first["title"], "Cargo Deny");
        assert!(
            first["score"].as_f64().unwrap() > 0.0,
            "expected a positive score, got: {}",
            first["score"]
        );

        // Prose body still contains the rendered result row — the fenced
        // block is appended at the end, not interleaved with the prose.
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("[1] Cargo Deny (source: rust/cargo-deny.md)"),
            "prose body should still contain the rendered result row, got: {text}"
        );
    }

    /// When the caller does NOT pass `include_metadata: true`, the response
    /// must not contain a `lore-metadata` fence. Pins the opt-in contract.
    #[test]
    fn search_patterns_omits_metadata_fence_by_default() {
        // Arrange
        let h = TestHarness::new();
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Cargo Deny".into(),
            body: "Run cargo deny check before every release.".into(),
            tags: "rust".into(),
            source_file: "rust/cargo-deny.md".into(),
            heading_path: String::new(),
            is_universal: false,
        };
        let emb = h.embedder.embed(&chunk.body).unwrap();
        h.db.insert_chunk(&chunk, Some(&emb)).unwrap();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":20,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"cargo deny check"}
                }
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains("```lore-metadata"),
            "default search_patterns response must not contain a lore-metadata \
             fence (the fence is opt-in via include_metadata: true), got: {text}"
        );
        assert!(
            extract_lore_metadata_fence(text).is_none(),
            "extract_lore_metadata_fence should return None for a response \
             without the fence, got Some(..)"
        );
    }

    /// Empty result set: `results` array is empty, `result_count` is 0,
    /// `mode` is still `hybrid` (the embedder did not fail; there were just
    /// no matches).
    #[test]
    fn search_patterns_response_metadata_empty_results() {
        let h = TestHarness::new();

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":11,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"nothing matches this","include_metadata":true}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["mode"], "hybrid");
        assert_eq!(metadata["result_count"], 0);
        let results = metadata["results"].as_array().unwrap();
        assert!(results.is_empty(), "results should be empty for no matches");
    }

    /// Multiple chunks for the same source file produce multiple result rows
    /// with the same `source_file` and ascending rank values. The skill
    /// computes "rank of the pattern" as the minimum rank across matching
    /// rows; this test pins the per-row `source_file` field so that
    /// computation is well-defined.
    ///
    /// `min_relevance` is set to 0.0 in this harness to bypass the
    /// production-default 0.6 threshold, which filters `FakeEmbedder`'s
    /// hash-based scores aggressively. The threshold is exercised in
    /// production by real embeddings; here we are pinning the metadata
    /// shape, not the threshold behaviour.
    #[test]
    fn search_patterns_response_metadata_multiple_chunks_same_source() {
        let tmp = tempdir().unwrap();
        let embedder = FakeEmbedder::with_dimensions(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
        db.init().unwrap();
        let mut config = Config::default_with(
            tmp.path().to_path_buf(),
            tmp.path().join("lore-test.db"),
            "test-model",
        );
        config.search.min_relevance = 0.0;

        for (i, body) in [
            "Run cargo deny check on every release",
            "Cargo deny validates licenses and advisories",
        ]
        .iter()
        .enumerate()
        {
            let chunk = crate::chunking::Chunk {
                id: format!("c{i}"),
                title: format!("Cargo Deny Section {i}"),
                body: (*body).to_string(),
                tags: "rust".into(),
                source_file: "rust/cargo-deny.md".into(),
                heading_path: format!("section-{i}"),
                is_universal: false,
            };
            let emb = embedder.embed(&chunk.body).unwrap();
            db.insert_chunk(&chunk, Some(&emb)).unwrap();
        }

        let resp = dispatch_value(
            &db,
            &embedder,
            &config,
            r#"{
                "jsonrpc":"2.0","id":12,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"cargo deny licenses","include_metadata":true}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        let results = metadata["results"].as_array().unwrap();
        assert!(
            results.len() >= 2,
            "expected at least two result rows from two chunks, got {}",
            results.len()
        );
        let matching: Vec<_> = results
            .iter()
            .filter(|r| r["source_file"] == "rust/cargo-deny.md")
            .collect();
        assert_eq!(
            matching.len(),
            results.len(),
            "all rows should share source_file"
        );
        // Ranks should be monotonically increasing 1-indexed integers.
        for (i, row) in results.iter().enumerate() {
            assert_eq!(
                row["rank"],
                i + 1,
                "row {i} should have rank {} (1-indexed), got {}",
                i + 1,
                row["rank"]
            );
        }
    }

    /// Embedder failure (Ollama unreachable) flips `mode` to `"fts_fallback"`.
    /// The prose body still contains the existing "Note: Ollama unreachable"
    /// prefix so prose-consuming clients keep working, but the metadata is
    /// the load-bearing signal that lets the coverage-check skill detect the
    /// silent FTS-only fallback. `fts_fallback` is distinct from `fts_only`:
    /// the former means "the embedder was attempted and failed", the latter
    /// means "the embedder was never attempted because the deployment is
    /// configured for FTS-only" (see
    /// `search_patterns_response_metadata_fts_only_when_hybrid_disabled`).
    #[test]
    fn search_patterns_response_metadata_fts_fallback_on_embedder_failure() {
        let (db, embedder, config, _tmp) = failing_embedder_harness();

        // Insert a chunk under the failing-embedder harness's db. We can't
        // call `embedder.embed()` for the insert (it would fail), so insert
        // with no embedding — FTS-only path will still find it via lexical
        // match.
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Cargo Deny".into(),
            body: "Run cargo deny check before every release.".into(),
            tags: "rust".into(),
            source_file: "rust/cargo-deny.md".into(),
            heading_path: String::new(),
            is_universal: false,
        };
        db.insert_chunk(&chunk, None).unwrap();

        let resp = dispatch_value(
            &db,
            &embedder,
            &config,
            r#"{
                "jsonrpc":"2.0","id":13,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"cargo deny check","include_metadata":true}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["mode"], "fts_fallback");

        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Note: Ollama unreachable"),
            "prose body should still contain the Ollama-unreachable note, got: {text}"
        );
    }

    /// FTS-only configured search (`config.search.hybrid = false`) flips
    /// `mode` to `"fts_only"`, distinct from both `"hybrid"` (full
    /// embedder + FTS) and `"fts_fallback"` (embedder unreachable in a
    /// hybrid-configured deployment).
    ///
    /// This test pins the contract that `fts_only` is its own mode value,
    /// not aliased to `hybrid` or `fts_fallback`. The coverage-check skill
    /// reads `mode` to decide whether aggregate coverage metrics are
    /// meaningful — only `hybrid` is comparable across queries; both
    /// `fts_only` and `fts_fallback` use BM25 ranks that are incomparable
    /// to RRF scores, so the skill must refuse to compute a coverage ratio
    /// when it sees either value.
    ///
    /// Without this test, the previous bug — where `mode` was derived
    /// solely from `embed_failed` and silently reported `"hybrid"` for
    /// `config.search.hybrid = false` deployments — would not be caught
    /// by the existing test suite.
    #[test]
    fn search_patterns_response_metadata_fts_only_when_hybrid_disabled() {
        let tmp = tempdir().unwrap();
        let embedder = FakeEmbedder::with_dimensions(4);
        let db = KnowledgeDB::open(Path::new(":memory:"), embedder.dimensions()).unwrap();
        db.init().unwrap();
        let mut config = Config::default_with(
            tmp.path().to_path_buf(),
            tmp.path().join("lore-test.db"),
            "test-model",
        );
        config.search.hybrid = false;

        // Insert without an embedding — FTS-only path does not consult the
        // vector index, so the chunk only needs to be in the FTS5 table.
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Cargo Deny".into(),
            body: "Run cargo deny check before every release.".into(),
            tags: "rust".into(),
            source_file: "rust/cargo-deny.md".into(),
            heading_path: String::new(),
            is_universal: false,
        };
        db.insert_chunk(&chunk, None).unwrap();

        let resp = dispatch_value(
            &db,
            &embedder,
            &config,
            r#"{
                "jsonrpc":"2.0","id":17,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"cargo deny check","include_metadata":true}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(
            metadata["mode"], "fts_only",
            "FTS-only configured search must report mode='fts_only' (distinct from \
             'hybrid' and 'fts_fallback'); the previous derivation from embed_failed \
             alone silently reported 'hybrid' here"
        );
        // Sanity check: the FTS-only path actually returned the chunk via
        // lexical match.
        assert!(
            metadata["result_count"].as_u64().unwrap() >= 1,
            "expected the FTS-only path to find the chunk via lexical match, got: {metadata}"
        );
        let results = metadata["results"].as_array().unwrap();
        assert_eq!(results[0]["source_file"], "rust/cargo-deny.md");
    }

    /// `top_k` parameter is reflected in the metadata block, and the results
    /// array honours the cap.
    #[test]
    fn search_patterns_response_metadata_top_k_respected() {
        let h = TestHarness::new();

        for i in 0..6 {
            let chunk = crate::chunking::Chunk {
                id: format!("c{i}"),
                title: format!("Pattern {i}"),
                body: format!("cargo deny content number {i}"),
                tags: "rust".into(),
                source_file: format!("rust/pattern-{i}.md"),
                heading_path: String::new(),
                is_universal: false,
            };
            let emb = h.embedder.embed(&chunk.body).unwrap();
            h.db.insert_chunk(&chunk, Some(&emb)).unwrap();
        }

        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":14,"method":"tools/call",
                "params":{
                    "name":"search_patterns",
                    "arguments":{"query":"cargo deny content","top_k":3,"include_metadata":true}
                }
            }"#,
        );

        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["top_k"], 3);
        let results = metadata["results"].as_array().unwrap();
        assert!(
            results.len() <= 3,
            "results array length must respect top_k=3, got {}",
            results.len()
        );
    }

    /// Pin the JSON-RPC error response shape for `search_patterns` so the
    /// `coverage-check` skill's step 8 (`errored` per-query state) is grounded
    /// in a tested contract. When `handle_search` rejects a request (e.g.
    /// `top_k` over `MAX_TOP_K`), the response must have `error` non-null and
    /// **must not** carry a `content` array with a fenced metadata block.
    /// The skill's step 8 reads `resp["error"].is_null()` first before
    /// touching `content[0].text`, so the absence of a success-path response
    /// on the error path is the contract that prevents the skill from
    /// attempting to extract fields from a missing object.
    ///
    /// This test passes `include_metadata: true` to verify that even when
    /// the caller opts in to the fenced-metadata channel, error responses
    /// still short-circuit past the append step and return via
    /// `error_response` with no `result` field populated.
    #[test]
    fn search_patterns_response_metadata_absent_on_error() {
        // Arrange
        let h = TestHarness::new();
        let oversized = MAX_TOP_K + 1;
        let body = format!(
            r#"{{
                "jsonrpc":"2.0","id":15,"method":"tools/call",
                "params":{{
                    "name":"search_patterns",
                    "arguments":{{"query":"anything","top_k":{oversized},"include_metadata":true}}
                }}
            }}"#
        );

        // Act
        let resp = h.request_value(&body);

        // Assert
        assert!(
            !resp["error"].is_null(),
            "error response should populate the JSON-RPC error field, got: {resp}"
        );
        assert!(
            resp["result"].is_null(),
            "error response must not carry a `result` field (skill consumers \
             read resp[\"error\"] first), got result: {}",
            resp["result"]
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

    // -- lore_status -------------------------------------------------------

    #[test]
    fn lore_status_reports_non_git_state() {
        // Arrange
        // TestHarness uses a plain tempdir for knowledge_dir, so the status
        // tool should report git_repository = false and delta_ingest_available
        // = false.
        let h = TestHarness::new();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":50,"method":"tools/call",
                "params":{"name":"lore_status","arguments":{"include_metadata":true}}
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["git_repository"], false);
        assert_eq!(metadata["delta_ingest_available"], false);
        assert_eq!(metadata["inbox_workflow_configured"], false);
        // No patterns ingested in this harness — chunks/sources should be 0.
        assert_eq!(metadata["chunks_indexed"], 0);
        assert_eq!(metadata["sources_indexed"], 0);
        assert!(metadata["last_ingested_commit"].is_null());
        // No .loreignore in the bare harness directory.
        assert_eq!(metadata["loreignore_active"], false);

        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Git repository: no"),
            "summary should reflect non-git state, got: {text}"
        );
        assert!(
            text.contains("unavailable"),
            "summary should mention delta ingest unavailability, got: {text}"
        );
        assert!(
            text.contains(".loreignore: absent"),
            "summary should mention .loreignore absence, got: {text}"
        );
    }

    /// When the caller does NOT pass `include_metadata: true`, `lore_status`
    /// must return the prose summary with no `lore-metadata` fence. Pins
    /// the opt-in contract: default callers pay no token cost for the
    /// embedded JSON block.
    #[test]
    fn lore_status_omits_metadata_fence_by_default() {
        // Arrange
        let h = TestHarness::new();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":55,"method":"tools/call",
                "params":{"name":"lore_status","arguments":{}}
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains("```lore-metadata"),
            "default lore_status response must not contain a lore-metadata fence, got: {text}"
        );
        assert!(extract_lore_metadata_fence(text).is_none());
        // Prose summary is still present — the opt-out path does not
        // suppress the human-readable response.
        assert!(
            text.contains("Knowledge base:"),
            "prose summary should still be present, got: {text}"
        );
    }

    #[test]
    fn lore_status_reports_loreignore_active_when_present() {
        // Arrange
        // Drop a .loreignore into the harness's knowledge_dir and verify the
        // status tool reflects it.
        let h = TestHarness::new();
        std::fs::write(h.config.knowledge_dir.join(".loreignore"), "README.md\n").unwrap();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":52,"method":"tools/call",
                "params":{"name":"lore_status","arguments":{"include_metadata":true}}
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["loreignore_active"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains(".loreignore: active"),
            "summary should mention .loreignore activation, got: {text}"
        );
    }

    #[test]
    fn lore_status_reports_loreignore_inactive_when_only_comments() {
        // Arrange
        // A comment-only .loreignore produces zero effective patterns and is
        // not considered active. Pin this so the field reflects "would this
        // file actually filter anything" rather than "does the file exist".
        let h = TestHarness::new();
        std::fs::write(
            h.config.knowledge_dir.join(".loreignore"),
            "# nothing excluded\n",
        )
        .unwrap();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":53,"method":"tools/call",
                "params":{"name":"lore_status","arguments":{"include_metadata":true}}
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["loreignore_active"], false);
    }

    #[test]
    fn lore_status_reports_git_state() {
        // Arrange
        // Initialise the harness's knowledge_dir as a git repository so the
        // status tool reports git_repository = true. Without an ingested
        // commit, delta_ingest_available remains false.
        let h = TestHarness::new();
        std::process::Command::new("git")
            .arg("init")
            .arg("--quiet")
            .current_dir(&h.config.knowledge_dir)
            .status()
            .unwrap();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":51,"method":"tools/call",
                "params":{"name":"lore_status","arguments":{"include_metadata":true}}
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["git_repository"], true);
        // No ingest has been recorded, so delta is still unavailable.
        assert_eq!(metadata["delta_ingest_available"], false);
        assert!(metadata["last_ingested_commit"].is_null());

        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Git repository: yes"),
            "summary should reflect git state, got: {text}"
        );
    }

    #[test]
    fn lore_status_reports_delta_ingest_available_when_git_and_commit_present() {
        // Arrange
        // The only path that flips `delta_ingest_available` to true is
        // `is_git_repo && last_commit.is_some()`. The previous two lore_status
        // tests cover the false branches; this test pins the true branch by
        // initialising a git repo AND seeding the META_LAST_COMMIT metadata
        // directly. Setting metadata directly avoids the cost of running a
        // full ingest, which would also require seeding markdown files.
        let h = TestHarness::new();
        std::process::Command::new("git")
            .arg("init")
            .arg("--quiet")
            .current_dir(&h.config.knowledge_dir)
            .status()
            .unwrap();
        h.db.set_metadata(crate::ingest::META_LAST_COMMIT, "abc1234deadbeef")
            .unwrap();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":54,"method":"tools/call",
                "params":{"name":"lore_status","arguments":{"include_metadata":true}}
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(metadata["git_repository"], true);
        assert_eq!(metadata["delta_ingest_available"], true);
        assert_eq!(metadata["last_ingested_commit"], "abc1234deadbeef");

        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Delta ingest: available"),
            "summary should reflect delta-available state, got: {text}"
        );
    }

    #[test]
    fn add_pattern_response_metadata_pins_commit_status_for_non_git_dir() {
        // Arrange
        // The TestHarness uses a plain tempdir for knowledge_dir (no `git init`),
        // so add_pattern should write the file and report commit_status = "not_committed"
        // in the fenced metadata block. Agents reading the fence can detect the
        // degraded state without parsing the human-readable prose summary.
        let h = TestHarness::new();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":40,"method":"tools/call",
                "params":{
                    "name":"add_pattern",
                    "arguments":{
                        "title":"Metadata Pinned Pattern",
                        "body":"Body text that is long enough for chunking.",
                        "include_metadata":true
                    }
                }
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let metadata = metadata_from_response(&resp);
        assert_eq!(
            metadata["commit_status"]["kind"], "not_committed",
            "non-git directory should report not_committed, got: {metadata:?}"
        );
        assert!(
            metadata["chunks_indexed"].as_u64().unwrap() >= 1,
            "metadata should report chunks_indexed >= 1"
        );
        assert!(
            metadata["file_path"].is_string(),
            "metadata should expose file_path"
        );
    }

    /// When the caller does NOT pass `include_metadata: true`, the write
    /// tool responses must not contain a `lore-metadata` fence. Pins the
    /// opt-in contract for the write tools (`add_pattern` in particular).
    #[test]
    fn add_pattern_omits_metadata_fence_by_default() {
        // Arrange
        let h = TestHarness::new();

        // Act
        let resp = h.request_value(
            r#"{
                "jsonrpc":"2.0","id":41,"method":"tools/call",
                "params":{
                    "name":"add_pattern",
                    "arguments":{
                        "title":"Plain Pattern",
                        "body":"Body text for default path."
                    }
                }
            }"#,
        );

        // Assert
        assert!(resp["error"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains("```lore-metadata"),
            "default add_pattern response must not contain a lore-metadata fence, got: {text}"
        );
        assert!(extract_lore_metadata_fence(text).is_none());
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
            tmp.path().join("lore-test.db"),
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
            is_universal: false,
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
        assert_eq!(tools.len(), 5);

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
        // Database is in-memory but the write-lock file lands beside the
        // configured database path. Point it inside the tempdir so the lock
        // is cleaned up automatically when the test exits.
        let mut config =
            Config::default_with(dir.to_path_buf(), dir.join("lore-test.db"), "test-model");
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
            tmp.path().join("lore-test.db"),
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
            is_universal: false,
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
            is_universal: false,
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
            is_universal: false,
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
            is_universal: false,
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
            tmp.path().join("lore-test.db"),
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
            is_universal: false,
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
            tmp.path().join("lore-test.db"),
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
            is_universal: false,
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
            is_universal: false,
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
