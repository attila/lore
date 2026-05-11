//! End-to-end CLI integration tests for edge-case behaviours.
//!
//! Currently covers the effective-empty-knowledge-dir warning surfaced via
//! `lore ingest` and `lore status` (units U1, U3.b in
//! `docs/plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md`).
//! Designed as a shared home for related edge-case CLI tests.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

use lore::config::Config;

/// Set up an empty knowledge directory with a saved config pointing at it.
///
/// Returns the config file path so callers can pass `--config <path>` to CLI
/// commands. The knowledge directory is created but contains no markdown
/// files; the database file path is configured but not initialised — the
/// CLI commands under test will create or open it as needed.
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
fn ingest_empty_directory_warns_via_cli() {
    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());

    // Act
    let assert = Command::cargo_bin("lore")
        .unwrap()
        .args(["ingest", "--config", config_path.to_str().unwrap()])
        .assert();

    // Assert: tier-2 contract — exit 0, warning on stderr.
    assert
        .success()
        .stderr(predicate::str::contains("knowledge directory is empty"));
}

#[test]
fn status_reports_empty_knowledge_dir_via_cli() {
    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());

    // Act
    let assert = Command::cargo_bin("lore")
        .unwrap()
        .args(["status", "--config", config_path.to_str().unwrap()])
        .assert();

    // Assert: the new Scan set line reports empty.
    assert
        .success()
        .stderr(predicate::str::contains("Scan set:").and(predicate::str::contains("empty")));
}

#[test]
fn serve_startup_warns_on_empty_dir_via_cli() {
    // Spawn `lore serve` with closed stdin so the read-loop exits on EOF
    // immediately after boot. The startup warning fires before the loop is
    // entered, so it must appear on stderr regardless.
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());

    let assert = Command::cargo_bin("lore")
        .unwrap()
        .args(["serve", "--config", config_path.to_str().unwrap()])
        .write_stdin("")
        .timeout(std::time::Duration::from_secs(10))
        .assert();

    assert
        .success()
        .stderr(predicate::str::contains("knowledge directory is empty"));
}

#[test]
fn list_does_not_warn_on_empty_knowledge_dir_via_cli() {
    // Negative control: read-only commands that do not run ingest must not
    // fire the empty-dir warning. A regression that broadens the check to
    // every CLI entry point would be caught here. `lore list` is the
    // simplest read-only path that touches the database without ingesting.
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());

    let assert = Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--config", config_path.to_str().unwrap()])
        .assert();

    assert
        .success()
        .stderr(predicate::str::contains("knowledge directory is empty").not());
}

#[test]
fn ingest_without_git_binary_falls_back_to_full_mode() {
    // R11.4 regression test for Slice E of the edge-case-handling brainstorm.
    //
    // `is_git_repo` returns false on both "this directory isn't a git repo"
    // and "the git binary isn't reachable via PATH" because the underlying
    // `Command::new("git").output()` returns `Err` in the latter case. The
    // non-repo branch is exercised by `src/ingest.rs::tests::add_pattern_*`
    // (non-`git_init`-ed tempdir → `CommitStatus::NotCommitted`, R11.1). The
    // missing-binary branch had no regression test before this slice — a
    // refactor that swaps `Command::new("git")` for `git2`/`gix`/etc. could
    // silently regress binary-less environments without CI catching it.
    //
    // The brainstorm's recipe is `.env_clear().env("PATH", "")` on the child
    // only. Mutating the parent test process's `PATH` via `std::env::set_var`
    // would race with every concurrent test that shells out to `git` (and
    // many do, via `git_init` helpers). `assert_cmd` sets the child's env in
    // isolation, so the parent is unaffected.
    //
    // The load-bearing assertion is the unique fallback marker
    // `"Not a git repository — running full ingest"` from
    // `src/ingest.rs::ingest`. Other full-mode fallbacks emit different copy
    // (`"No previous ingest recorded — running full ingest"`, etc.), so this
    // string uniquely pins the missing-git path. Embedding via Ollama may
    // fail in CI without a running embedder, but `cmd_ingest` only bails on
    // single-file mode — full-mode ingest collects per-file failures into
    // `result.errors` and still exits 0, so the assertion holds regardless
    // of embedder availability.

    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    std::fs::write(
        knowledge_dir.join("rust.md"),
        "# Rust Patterns\n\nA pattern body long enough to chunk during full ingest.\n",
    )
    .unwrap();
    let db_path = tmp.path().join("knowledge.db");
    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    // Act: spawn `lore ingest` with PATH cleared on the child only.
    // `assert_cmd::Command::cargo_bin` resolves the binary path from
    // `CARGO_BIN_EXE_lore` before `.env_clear()` runs, so the child still
    // finds the executable.
    let assert = Command::cargo_bin("lore")
        .unwrap()
        .args(["ingest", "--config", config_path.to_str().unwrap()])
        .env_clear()
        .env("PATH", "")
        .assert();

    // Assert: exit 0, missing-git fallback marker fires on stderr.
    assert
        .success()
        .stderr(predicate::str::contains(
            "Not a git repository — running full ingest",
        ))
        .stderr(predicate::str::contains("Found 1 markdown files"));
}
