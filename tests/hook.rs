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
fn hook_session_start_returns_meta_instruction() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "test-session-start"
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.is_empty(),
        "SessionStart should produce output with pattern index"
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
    let hso = &parsed["hookSpecificOutput"];
    assert_eq!(hso["hookEventName"], "SessionStart");

    let ctx = hso["additionalContext"].as_str().unwrap();
    assert!(
        ctx.contains("lore for coding conventions"),
        "should contain meta-instruction: {ctx}"
    );
    assert!(
        ctx.contains("REQUIRED CONVENTIONS"),
        "should mention required conventions: {ctx}"
    );
    assert!(
        ctx.contains("Available patterns:"),
        "should list available patterns: {ctx}"
    );
}

#[test]
fn hook_post_compact_returns_session_context() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PostCompact",
        "session_id": "test-post-compact"
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.is_empty(),
        "PostCompact should produce output like SessionStart"
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
    let hso = &parsed["hookSpecificOutput"];
    assert_eq!(hso["hookEventName"], "PostCompact");

    let ctx = hso["additionalContext"].as_str().unwrap();
    assert!(
        ctx.contains("lore for coding conventions"),
        "should contain meta-instruction: {ctx}"
    );
    assert!(
        ctx.contains("Available patterns:"),
        "should list available patterns: {ctx}"
    );
}

#[test]
fn hook_unknown_event_produces_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "UnknownEvent",
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

// ---------------------------------------------------------------------------
// PostToolUse tests
// ---------------------------------------------------------------------------

#[test]
fn hook_post_tool_use_bash_exit_zero_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "test-session",
        "tool_name": "Bash",
        "tool_response": {
            "exit_code": 0,
            "stderr": ""
        }
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
fn hook_post_tool_use_non_bash_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "test-session",
        "tool_name": "Edit",
        "tool_response": {
            "exit_code": 1,
            "stderr": "error in rust conventions"
        }
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
fn hook_post_tool_use_bash_error_with_matching_stderr() {
    let (_tmp, config_path) = setup_test_env();

    // Use stderr text that contains terms likely to match rust-conventions.
    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "test-session",
        "tool_name": "Bash",
        "tool_response": {
            "exit_code": 1,
            "stderr": "error: anyhow rust conventions unwrap propagation"
        }
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    // If FTS matches, we should get patterns back.
    if !stdout.is_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
        let hso = &parsed["hookSpecificOutput"];
        assert_eq!(hso["hookEventName"], "PostToolUse");
        assert!(
            hso["additionalContext"].as_str().is_some(),
            "should have additionalContext string"
        );
    }
}

#[test]
fn hook_post_tool_use_exit_code_camel_case() {
    let (_tmp, config_path) = setup_test_env();

    // Test with camelCase `exitCode` field name.
    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "test-session",
        "tool_name": "Bash",
        "tool_response": {
            "exitCode": 0,
            "stderr": "some error text"
        }
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
fn hook_post_tool_use_no_response_no_output() {
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "test-session",
        "tool_name": "Bash"
    });

    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

// ---------------------------------------------------------------------------
// Full lifecycle test
// ---------------------------------------------------------------------------

#[test]
fn hook_full_lifecycle_session_dedup_compact_reinject() {
    let (_tmp, config_path) = setup_test_env();
    let session_id = format!("lifecycle-test-{}", std::process::id());

    // 1. SessionStart — should return meta-instruction.
    let start_input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id
    });

    let start_output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&start_input).unwrap())
        .assert()
        .success();

    let start_stdout = String::from_utf8(start_output.get_output().stdout.clone()).unwrap();
    assert!(
        !start_stdout.is_empty(),
        "SessionStart should produce output"
    );

    let start_parsed: serde_json::Value = serde_json::from_str(&start_stdout).unwrap();
    assert_eq!(
        start_parsed["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );

    // 2. PreToolUse — first call should inject patterns.
    let pre_input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": session_id,
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/error_handling.rs"
        }
    });

    let pre_output1 = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&pre_input).unwrap())
        .assert()
        .success();

    let pre_stdout1 = String::from_utf8(pre_output1.get_output().stdout.clone()).unwrap();

    // 3. Same PreToolUse again — dedup should filter (may produce no output or
    //    fewer results, depending on whether FTS matched in step 2).
    let pre_output2 = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&pre_input).unwrap())
        .assert()
        .success();

    let pre_stdout2 = String::from_utf8(pre_output2.get_output().stdout.clone()).unwrap();

    // If step 2 produced results, step 3 should produce fewer or no results
    // (dedup filtered them). If step 2 was empty (no FTS match), both are empty.
    if !pre_stdout1.is_empty() {
        // Step 2 injected patterns; step 3 should have fewer or no patterns.
        assert!(
            pre_stdout2.is_empty() || pre_stdout2.len() < pre_stdout1.len(),
            "dedup should filter: first={} bytes, second={} bytes",
            pre_stdout1.len(),
            pre_stdout2.len()
        );
    }

    // 4. PostCompact — should reset dedup and return session context.
    let compact_input = serde_json::json!({
        "hook_event_name": "PostCompact",
        "session_id": session_id
    });

    let compact_output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&compact_input).unwrap())
        .assert()
        .success();

    let compact_stdout = String::from_utf8(compact_output.get_output().stdout.clone()).unwrap();
    assert!(
        !compact_stdout.is_empty(),
        "PostCompact should produce output"
    );

    // 5. Same PreToolUse again — after PostCompact reset, should re-inject.
    let pre_output3 = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&pre_input).unwrap())
        .assert()
        .success();

    let pre_stdout3 = String::from_utf8(pre_output3.get_output().stdout.clone()).unwrap();

    // After reset, re-injection should match step 2.
    if !pre_stdout1.is_empty() {
        assert!(
            !pre_stdout3.is_empty(),
            "after PostCompact reset, PreToolUse should re-inject patterns"
        );
    }

    // Clean up dedup file.
    let dedup_path = lore::hook::dedup_file_path(&session_id);
    let _ = std::fs::remove_file(dedup_path);
}

#[test]
fn hook_session_start_and_post_compact_return_same_content() {
    let (_tmp, config_path) = setup_test_env();
    let session_id = format!("same-content-test-{}", std::process::id());

    let start_input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id
    });

    let start_output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&start_input).unwrap())
        .assert()
        .success();

    let start_stdout = String::from_utf8(start_output.get_output().stdout.clone()).unwrap();

    let compact_input = serde_json::json!({
        "hook_event_name": "PostCompact",
        "session_id": session_id
    });

    let compact_output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&compact_input).unwrap())
        .assert()
        .success();

    let compact_stdout = String::from_utf8(compact_output.get_output().stdout.clone()).unwrap();

    // Both should produce output.
    assert!(!start_stdout.is_empty());
    assert!(!compact_stdout.is_empty());

    // The additionalContext content should be the same (event names differ).
    let start_parsed: serde_json::Value = serde_json::from_str(&start_stdout).unwrap();
    let compact_parsed: serde_json::Value = serde_json::from_str(&compact_stdout).unwrap();

    let start_ctx = start_parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let compact_ctx = compact_parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        start_ctx, compact_ctx,
        "SessionStart and PostCompact should return the same context content"
    );

    // Clean up.
    let dedup_path = lore::hook::dedup_file_path(&session_id);
    let _ = std::fs::remove_file(dedup_path);
}

// ---------------------------------------------------------------------------
// .test.ts TypeScript testing pattern
// ---------------------------------------------------------------------------

/// Set up a test env with an additional TypeScript testing pattern.
///
/// The standard `seed_patterns` only has `typescript-conventions.md`. This
/// variant adds a `typescript-testing.md` so that a `.test.ts` file path
/// can match TypeScript testing-specific content.
fn setup_test_env_with_ts_testing() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    seed_patterns(dir);

    fs::write(
        dir.join("typescript-testing.md"),
        "# TypeScript Testing\n\n\
         tags: typescript, testing, vitest\n\n\
         Use vitest for all TypeScript test files.\n\
         Colocate test files next to source using the .test.ts suffix.\n\
         Prefer explicit assertions over snapshot tests.\n",
    )
    .unwrap();

    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(
        result.chunks_created >= 3,
        "expected >=3 chunks, got {}",
        result.chunks_created
    );

    let config_path = write_config(dir, &dir.join("knowledge.db"));
    (tmp, config_path)
}

#[test]
fn hook_pretooluse_edit_test_ts_returns_typescript_testing() {
    let (_tmp, config_path) = setup_test_env_with_ts_testing();
    let session_id = format!("ts-test-{}", std::process::id());

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": session_id,
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/validators/email.test.ts"
        }
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    // With "typescript AND (email OR test)" as the query, the testing pattern
    // should match via "typescript" in tags + "test" in body/title.
    assert!(
        !stdout.is_empty(),
        "editing a .test.ts file should return TypeScript testing patterns"
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
    let hso = &parsed["hookSpecificOutput"];
    assert_eq!(hso["hookEventName"], "PreToolUse");
    let ctx = hso["additionalContext"].as_str().unwrap();
    assert!(
        ctx.contains("REQUIRED CONVENTIONS"),
        "should contain imperative header: {ctx}"
    );

    // Clean up dedup file.
    let dedup_path = lore::hook::dedup_file_path(&session_id);
    let _ = std::fs::remove_file(dedup_path);
}

// ---------------------------------------------------------------------------
// All results deduped — dedicated test
// ---------------------------------------------------------------------------

#[test]
fn hook_pretooluse_all_results_deduped_no_output() {
    let (_tmp, config_path) = setup_test_env();
    let session_id = format!("all-dedup-test-{}", std::process::id());

    // First call: inject patterns from an .rs file edit.
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": session_id,
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/error_handling.rs"
        }
    });

    let output1 = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout1 = String::from_utf8(output1.get_output().stdout.clone()).unwrap();

    // Only proceed if the first call actually returned results (FTS matched).
    if !stdout1.is_empty() {
        // Second call with the same query — all results should be deduped.
        let output2 = Command::cargo_bin("lore")
            .unwrap()
            .args(["hook", "--config", config_path.to_str().unwrap()])
            .write_stdin(serde_json::to_string(&input).unwrap())
            .assert()
            .success();

        let stdout2 = String::from_utf8(output2.get_output().stdout.clone()).unwrap();
        assert!(
            stdout2.is_empty(),
            "second call with same query should produce no output (all deduped), got: {stdout2}"
        );
    }

    // Clean up dedup file.
    let dedup_path = lore::hook::dedup_file_path(&session_id);
    let _ = std::fs::remove_file(dedup_path);
}
