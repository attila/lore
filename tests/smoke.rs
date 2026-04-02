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
    out.stderr(predicate::str::contains("[2]"));

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
    out.stderr(predicate::str::contains("[1]"))
        .stderr(predicate::str::contains("[2]").not());
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
        .stderr(predicate::str::contains("Rust Testing"))
        .stderr(predicate::str::contains("Error Handling"));
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
        .stderr(predicate::str::is_empty());
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
