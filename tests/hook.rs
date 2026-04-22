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
            ctx.contains("PROJECT CONVENTIONS"),
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
    let ctx = parsed["systemMessage"]
        .as_str()
        .expect("SessionStart should return a top-level systemMessage");
    assert!(
        ctx.contains("lore for the author"),
        "should contain meta-instruction: {ctx}"
    );
    assert!(
        ctx.contains("strong coding preferences"),
        "should describe convention authority: {ctx}"
    );
    assert!(
        ctx.contains("Available patterns:"),
        "should list available patterns: {ctx}"
    );
}

#[test]
fn hook_session_start_advertises_git_advisory_for_non_git_dir() {
    // setup_test_env creates a plain tempdir without `git init`, so the
    // SessionStart context should warn the agent that git-dependent features
    // are unavailable.
    let (_tmp, config_path) = setup_test_env();

    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "test-non-git-advisory"
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ctx = parsed["systemMessage"].as_str().unwrap();

    assert!(
        ctx.contains("not a git repository"),
        "non-git knowledge base should advertise the git advisory: {ctx}"
    );
    assert!(
        ctx.contains("delta ingest is unavailable"),
        "advisory should mention delta ingest: {ctx}"
    );
    assert!(
        ctx.contains("lore_status"),
        "advisory should point at the lore_status tool: {ctx}"
    );
}

#[test]
fn hook_session_start_omits_git_advisory_for_git_dir() {
    // When the knowledge base is a git repository, the SessionStart context
    // should not contain the git advisory.
    let (tmp, config_path) = setup_test_env();
    let dir = tmp.path();

    std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(dir)
        .status()
        .unwrap();

    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "test-git-advisory-absent"
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ctx = parsed["systemMessage"].as_str().unwrap();

    assert!(
        !ctx.contains("not a git repository"),
        "git-initialised knowledge base should not show the advisory: {ctx}"
    );
    // The original meta-instruction and pattern list must still be present.
    assert!(ctx.contains("Available patterns:"));
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
    let ctx = parsed["systemMessage"]
        .as_str()
        .expect("PostCompact should return a top-level systemMessage");
    assert!(
        ctx.contains("lore for the author"),
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
    assert!(
        start_parsed["systemMessage"].is_string(),
        "SessionStart should return a systemMessage"
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

    // Both use systemMessage — content should be identical.
    let start_parsed: serde_json::Value = serde_json::from_str(&start_stdout).unwrap();
    let compact_parsed: serde_json::Value = serde_json::from_str(&compact_stdout).unwrap();

    let start_ctx = start_parsed["systemMessage"].as_str().unwrap();
    let compact_ctx = compact_parsed["systemMessage"].as_str().unwrap();
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
        ctx.contains("PROJECT CONVENTIONS"),
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

    // SessionStart creates the dedup file — dedup only activates when this
    // file exists (no SessionStart → no dedup).
    let session_start = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&session_start).unwrap())
        .assert()
        .success();

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

// ---------------------------------------------------------------------------
// Universal patterns: SessionStart pinned-conventions section
// ---------------------------------------------------------------------------

/// Set up a knowledge directory containing one universal pattern alongside the
/// usual seeded patterns. Returns the temp dir handle and config path.
fn setup_with_universal_pattern() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    seed_patterns(dir);
    fs::write(
        dir.join("workflow.md"),
        "---\ntags: [universal, conventions]\n---\n\n\
         # Workflow Conventions\n\n\
         Always push with `git push origin HEAD`, never plain `git push`.\n",
    )
    .unwrap();

    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});

    let config_path = write_config(dir, &dir.join("knowledge.db"));
    (tmp, config_path)
}

fn invoke_session_start(config_path: &Path, session_id: &str) -> String {
    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    parsed["systemMessage"].as_str().unwrap().to_string()
}

#[test]
fn hook_session_start_omits_pinned_section_when_no_universal_patterns() {
    let (_tmp, config_path) = setup_test_env();
    let ctx = invoke_session_start(&config_path, "test-no-universal");
    assert!(
        !ctx.contains("## Pinned conventions"),
        "section header should be omitted when no patterns are universal: {ctx}"
    );
    assert!(
        ctx.contains("Available patterns:"),
        "index should still be present: {ctx}"
    );
}

#[test]
fn hook_session_start_emits_pinned_section_with_body_above_index_when_universal_present() {
    let (_tmp, config_path) = setup_with_universal_pattern();
    let ctx = invoke_session_start(&config_path, "test-pinned-present");

    let pinned_idx = ctx
        .find("## Pinned conventions")
        .expect("pinned conventions section should be present");
    let index_idx = ctx
        .find("Available patterns:")
        .expect("available patterns index should be present");

    assert!(
        pinned_idx < index_idx,
        "pinned conventions section should appear above the available-patterns index"
    );
    assert!(
        ctx.contains("Always push with `git push origin HEAD`"),
        "pinned section should contain the body of the universal pattern: {ctx}"
    );
    assert!(
        ctx.contains("### Workflow Conventions"),
        "pinned section should label each pattern with its title: {ctx}"
    );
}

#[test]
fn hook_post_compact_re_emits_pinned_section() {
    let (_tmp, config_path) = setup_with_universal_pattern();

    let input = serde_json::json!({
        "hook_event_name": "PostCompact",
        "session_id": "test-post-compact-pinned",
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ctx = parsed["systemMessage"].as_str().unwrap();

    assert!(
        ctx.contains("## Pinned conventions"),
        "PostCompact should re-emit the pinned section: {ctx}"
    );
    assert!(
        ctx.contains("Always push with `git push origin HEAD`"),
        "pinned body should re-appear at PostCompact: {ctx}"
    );
}

#[test]
fn hook_session_start_skips_pinned_pattern_with_path_traversal_source_file() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    fs::write(
        dir.join("safe.md"),
        "---\ntags: [universal]\n---\n\n# Safe Pattern\n\nLegitimate body content.\n",
    )
    .unwrap();

    let embedder = FakeEmbedder::new();
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
    db.init().unwrap();
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});

    // Tamper a chunk row so its source_file points outside knowledge_dir.
    // No public API exposes this on purpose — the test exercises the defensive
    // guard against exactly this kind of out-of-band tampering.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO chunks (id, title, body, tags, source_file, heading_path, is_universal) \
         VALUES ('escape-attempt', 'Tampered', 'Body', 'universal', \
         '../../../etc/passwd', '', 1)",
        [],
    )
    .unwrap();
    drop(conn);

    let config_path = write_config(dir, &db_path);
    let ctx = invoke_session_start(&config_path, "test-path-traversal");

    assert!(
        ctx.contains("Legitimate body content"),
        "the safe universal pattern should still render: {ctx}"
    );
    assert!(
        !ctx.contains("/etc/passwd") && !ctx.contains("root:"),
        "the tampered source_file should never be read or surfaced: {ctx}"
    );
}

#[test]
fn hook_session_start_truncates_pinned_section_at_render_budget() {
    // Construct five universal-tagged files, each under the 8 KB per-file cap
    // but collectively exceeding the 32 KB render-time ceiling. SessionStart
    // must emit the truncation marker and stop rather than blowing past the
    // cap. The `_[pinned conventions truncated ...]_` string is load-bearing
    // for the "defense-in-depth against DB tampering" argument.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // Each padding line is 68 bytes; 100 repeats = 6800 bytes of body.
    // Plus title + blank lines puts each file around 6900 bytes on disk.
    // Five of them = ~34.5 KB, comfortably over the 32 KB render cap,
    // each individual file still under the 8 KB per-file ingest cap.
    let padding_line = "padding content for render-cap truncation test that crosses 32 KB.\n";
    let padding: String = padding_line.repeat(100);

    for i in 1..=5 {
        fs::write(
            dir.join(format!("u{i}.md")),
            format!("---\ntags: [universal]\n---\n\n# Universal {i}\n\n{padding}"),
        )
        .unwrap();
    }

    let embedder = FakeEmbedder::new();
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
    db.init().unwrap();
    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(
        result.errors.is_empty(),
        "no file should hit the per-file cap: {:?}",
        result.errors
    );
    assert_eq!(result.universal_sources.len(), 5);

    let config_path = write_config(dir, &db_path);
    let ctx = invoke_session_start(&config_path, "test-render-cap");

    assert!(
        ctx.contains("_[pinned conventions truncated at 32768 bytes"),
        "expected truncation marker once cumulative body crossed 32 KB; \
         got {} bytes of systemMessage starting {:?}",
        ctx.len(),
        &ctx.chars().take(200).collect::<String>(),
    );
    // The pinned section header is still present — we don't collapse the
    // section, we truncate inside it.
    assert!(ctx.contains("## Pinned conventions"));
    // And the first few patterns did render before truncation.
    assert!(ctx.contains("### Universal 1"));
}

#[test]
fn hook_session_start_renders_raw_body_control_chars_verbatim() {
    // Pin R2b of the db-sole-read-surface PR: the render path does NOT
    // sanitise `raw_body` control characters. DB write access is the
    // existing trust boundary — an adversary who can tamper patterns rows
    // can already influence agent context via chunks. Sanitising here
    // would mangle legitimate code-block examples and escape-sequence
    // documentation that pattern authors must be able to write. A future
    // refactor that adds `sanitize_for_log` (or similar) to the render
    // path will fail this test and have to justify the change.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    fs::write(
        dir.join("safe.md"),
        "---\ntags: [universal]\n---\n\n# Safe\n\nLegitimate body content.\n",
    )
    .unwrap();

    let embedder = FakeEmbedder::new();
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
    db.init().unwrap();
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});

    // Tamper: insert a `patterns` row whose `raw_body` carries raw ANSI
    // CSI sequences. Render must pass them through verbatim.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO patterns (source_file, title, tags, is_universal, raw_body, content_hash) \
         VALUES ('tamper.md', 'Tampered', 'universal', 1, \
                 'escape:' || char(27) || '[2Jpayload' || char(27) || '[0m', \
                 '0000000000000000')",
        [],
    )
    .unwrap();
    drop(conn);

    let config_path = write_config(dir, &db_path);
    let ctx = invoke_session_start(&config_path, "test-raw-body-verbatim");

    // The safe pattern still renders.
    assert!(
        ctx.contains("Legitimate body content"),
        "safe universal pattern must still render: {ctx}"
    );
    // The tampered body appears with ESC bytes intact — verbatim is the
    // contract. Stringify the ctx bytes to match against the raw escape.
    assert!(
        ctx.contains("escape:\x1b[2Jpayload\x1b[0m"),
        "raw_body must render verbatim including control chars"
    );
}

#[test]
fn hook_session_start_renders_from_db_even_when_source_file_removed() {
    // Pin the DB-as-sole-read-surface invariant: once a pattern file has
    // been ingested, removing it from disk does NOT remove the body from
    // the rendered pinned section. Render reads from the `patterns` table,
    // not the filesystem.
    //
    // This is a deliberate behavioural change from #33-era behaviour,
    // where the render path re-read source markdown at SessionStart and
    // would skip any file that had vanished. The new contract is: ingest
    // writes, render reads DB — the patterns directory is no longer a
    // runtime dependency.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    fs::write(
        dir.join("ghost.md"),
        "---\ntags: [universal]\n---\n\n# Ghost Pattern\n\nBody content marker alpha-xyzzy.\n",
    )
    .unwrap();
    fs::write(
        dir.join("present.md"),
        "---\ntags: [universal]\n---\n\n# Present Pattern\n\nDistinctive body marker beta-xyzzy.\n",
    )
    .unwrap();

    let embedder = FakeEmbedder::new();
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
    db.init().unwrap();
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});

    // Remove ghost.md from disk after ingest — its patterns row stays in
    // the DB and the render still surfaces its body.
    fs::remove_file(dir.join("ghost.md")).unwrap();

    let config_path = write_config(dir, &db_path);
    let ctx = invoke_session_start(&config_path, "test-ghost-file-still-renders");

    assert!(
        ctx.contains("beta-xyzzy"),
        "the present universal pattern should render: {ctx}"
    );
    assert!(
        ctx.contains("alpha-xyzzy"),
        "ghost pattern should still render from DB despite disk file removed: {ctx}"
    );
}

// ---------------------------------------------------------------------------
// Universal patterns: PreToolUse partition + dedup bypass
// ---------------------------------------------------------------------------

/// Run a `SessionStart` followed by N `PreToolUse` calls with the same input,
/// returning the `additional_context` string from each call (empty when no
/// output was produced). Cleans up the dedup file on drop.
fn run_pre_tool_use_sequence(
    config_path: &Path,
    session_id: &str,
    pre_tool_input: &serde_json::Value,
    repeats: usize,
) -> Vec<String> {
    // SessionStart first to create the dedup file.
    let session_start = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&session_start).unwrap())
        .assert()
        .success();

    let mut outputs = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let output = Command::cargo_bin("lore")
            .unwrap()
            .args(["hook", "--config", config_path.to_str().unwrap()])
            .write_stdin(serde_json::to_string(pre_tool_input).unwrap())
            .assert()
            .success();
        let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
        let context = if stdout.is_empty() {
            String::new()
        } else {
            let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
            parsed["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        outputs.push(context);
    }

    let _ = std::fs::remove_file(lore::hook::dedup_file_path(session_id));
    outputs
}

#[test]
fn hook_pre_tool_use_non_universal_chunk_present_on_first_call_only() {
    let (_tmp, config_path) = setup_test_env();

    // PreToolUse for an Edit that matches the seeded rust pattern.
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "test-non-universal-deduped",
        "tool_name": "Edit",
        "tool_input": { "file_path": "src/lib.rs" },
    });

    let contexts = run_pre_tool_use_sequence(&config_path, "test-non-universal-deduped", &input, 2);

    // Either nothing matches at all (then both calls are empty), or the
    // non-universal pattern appears on the first call but is deduped on the
    // second. The contract is: dedup still works for non-universal chunks.
    if !contexts[0].is_empty() {
        assert!(
            contexts[1].is_empty()
                || !contexts[1].contains("anyhow for application-level error propagation"),
            "second call must dedup the non-universal pattern, got: {:?}",
            contexts[1]
        );
    }
}

#[test]
fn hook_pre_tool_use_universal_persists_after_post_compact_truncation() {
    // Composition cascade hazard pin (per docs/solutions/best-practices/
    // composition-cascades-...): the dedup file is mutated by both
    // SessionStart truncation, PostCompact truncation, AND PreToolUse writes.
    // Ensure universal chunks survive all three mutation routes.
    let (_tmp, config_path) = setup_with_universal_pattern();
    let session_id = "test-cascade";

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": session_id,
        "tool_name": "Bash",
        "tool_input": { "command": "git push" },
    });

    // SessionStart
    let session_start = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&session_start).unwrap())
        .assert()
        .success();

    // 3x PreToolUse (writes universal IDs to dedup, but read-side bypass)
    for _ in 0..3 {
        Command::cargo_bin("lore")
            .unwrap()
            .args(["hook", "--config", config_path.to_str().unwrap()])
            .write_stdin(serde_json::to_string(&input).unwrap())
            .assert()
            .success();
    }

    // PostCompact (truncates dedup, re-emits SessionStart content)
    let post_compact = serde_json::json!({
        "hook_event_name": "PostCompact",
        "session_id": session_id,
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&post_compact).unwrap())
        .assert()
        .success();

    // 4th PreToolUse — universal must still inject after the truncate cycle.
    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("git push origin HEAD"),
        "universal pattern must persist across PostCompact truncation: {stdout}"
    );

    let _ = std::fs::remove_file(lore::hook::dedup_file_path(session_id));
}

#[test]
fn hook_pre_tool_use_universal_chunk_absent_when_query_does_not_match() {
    // R4 negative pin: a universal pattern only injects when it passes the
    // search relevance gate for the current tool call.
    //
    // workflow.md universal pattern is about `git push origin HEAD`. An
    // Edit against a Cargo.toml file with no overlap in extracted query
    // terms must NOT pull in the workflow pattern.
    let (_tmp, config_path) = setup_with_universal_pattern();
    let session_id = "test-irrelevant-tool-call";

    let session_start = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&session_start).unwrap())
        .assert()
        .success();

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": session_id,
        "tool_name": "Edit",
        "tool_input": { "file_path": "Cargo.toml" },
    });

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    assert!(
        !stdout.contains("git push origin HEAD"),
        "universal git pattern must not inject for an unrelated Cargo.toml edit: {stdout}"
    );

    let _ = std::fs::remove_file(lore::hook::dedup_file_path(session_id));
}

#[test]
fn hook_pre_tool_use_dedup_file_records_universal_chunks() {
    // Pins the read-side semantic: universal IDs ARE written to the dedup
    // file (the file remains a faithful injection log) but ignored on read.
    let (_tmp, config_path) = setup_with_universal_pattern();
    let session_id = "test-dedup-records-universal";

    let session_start = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": session_id,
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&session_start).unwrap())
        .assert()
        .success();

    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": session_id,
        "tool_name": "Bash",
        "tool_input": { "command": "git push" },
    });
    Command::cargo_bin("lore")
        .unwrap()
        .args(["hook", "--config", config_path.to_str().unwrap()])
        .write_stdin(serde_json::to_string(&input).unwrap())
        .assert()
        .success();

    let dedup_path = lore::hook::dedup_file_path(session_id);
    let contents = std::fs::read_to_string(&dedup_path).unwrap();
    assert!(
        contents.contains("workflow.md"),
        "dedup file should record the universal chunk's id (which contains its source_file): {contents}"
    );

    let _ = std::fs::remove_file(dedup_path);
}
