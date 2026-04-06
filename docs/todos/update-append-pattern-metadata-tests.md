---
title: "Add commit_status metadata pin tests for update_pattern and append_to_pattern"
priority: P2
category: testing
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/server.rs::tests
related_pr: feat/git-optional-knowledge-base
---

# Add commit_status metadata pin tests for update_pattern and append_to_pattern

## Context

Commit `d7a20f1` introduced a structured `commit_status` metadata field on the MCP responses for the
three write tools. Only one test pins the new contract:

- `add_pattern_response_metadata_pins_commit_status_for_non_git_dir` (src/server.rs::tests)

`update_pattern` and `append_to_pattern` build identical metadata via the same helper code path, but
have no parallel test. A defect in either handler's metadata construction (e.g., a field name typo
specific to one tool) would evade detection.

## Proposed fix

Add two mirror tests in `src/server.rs::tests`:

```rust
#[test]
fn update_pattern_response_metadata_pins_commit_status_for_non_git_dir() {
    let h = TestHarness::new();
    // Pre-create a file so update has something to overwrite.
    let file = h.config.knowledge_dir.join("existing.md");
    std::fs::write(&file, "# Existing\n\nOld body that is long enough.\n").unwrap();

    let resp = h.request_value(
        r#"{
            "jsonrpc":"2.0","id":41,"method":"tools/call",
            "params":{
                "name":"update_pattern",
                "arguments":{
                    "source_file":"existing.md",
                    "body":"Brand new body content that is long enough."
                }
            }
        }"#,
    );

    assert!(resp["error"].is_null());
    let metadata = &resp["result"]["metadata"];
    assert_eq!(metadata["commit_status"]["kind"], "not_committed");
    assert!(metadata["chunks_indexed"].as_u64().unwrap() >= 1);
    assert!(metadata["file_path"].is_string());
}

#[test]
fn append_to_pattern_response_metadata_pins_commit_status_for_non_git_dir() {
    let h = TestHarness::new();
    let file = h.config.knowledge_dir.join("appendable.md");
    std::fs::write(&file, "# Appendable\n\nOriginal body content.\n").unwrap();

    let resp = h.request_value(
        r#"{
            "jsonrpc":"2.0","id":42,"method":"tools/call",
            "params":{
                "name":"append_to_pattern",
                "arguments":{
                    "source_file":"appendable.md",
                    "heading":"New Section",
                    "body":"Appended section content that is long enough."
                }
            }
        }"#,
    );

    assert!(resp["error"].is_null());
    let metadata = &resp["result"]["metadata"];
    assert_eq!(metadata["commit_status"]["kind"], "not_committed");
    assert!(metadata["chunks_indexed"].as_u64().unwrap() >= 1);
    assert!(metadata["file_path"].is_string());
}
```

These tests reuse the existing `TestHarness` and `request_value` helpers and follow the exact
pattern of `add_pattern_response_metadata_pins_commit_status_for_non_git_dir`.

## References

- Testing finding (confidence 0.75): metadata test coverage parity
