// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for `lore extract-queries`.
//!
//! These tests pin the stdin envelope shape and output contract so the
//! coverage-check skill can rely on stable behavior when piping synthetic
//! tool calls through the subcommand.

use assert_cmd::Command;
use predicates::prelude::*;

/// Run `lore extract-queries` with the given JSON on stdin and return stdout.
fn run(stdin: &str) -> String {
    let output = Command::cargo_bin("lore")
        .expect("binary exists")
        .arg("extract-queries")
        .write_stdin(stdin.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("valid utf-8")
}

#[test]
fn extract_queries_edit_rust_file_emits_language_anchor() {
    let out = run(r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs"}}"#);
    assert!(
        out.starts_with("rust"),
        "expected rust anchor, got: {out:?}"
    );
}

#[test]
fn extract_queries_bash_cargo_command_emits_rust_anchor_with_enrichment() {
    let out = run(r#"{"tool_name":"Bash","tool_input":{"command":"cargo deny check"}}"#);
    assert!(out.contains("rust"), "expected rust anchor, got: {out:?}");
    assert!(out.contains("deny") || out.contains("check"));
}

#[test]
fn extract_queries_edit_typescript_file_emits_typescript_anchor() {
    let out = run(r#"{"tool_name":"Edit","tool_input":{"file_path":"app/page.tsx"}}"#);
    assert!(out.contains("typescript"), "got: {out:?}");
}

#[test]
fn extract_queries_bare_bash_command_without_language_signal_emits_empty_stdout() {
    // `just ci` — "just" is a stop-word, "ci" is filtered as too short
    // (< 3 chars), so nothing survives cleaning. This degenerate case is
    // the coverage-check skill's diagnostic signal for weak discoverability.
    let out = run(r#"{"tool_name":"Bash","tool_input":{"command":"just ci"}}"#);
    assert_eq!(out.trim(), "", "expected empty stdout for degenerate case");
}

#[test]
fn extract_queries_invalid_json_exits_nonzero_with_stderr_message() {
    Command::cargo_bin("lore")
        .expect("binary exists")
        .arg("extract-queries")
        .write_stdin("not json".to_string())
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid JSON"));
}
