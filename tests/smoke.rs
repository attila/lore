use assert_cmd::Command;
use predicates::prelude::*;

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
