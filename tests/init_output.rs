//! Integration tests for `lore init` output.
//!
//! These tests require a running Ollama instance and are skipped by default.
//! Run with: `cargo test -- --ignored`

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

/// Seed a minimal markdown file so ingestion has something to process.
fn seed_knowledge(dir: &Path) {
    fs::write(
        dir.join("example.md"),
        "# Example\n\nA test pattern for integration testing.\n",
    )
    .unwrap();
}

/// Initialise a git repo in `dir` with a test user identity.
fn git_init(dir: &Path) {
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        StdCommand::new("git")
            .args(&args)
            .current_dir(dir)
            .output()
            .expect("git command failed");
    }
}

#[test]
#[ignore = "requires running Ollama instance"]
fn init_default_paths_omits_config_from_output() {
    let xdg_config = tempdir().unwrap();
    let xdg_data = tempdir().unwrap();
    let repo = tempdir().unwrap();

    seed_knowledge(repo.path());
    git_init(repo.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args(["init", "--repo"])
        .arg(repo.path())
        .env("XDG_CONFIG_HOME", xdg_config.path())
        .env("XDG_DATA_HOME", xdg_data.path())
        .env("HOME", xdg_config.path()) // fallback, won't be used
        .assert()
        .success()
        .stderr(
            predicate::str::contains("\"args\": [\"serve\"]")
                .and(predicate::str::contains(
                    "claude mcp add --scope user --transport stdio lore -- lore serve",
                ))
                .and(predicate::str::contains("--config").not()),
        );

    // Config should be at XDG_CONFIG_HOME/lore/lore.toml
    let config_path = xdg_config.path().join("lore").join("lore.toml");
    assert!(config_path.exists(), "config not at expected XDG path");

    // Database should be at XDG_DATA_HOME/lore/knowledge.db
    let db_path = xdg_data.path().join("lore").join("knowledge.db");
    assert!(db_path.exists(), "database not at expected XDG path");

    // Config should contain absolute database path
    let config_contents = fs::read_to_string(&config_path).unwrap();
    let db_str = db_path.to_str().unwrap();
    assert!(
        config_contents.contains(db_str),
        "config should contain absolute database path {db_str}, got:\n{config_contents}"
    );
}

#[test]
#[ignore = "requires running Ollama instance"]
fn init_custom_config_includes_config_in_output() {
    let tmp = tempdir().unwrap();
    let custom_config = tmp.path().join("custom").join("lore.toml");
    let repo = tempdir().unwrap();
    let xdg_data = tempdir().unwrap();

    seed_knowledge(repo.path());
    git_init(repo.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args(["--config"])
        .arg(&custom_config)
        .args(["init", "--repo"])
        .arg(repo.path())
        .env("XDG_DATA_HOME", xdg_data.path())
        .env("HOME", tmp.path())
        .assert()
        .success()
        .stderr(
            predicate::str::contains("\"--config\"")
                .and(predicate::str::contains("lore serve --config")),
        );

    assert!(custom_config.exists(), "config not at custom path");
}

#[test]
#[ignore = "requires running Ollama instance"]
fn init_custom_database_does_not_appear_in_output() {
    let xdg_config = tempdir().unwrap();
    let tmp = tempdir().unwrap();
    let custom_db = tmp.path().join("custom.db");
    let repo = tempdir().unwrap();

    seed_knowledge(repo.path());
    git_init(repo.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args(["init", "--repo"])
        .arg(repo.path())
        .args(["--database"])
        .arg(&custom_db)
        .env("XDG_CONFIG_HOME", xdg_config.path())
        .env("HOME", xdg_config.path())
        .assert()
        .success()
        .stderr(
            // Output should show simple serve args (no --config, no --database)
            predicate::str::contains("\"args\": [\"serve\"]")
                .and(predicate::str::contains("--database").not()),
        );

    assert!(custom_db.exists(), "database not at custom path");

    // Config should reference the custom database path
    let config_path = xdg_config.path().join("lore").join("lore.toml");
    let config_contents = fs::read_to_string(&config_path).unwrap();
    let db_str = custom_db.to_str().unwrap();
    assert!(
        config_contents.contains(db_str),
        "config should contain custom database path {db_str}"
    );
}

#[test]
#[ignore = "requires running Ollama instance"]
fn init_output_paths_are_clean() {
    let xdg_config = tempdir().unwrap();
    let xdg_data = tempdir().unwrap();
    let repo = tempdir().unwrap();

    // Create a nested dir so we can use ".." in the config path
    let nested = repo.path().join("subdir");
    fs::create_dir(&nested).unwrap();

    seed_knowledge(repo.path());
    git_init(repo.path());

    let relative_config = nested.join("..").join("lore.toml");

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["--config"])
        .arg(&relative_config)
        .args(["init", "--repo"])
        .arg(repo.path())
        .env("XDG_DATA_HOME", xdg_data.path())
        .env("HOME", xdg_config.path())
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    // The output should NOT contain ".." in the config path
    assert!(
        !stderr.contains("/.."),
        "output should not contain '..' path hops, got:\n{stderr}"
    );
}
