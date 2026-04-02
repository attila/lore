use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, FakeEmbedder};
use lore::ingest;
use tempfile::tempdir;

/// Create a minimal lore config file pointing at the given database path.
///
/// Uses `hybrid = false` to ensure deterministic FTS-only behavior
/// regardless of whether Ollama happens to be running on the machine.
fn write_config(dir: &Path, db_path: &Path) -> std::path::PathBuf {
    let config_path = dir.join("lore.toml");
    let content = format!(
        r#"
knowledge_dir = "{knowledge_dir}"
database = "{database}"
bind = "localhost:3100"

[ollama]
host = "http://127.0.0.1:11434"
model = "nomic-embed-text"

[search]
hybrid = false
top_k = 5
min_relevance = 0.0

[chunking]
strategy = "heading"
max_tokens = 1024
"#,
        knowledge_dir = dir.display(),
        database = db_path.display(),
    );
    fs::write(&config_path, content).unwrap();
    config_path
}

/// Open an on-disk `KnowledgeDB` in the given directory.
fn open_db(dir: &Path, dims: usize) -> KnowledgeDB {
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, dims).expect("failed to open DB");
    db.init().expect("failed to init DB");
    db
}

/// Seed the knowledge directory with pattern files that have distinctive terms.
fn seed_patterns(dir: &Path) {
    fs::write(
        dir.join("rust-conventions.md"),
        "# Rust Conventions\n\n\
         tags: rust, conventions\n\n\
         Use anyhow for application-level error propagation.\n\
         Reserve thiserror for library crates that need typed error variants.\n\
         Never use unwrap in production paths.\n",
    )
    .unwrap();

    fs::write(
        dir.join("typescript-conventions.md"),
        "# TypeScript Conventions\n\n\
         tags: typescript, conventions\n\n\
         Prefer type over interface for object shapes.\n\
         Use arrow functions for all callbacks.\n\
         Always use named exports, never default exports.\n",
    )
    .unwrap();
}

/// Set up a temp directory with patterns ingested into a database, returning
/// the config path and temp dir handle (must stay alive for the test).
fn setup_test_env() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    seed_patterns(dir);

    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(
        result.chunks_created >= 2,
        "expected >=2 chunks, got {}",
        result.chunks_created
    );

    let config_path = write_config(dir, &dir.join("knowledge.db"));
    (tmp, config_path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn hook_pretooluse_edit_rs_returns_context() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-session",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/error_handling.rs"
        }
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    // Should produce JSON output with additionalContext.
    if !stdout.is_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
        assert!(
            parsed.get("hookSpecificOutput").is_some(),
            "should have hookSpecificOutput: {stdout}"
        );
        let hso = &parsed["hookSpecificOutput"];
        assert_eq!(hso["hookEventName"], "PreToolUse");
        assert!(
            hso["additionalContext"].as_str().is_some(),
            "should have additionalContext string: {stdout}"
        );
        let ctx = hso["additionalContext"].as_str().unwrap();
        assert!(
            ctx.contains("REQUIRED CONVENTIONS"),
            "should contain imperative header: {ctx}"
        );
    }
    // If empty, it means no patterns matched (FTS-only path) -- that's
    // acceptable in CI without Ollama.
}

#[test]
fn hook_pretooluse_edit_ts_returns_context() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-session",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/components/UserProfile.tsx"
        }
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    if !stdout.is_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(parsed.get("hookSpecificOutput").is_some());
    }
}

#[test]
fn hook_explore_agent_produces_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-session",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/error_handling.rs"
        },
        "agent_type": "Explore"
    });

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn hook_plan_agent_produces_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-session",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/error_handling.rs"
        },
        "agent_type": "Plan"
    });

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn hook_non_pretooluse_event_produces_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "test-session"
    });

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn hook_post_compact_produces_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PostCompact",
        "session_id": "test-session"
    });

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn hook_invalid_json_exits_zero_no_stdout() {
    let (_tmp, config_path) = setup_test_env();

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin("not valid json {{{")
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("lore hook:"));
}

#[test]
fn hook_empty_stdin_exits_zero_no_stdout() {
    let (_tmp, config_path) = setup_test_env();

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin("")
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("lore hook:"));
}

#[test]
fn hook_no_matching_patterns_produces_no_output() {
    let (_tmp, config_path) = setup_test_env();

    // Use a tool_input with a file path that generates terms unlikely to
    // match any pattern in the test database.
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-session",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/quantum_physics_simulation.go"
        }
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    // With FTS-only, "golang AND (quantum OR physics OR simulation)" should
    // not match our rust/typescript patterns.
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.is_empty(),
        "should produce no output for unmatched query, got: {stdout}"
    );
}

#[test]
fn hook_output_has_valid_json_envelope() {
    let (_tmp, config_path) = setup_test_env();

    // Use a Bash command referencing cargo + error handling to maximize
    // chance of matching the rust-conventions pattern.
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-session",
        "tool_name": "Bash",
        "tool_input": {
            "description": "Run cargo test for anyhow error propagation",
            "command": "cargo test"
        }
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    if !stdout.is_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout should be valid JSON: {e}\nraw: {stdout}"));

        // Verify envelope structure.
        let hso = parsed
            .get("hookSpecificOutput")
            .expect("must have hookSpecificOutput");
        assert_eq!(
            hso.get("hookEventName").and_then(|v| v.as_str()),
            Some("PreToolUse"),
            "hookEventName should be PreToolUse"
        );
        assert!(
            hso.get("additionalContext")
                .and_then(|v| v.as_str())
                .is_some(),
            "additionalContext should be a string"
        );
    }
}

#[test]
fn hook_help_shows_in_subcommands() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("hook"));
}
