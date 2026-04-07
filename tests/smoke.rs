use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

use lore::config::Config;
use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, FakeEmbedder};
use lore::ingest;

/// Set up a temp directory with a config, database, and ingested test patterns.
///
/// Returns the config file path so callers can pass `--config <path>` to CLI
/// commands.
fn setup_populated_env(dir: &Path) -> std::path::PathBuf {
    let knowledge_dir = dir.join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    std::fs::write(
        knowledge_dir.join("rust-testing.md"),
        "# Rust Testing\n\n\
         tags: rust, testing\n\n\
         Prefer integration tests that exercise real dependencies over mocks.\n\
         Use deterministic fakes only for external services.\n",
    )
    .unwrap();

    std::fs::write(
        knowledge_dir.join("error-handling.md"),
        "# Error Handling\n\n\
         tags: rust, errors\n\n\
         Always use anyhow for application-level error propagation.\n",
    )
    .unwrap();

    let db_path = dir.join("knowledge.db");
    let config = Config::default_with(knowledge_dir.clone(), db_path, "nomic-embed-text");

    let config_path = dir.join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database, embedder.dimensions()).unwrap();
    db.init().unwrap();

    ingest::ingest(&db, &embedder, &knowledge_dir, "heading", &|_| {});

    config_path
}

#[test]
fn help_exits_successfully() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn help_shows_all_subcommands() {
    let output = Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success();

    output
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("ingest"))
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn version_shows_version() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn search_without_query_shows_error() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", "/tmp/nonexistent-lore.toml"])
        .assert()
        .failure();
}

#[test]
fn init_without_repo_shows_error() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--repo"));
}

#[test]
fn no_knowledge_mcp_in_help_output() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("knowledge-mcp").not());
}

#[test]
fn init_help_shows_database_flag() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--database"));
}

#[test]
fn init_against_plain_directory_emits_git_advisory() {
    // The advisory is printed by `cmd_init` before Ollama provisioning, so we
    // can capture it from stderr regardless of whether the rest of `lore init`
    // succeeds (it will fail if Ollama is not running locally — that is fine).
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let config_path = tmp.path().join("lore.toml");
    let db_path = tmp.path().join("knowledge.db");

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "init",
            "--repo",
            knowledge_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
            "--database",
            db_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("is not a git repository"),
        "expected git advisory on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("delta ingest"),
        "expected advisory to mention delta ingest, got: {stderr}"
    );
    assert!(
        stderr.contains("docs/configuration.md#git-integration"),
        "expected advisory to point at the documentation reference, got: {stderr}"
    );
}

#[test]
fn init_against_git_repo_does_not_emit_git_advisory() {
    // When the target is a git repository, the advisory must not appear.
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    // Initialise an empty git repository in the knowledge dir.
    std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(&knowledge_dir)
        .status()
        .unwrap();

    let config_path = tmp.path().join("lore.toml");
    let db_path = tmp.path().join("knowledge.db");

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "init",
            "--repo",
            knowledge_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
            "--database",
            db_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("is not a git repository"),
        "git advisory should not appear for a git repository, got: {stderr}"
    );
}

#[test]
fn status_without_config_shows_init_hint() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("lore")
        .unwrap()
        .arg("status")
        .env("XDG_CONFIG_HOME", tmp.path())
        .env("HOME", tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("lore init"));
}

// -- top-k ------------------------------------------------------------------

#[test]
fn search_top_k_limits_results() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    // Without --top-k, we should get both patterns.
    let out = Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", config_path.to_str().unwrap(), "rust"])
        .assert()
        .success();
    // Both patterns mention "rust" so at least 2 results expected.
    out.stdout(predicate::str::contains("[2]"));

    // With --top-k 1, only one result.
    let out = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "--top-k",
            "1",
            "rust",
        ])
        .assert()
        .success();
    out.stdout(predicate::str::contains("[1]"))
        .stdout(predicate::str::contains("[2]").not());
}

#[test]
fn search_top_k_zero_returns_no_results() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "--top-k",
            "0",
            "rust",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("No results found."));
}

// -- lore list --------------------------------------------------------------

#[test]
fn list_outputs_pattern_titles() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--config", config_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust Testing"))
        .stdout(predicate::str::contains("Error Handling"));
}

#[test]
fn list_empty_database_exits_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("empty.db");
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    // Ensure the database tables exist (init creates them).
    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database, embedder.dimensions()).unwrap();
    db.init().unwrap();
    drop(db);

    Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--config", config_path.to_str().unwrap()])
        .assert()
        .success()
        // No output (empty database).
        .stdout(predicate::str::is_empty());
}

// -- FTS5 sanitization ------------------------------------------------------

#[test]
fn search_with_dots_does_not_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "file.with.dots",
        ])
        .assert()
        .success();
}

#[test]
fn search_with_path_does_not_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "path/to/file.ts",
        ])
        .assert()
        .success();
}

// -- LORE_DEBUG -------------------------------------------------------------

#[test]
fn lore_debug_emits_to_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .env("LORE_DEBUG", "1")
        .args(["search", "--config", config_path.to_str().unwrap(), "rust"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[lore debug]"),
        "expected debug output on stderr, got: {stderr}"
    );
}

#[test]
fn lore_debug_off_no_debug_output() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .env_remove("LORE_DEBUG")
        .args(["search", "--config", config_path.to_str().unwrap(), "rust"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("[lore debug]"),
        "unexpected debug output on stderr: {stderr}"
    );
}

// -- --json flag ------------------------------------------------------------

#[test]
fn search_json_outputs_valid_array() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "rust",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();

    assert!(arr.len() >= 2, "expected at least 2 results");

    // Verify expected fields are present.
    let first = &arr[0];
    assert!(first.get("title").is_some());
    assert!(first.get("body").is_some());
    assert!(first.get("tags").is_some());
    assert!(first.get("source_file").is_some());
    assert!(first.get("score").is_some());
}

#[test]
fn search_json_includes_full_body() {
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    // Create a pattern with body longer than the 500-byte human truncation.
    let long_body = "x".repeat(800);
    std::fs::write(
        knowledge_dir.join("long-body.md"),
        format!("# Long Body\n\ntags: test\n\n{long_body}\n"),
    )
    .unwrap();

    let db_path = tmp.path().join("knowledge.db");
    let config = Config::default_with(knowledge_dir.clone(), db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database, embedder.dimensions()).unwrap();
    db.init().unwrap();
    ingest::ingest(&db, &embedder, &knowledge_dir, "heading", &|_| {});
    drop(db);

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "long body",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();
    assert!(!arr.is_empty(), "expected at least 1 result");

    // Body should not be truncated.
    let body = arr[0]["body"].as_str().unwrap();
    assert!(
        body.len() >= 800,
        "JSON body should not be truncated (got {} bytes)",
        body.len()
    );
}

#[test]
fn search_json_empty_results() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "xyznonexistent",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), "[]");
}

#[test]
fn search_json_respects_top_k() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "--top-k",
            "1",
            "rust",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1, "expected exactly 1 result with --top-k 1");
}

#[test]
fn list_json_outputs_valid_array() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--json", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();

    assert!(arr.len() >= 2, "expected at least 2 patterns");

    let first = &arr[0];
    assert!(first.get("title").is_some());
    assert!(first.get("source_file").is_some());
    assert!(first.get("tags").is_some());
}

// -- lore ingest --file ------------------------------------------------------
//
// These tests exercise the CLI binary end-to-end for error paths that do not
// require a running Ollama embedder (all fail before any embed call). The
// happy path is covered by tests/single_file_ingest.rs at the library level
// with FakeEmbedder.

fn setup_empty_knowledge(dir: &Path) -> std::path::PathBuf {
    let knowledge_dir = dir.join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    let db_path = dir.join("knowledge.db");
    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = dir.join("lore.toml");
    config.save(&config_path).unwrap();
    config_path
}

#[test]
fn ingest_file_rejects_unsupported_extension_with_exit_code_1() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());
    let txt = tmp.path().join("knowledge").join("notes.txt");
    std::fs::write(&txt, "not markdown").unwrap();

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            txt.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unsupported extension"))
        .stderr(predicate::str::contains("single-file ingest failed"));
}

#[test]
fn ingest_file_rejects_path_outside_knowledge_dir_with_exit_code_1() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());
    // File is a sibling of knowledge_dir, not inside it.
    let outside = tmp.path().join("outside.md");
    std::fs::write(
        &outside,
        "# Outside\n\nBody that is long enough for chunking.\n",
    )
    .unwrap();

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            outside.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("escapes the knowledge directory"));
}

#[test]
fn ingest_file_rejects_missing_file_with_cwd_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());
    let missing = tmp.path().join("knowledge").join("does-not-exist.md");

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            missing.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Cannot access"))
        .stderr(predicate::str::contains("cwd:"));
}

#[test]
fn ingest_help_shows_file_flag_and_exit_codes() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["ingest", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--file"))
        .stdout(predicate::str::contains("EXIT CODES"))
        .stdout(predicate::str::contains("EXAMPLES"));
}

#[test]
fn list_json_empty_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("empty.db");
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database, embedder.dimensions()).unwrap();
    db.init().unwrap();
    drop(db);

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--json", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), "[]");
}
