//! Integration tests that exercise real Ollama embeddings.
//!
//! These tests require a running Ollama instance with `nomic-embed-text` pulled.
//! They are skipped by default — run with: `just test-integration`

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use lore::config::Config;
use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, OllamaClient};
use lore::ingest;
use tempfile::tempdir;

const OLLAMA_HOST: &str = "http://127.0.0.1:11434";
const OLLAMA_MODEL: &str = "nomic-embed-text";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create an `OllamaClient` pointing at the local Ollama instance.
fn ollama_client() -> OllamaClient {
    OllamaClient::new(OLLAMA_HOST, OLLAMA_MODEL)
}

/// Initialise a git repo in `dir` with a test user identity.
fn git_init(dir: &Path) {
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
}

/// Open an on-disk `KnowledgeDB` in the given directory.
fn open_db(dir: &Path, dims: usize) -> KnowledgeDB {
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, dims).expect("failed to open DB");
    db.init().expect("failed to init DB");
    db
}

/// Seed the knowledge directory with patterns designed for semantic search testing.
///
/// Three files with distinctive domains:
///
/// - `error-handling.md` — explicit error-handling vocabulary.
/// - `naming-conventions.md` — coding style and identifier rules.
/// - `database-performance.md` — query optimisation vocabulary, deliberately avoiding
///   words like "fast", "slow", "speed", "lookup", "SQL" so that the R2 query
///   ("making SQL lookups faster") has zero FTS token overlap while remaining
///   semantically related.
fn seed_patterns(dir: &Path) {
    fs::write(
        dir.join("error-handling.md"),
        "# Error Handling\n\n\
         Always use anyhow for application-level error propagation.\n\
         Reserve thiserror for library crates that need typed error variants.\n\
         Never use unwrap in production paths; prefer context-rich bail macros.\n",
    )
    .unwrap();

    fs::write(
        dir.join("naming-conventions.md"),
        "# Naming Conventions\n\n\
         Use snake_case for all Rust function and variable names.\n\
         Prefer descriptive identifiers over abbreviations.\n\
         Module names should be singular nouns, not pluralised.\n",
    )
    .unwrap();

    fs::write(
        dir.join("database-performance.md"),
        "# Database Performance\n\n\
         Create indexes on columns used in WHERE and JOIN clauses.\n\
         Use EXPLAIN ANALYZE to identify sequential scans and missing indexes.\n\
         Prefer B-tree indexes for range queries and hash indexes for equality checks.\n\
         Denormalize read-heavy tables to reduce join overhead.\n\
         Batch inserts inside transactions to amortize WAL flush cost.\n",
    )
    .unwrap();
}

/// Return `true` if `source_file` appears in the first `n` results.
fn in_top_n(results: &[lore::database::SearchResult], source_file: &str, n: usize) -> bool {
    results.iter().take(n).any(|r| r.source_file == source_file)
}

/// Drop guard that kills a child process on panic or early return.
struct ChildGuard(Option<std::process::Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Send a JSON-RPC request to a child process and read one response line.
fn send_and_recv(
    stdin: &mut dyn Write,
    reader: &mut BufReader<std::process::ChildStdout>,
    request: &str,
) -> serde_json::Value {
    writeln!(stdin, "{request}").expect("failed to write to stdin");
    stdin.flush().expect("failed to flush stdin");
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("failed to read response");
    serde_json::from_str(&line).expect("failed to parse JSON response")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full lifecycle: ingest → vector search → hybrid search (R2 proof) →
/// `add_pattern` → `update_pattern` → `append_to_pattern`.  (R1-R5)
#[test]
#[ignore = "requires running Ollama instance"]
#[allow(clippy::too_many_lines)]
fn ollama_lifecycle() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // -- Setup ----------------------------------------------------------------
    seed_patterns(dir);
    git_init(dir);

    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial seed"])
        .current_dir(dir)
        .output()
        .unwrap();

    let embedder = ollama_client();
    let db = open_db(dir, embedder.dimensions());

    // -- Ingest ---------------------------------------------------------------
    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});

    assert_eq!(result.files_processed, 3);
    assert!(
        result.chunks_created >= 3,
        "expected >=3 chunks, got {}",
        result.chunks_created
    );
    assert!(
        result.errors.is_empty(),
        "unexpected ingest errors: {:?}",
        result.errors
    );

    // -- R1: Vector search finds semantically related content -----------------
    let query = "dealing with runtime failures in Rust";
    let query_emb = embedder.embed(query).unwrap();
    let results = db.search_vector(&query_emb, 5).unwrap();

    assert!(
        in_top_n(&results, "error-handling.md", 3),
        "R1: vector search for '{query}' should find error-handling.md in top 3, got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );

    // -- R2: Hybrid search with FTS-negative proof ----------------------------
    // Query semantically related to database-performance.md but sharing no FTS tokens.
    let r2_query = "making SQL lookups faster";

    // Phase 1: FTS must return nothing for this query.
    let fts_results = db.search_fts(r2_query, 10).unwrap();
    assert!(
        fts_results.is_empty()
            || fts_results
                .iter()
                .all(|r| r.source_file != "database-performance.md"),
        "R2 precondition: FTS should not find database-performance.md for '{r2_query}', \
         got: {:?}",
        fts_results
            .iter()
            .map(|r| &r.source_file)
            .collect::<Vec<_>>()
    );

    // Phase 2: Hybrid search (with real embedding) must find it.
    let r2_emb = embedder.embed(r2_query).unwrap();
    let hybrid_results = db.search_hybrid(r2_query, Some(&r2_emb), 5).unwrap();
    assert!(
        in_top_n(&hybrid_results, "database-performance.md", 3),
        "R2: hybrid search for '{r2_query}' should find database-performance.md in top 3, \
         got: {:?}",
        hybrid_results
            .iter()
            .map(|r| &r.source_file)
            .collect::<Vec<_>>()
    );

    // -- R3: add_pattern is searchable ----------------------------------------
    let write_result = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Logging Guidelines",
        "Use structured logging with the tracing crate.\n\
         Always include span context for distributed tracing.\n\
         Log at warn level for recoverable errors, error level for unrecoverable.\n",
        &["observability", "rust"],
        None,
    )
    .unwrap();

    assert!(write_result.chunks_indexed >= 1);

    let query = "structured observability and tracing";
    let query_emb = embedder.embed(query).unwrap();
    let results = db.search_vector(&query_emb, 5).unwrap();
    assert!(
        in_top_n(&results, "logging-guidelines.md", 3),
        "R3: vector search should find newly added logging-guidelines.md in top 3, got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );

    // -- R4: update_pattern replaces content ----------------------------------
    let write_result = ingest::update_pattern(
        &db,
        &embedder,
        dir,
        "logging-guidelines.md",
        "Use OpenTelemetry for all telemetry collection.\n\
         Export metrics via Prometheus endpoints.\n\
         Never use println for diagnostic output in production.\n",
        Some(&["observability", "production"]),
        None,
    )
    .unwrap();

    assert!(write_result.chunks_indexed >= 1);

    // New content should be findable.
    let query = "OpenTelemetry and Prometheus metrics";
    let query_emb = embedder.embed(query).unwrap();
    let results = db.search_vector(&query_emb, 5).unwrap();
    assert!(
        in_top_n(&results, "logging-guidelines.md", 3),
        "R4: updated content should be findable, got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );

    // Old content ("distributed tracing") should not appear in this file's results.
    let old_results = db.search_fts("distributed tracing", 10).unwrap();
    assert!(
        old_results
            .iter()
            .all(|r| r.source_file != "logging-guidelines.md"),
        "R4: old content 'distributed tracing' should not appear in logging-guidelines.md"
    );

    // -- R5: append_to_pattern is searchable ----------------------------------
    let write_result = ingest::append_to_pattern(
        &db,
        &embedder,
        dir,
        "logging-guidelines.md",
        "Alerting",
        "Configure PagerDuty integration for critical alerts.\n\
         Use escalation policies with tiered response windows.\n",
        None,
    )
    .unwrap();

    assert!(write_result.chunks_indexed >= 1);

    let query = "PagerDuty alerting escalation";
    let query_emb = embedder.embed(query).unwrap();
    let results = db.search_vector(&query_emb, 5).unwrap();
    assert!(
        in_top_n(&results, "logging-guidelines.md", 3),
        "R5: appended content should be findable, got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );
}

/// Re-ingest after file deletion removes stale embeddings. (R6)
#[test]
#[ignore = "requires running Ollama instance"]
fn ollama_stale_embeddings_removed() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    let embedder = ollama_client();
    let db = open_db(dir, embedder.dimensions());

    // Seed two files with distinct content.
    fs::write(
        dir.join("alpha.md"),
        "# Alpha\n\nAlpha uses a waterfall deployment model for releases.\n",
    )
    .unwrap();
    fs::write(
        dir.join("beta.md"),
        "# Beta\n\nBeta requires canary deployments with gradual rollout.\n",
    )
    .unwrap();

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(result.files_processed, 2);
    assert!(result.errors.is_empty());

    // Both should be findable via vector search.
    let emb = embedder.embed("waterfall release model").unwrap();
    let results = db.search_vector(&emb, 5).unwrap();
    assert!(
        results.iter().any(|r| r.source_file == "alpha.md"),
        "alpha.md should be findable before deletion"
    );

    // Delete alpha, re-ingest.
    fs::remove_file(dir.join("alpha.md")).unwrap();
    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(result.files_processed, 1);

    // alpha's content should be gone from vector search.
    let emb = embedder.embed("waterfall release model").unwrap();
    let results = db.search_vector(&emb, 5).unwrap();
    assert!(
        results.iter().all(|r| r.source_file != "alpha.md"),
        "R6: deleted file's vector data should be gone, got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );

    // beta should still be findable.
    let emb = embedder.embed("canary deployment rollout").unwrap();
    let results = db.search_vector(&emb, 5).unwrap();
    assert!(
        results.iter().any(|r| r.source_file == "beta.md"),
        "beta.md should still be findable after re-ingest"
    );
}

/// MCP subprocess round-trip with real Ollama backend. (R7)
#[test]
#[ignore = "requires running Ollama instance"]
fn ollama_mcp_subprocess_roundtrip() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // -- Setup: seed, git init, ingest into a real DB -------------------------
    seed_patterns(dir);
    git_init(dir);

    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial seed"])
        .current_dir(dir)
        .output()
        .unwrap();

    let embedder = ollama_client();
    let db_path = dir.join("knowledge.db");
    {
        let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
        db.init().unwrap();
        let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(
            result.errors.is_empty(),
            "ingest errors: {:?}",
            result.errors
        );
        // db is dropped here — release the handle before spawning subprocess
    }

    // -- Write config ---------------------------------------------------------
    let config_path = dir.join("lore.toml");
    let config = Config::default_with(dir.to_path_buf(), db_path, OLLAMA_MODEL);
    config.save(&config_path).unwrap();

    // -- Spawn lore serve -----------------------------------------------------
    let bin = assert_cmd::cargo::cargo_bin("lore");
    let mut child = Command::new(bin)
        .args(["serve", "--config"])
        .arg(&config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn lore serve");

    // Guard ensures the child process is killed even if an assertion panics.
    let mut guard = ChildGuard(None);

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    let stdout = child.stdout.take().expect("failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Move child into the guard now that we've taken stdin/stdout.
    guard.0 = Some(child);

    // -- initialize -----------------------------------------------------------
    let resp = send_and_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    );
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["serverInfo"]["name"].is_string());

    // Note: notifications/initialized returns no response — do not read after sending.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    // -- tools/call search_patterns -------------------------------------------
    let resp = send_and_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_patterns","arguments":{"query":"error handling"}}}"#,
    );
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 2);

    let content_text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("R7: response should contain content text");
    assert!(
        !content_text.is_empty(),
        "R7: search result text should not be empty"
    );
    assert!(
        content_text.contains("error-handling.md") || content_text.contains("Error Handling"),
        "R7: search results should reference error handling content, got: {content_text}"
    );

    // guard drops here — kills the child process
}

/// Single-file ingest happy path with real embeddings.
///
/// Validates that `ingest::ingest_single_file` works end-to-end with the
/// production `OllamaClient` (not `FakeEmbedder`): the file is upserted into
/// the index, the chunks carry real (non-null) embeddings, vector search
/// finds the file, and the walk-based `META_LAST_COMMIT` metadata is left
/// untouched. This is the only test in the suite that exercises single-file
/// ingest against real embeddings — every other single-file test uses
/// `FakeEmbedder`. Without this, a regression in the embedding path
/// specific to the single-file flow (timeouts, dimension mismatches, model
/// swaps) would not be caught until production.
#[test]
#[ignore = "requires running Ollama instance"]
fn ingest_single_file_happy_path_with_real_embeddings() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // Note: deliberately NOT a git repo, so we also prove single-file
    // ingest does not consult git state and works in non-git knowledge
    // directories.
    fs::write(
        dir.join("retry-policies.md"),
        "# Retry Policies\n\n\
         Use exponential backoff with jitter for transient remote failures.\n\
         Cap the maximum delay to bound user-visible latency.\n\
         Distinguish retryable errors from permanent ones at the call site.\n",
    )
    .unwrap();

    let embedder = ollama_client();
    let db = open_db(dir, embedder.dimensions());

    let result = ingest::ingest_single_file(
        &db,
        &embedder,
        dir,
        &dir.join("retry-policies.md"),
        "heading",
        false,
        &|_| {},
    );

    assert!(
        result.errors.is_empty(),
        "single-file ingest with real Ollama errored: {:?}",
        result.errors
    );
    assert_eq!(result.files_processed, 1);
    assert!(
        result.chunks_created >= 1,
        "expected >=1 chunk, got {}",
        result.chunks_created
    );
    assert!(matches!(
        result.mode,
        ingest::IngestMode::SingleFile { ref path } if path == "retry-policies.md"
    ));

    // Vector search must find the file using a semantically related query
    // that does not share FTS tokens with the body. This proves the chunks
    // carry real embeddings, not null ones.
    let query = "handling temporary network glitches when calling APIs";
    let query_emb = embedder.embed(query).unwrap();
    let results = db.search_vector(&query_emb, 5).unwrap();
    assert!(
        in_top_n(&results, "retry-policies.md", 3),
        "vector search for '{query}' should find retry-policies.md in top 3, got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );

    // Critical orthogonality invariant: walk-based delta state is untouched.
    // Single-file ingest must never write META_LAST_COMMIT.
    assert_eq!(
        db.get_metadata("last_ingested_commit").unwrap(),
        None,
        "single-file ingest must not write last_ingested_commit"
    );
}

/// Single-file ingest happy path through the CLI binary.
///
/// Exercises the full dispatch chain that `tests/single_file_ingest.rs` and
/// `tests/smoke.rs` cannot reach: clap → `cmd_ingest` → `Config::load` →
/// `KnowledgeDB::open` → `WriteLock` acquire → `dispatch_ingest` →
/// `std::path::absolute` → `ingest_single_file` → real embed → `on_progress`
/// → `print_ingest_summary` success branch → exit 0. The library-level
/// Ollama test above proves the embedding path works; this test proves the
/// binary plumbing wires it together correctly.
#[test]
#[ignore = "requires running Ollama instance"]
fn ingest_file_happy_path_via_binary() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    fs::write(
        dir.join("draft.md"),
        "# Draft Pattern\n\n\
         Distinctive vocabulary for the cli binary single file ingest test.\n\
         The marker word is xyzzysinglefile so we can search for it later.\n",
    )
    .unwrap();

    // Write a config pointing the database at the same tempdir.
    let db_path = dir.join("knowledge.db");
    let config_path = dir.join("lore.toml");
    let config = Config::default_with(dir.to_path_buf(), db_path.clone(), OLLAMA_MODEL);
    config.save(&config_path).unwrap();

    // Pre-create the DB so the CLI does not have to do first-time setup.
    {
        let embedder = ollama_client();
        let db = open_db(dir, embedder.dimensions());
        // Drop closes the DB; the CLI will reopen it.
        drop(db);
    }

    let bin = assert_cmd::cargo::cargo_bin("lore");
    let output = Command::new(&bin)
        .args(["ingest", "--config"])
        .arg(&config_path)
        .arg("--file")
        .arg(dir.join("draft.md"))
        .output()
        .expect("failed to spawn lore ingest --file");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "lore ingest --file should exit 0, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    );

    // Stderr carries the success summary in the documented format.
    assert!(
        stderr.contains("Done (single-file): draft.md"),
        "stderr missing Done summary, got: {stderr}"
    );
    assert!(
        stderr.contains("chunks"),
        "stderr missing chunk count, got: {stderr}"
    );

    // Reopen the DB and confirm the file is searchable by its distinctive
    // marker. This proves the binary actually persisted real chunks, not
    // just printed a success message.
    let embedder = ollama_client();
    let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
    db.init().unwrap();
    let hits = db.search_fts("xyzzysinglefile", 10).unwrap();
    assert!(
        hits.iter().any(|h| h.source_file == "draft.md"),
        "draft.md should be searchable after single-file ingest via the binary, got: {:?}",
        hits.iter().map(|h| &h.source_file).collect::<Vec<_>>()
    );
}

/// Ollama unreachable: operations return errors, not panics. (R8)
#[test]
#[ignore = "requires running Ollama instance"]
fn ollama_unreachable_returns_error() {
    // Use a dead port — connection-refused returns instantly.
    let dead_client = OllamaClient::new("http://127.0.0.1:1", OLLAMA_MODEL);

    // -- Direct embed call returns Err ----------------------------------------
    let result = dead_client.embed("anything");
    assert!(
        result.is_err(),
        "R8: embed() with dead port should return Err, got Ok({:?})",
        result.unwrap()
    );

    // -- ingest() records errors but doesn't panic ----------------------------
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    fs::write(
        dir.join("test.md"),
        "# Test\n\nSome content for testing error handling.\n",
    )
    .unwrap();

    // Use the dead client's dimensions (768 for nomic-embed-text).
    let db = open_db(dir, dead_client.dimensions());
    let result = ingest::ingest(&db, &dead_client, dir, "heading", &|_| {});

    assert_eq!(result.files_processed, 1);
    assert!(
        !result.errors.is_empty(),
        "R8: ingest with dead Ollama should record errors, got none"
    );
    // Chunks are created with None embeddings — chunks_created will be > 0.
    assert!(
        result.chunks_created >= 1,
        "R8: chunks should still be created (with None embeddings)"
    );
}
