---
title: "Replace hardcoded tool count assertions with name-based checks"
priority: P3
category: testing
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/server.rs:tools_list_returns_all_five_tools
  - src/server.rs:mcp_round_trip
related_pr: feat/git-optional-knowledge-base
---

# Replace hardcoded tool count assertions with name-based checks

## Context

Two tests in `src/server.rs::tests` assert the exact number of MCP tools:

- `tools_list_returns_all_five_tools` (around line 880): `assert_eq!(tools.len(), 5)`
- `mcp_round_trip` (line 1366): `assert_eq!(tools.len(), 5)`

This branch already had to update both call sites when adding `lore_status`. Adding a sixth tool
will require:

1. Update `tool_definitions()` in src/server.rs
2. Update the snapshot file (rename + add the new tool entry)
3. Update `tools_list_returns_all_five_tools` (rename + change `5` to `6`)
4. Update `mcp_round_trip` (change `5` to `6`)
5. Update README.md MCP Tools table

Three of these are mechanical and easy to forget. A future contributor adding a tool may pass tests
on points 1-2 only and ship with broken assertions.

## Proposed fix

Replace `assert_eq!(tools.len(), 5)` with name-based checks that survive additions:

```rust
// in tools_list_returns_all_five_tools
let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
assert!(names.contains(&"search_patterns"));
assert!(names.contains(&"add_pattern"));
assert!(names.contains(&"update_pattern"));
assert!(names.contains(&"append_to_pattern"));
assert!(names.contains(&"lore_status"));
// snapshot test still catches additions/removals via insta
```

Similarly in `mcp_round_trip`:

```rust
let tools = resp["result"]["tools"].as_array().unwrap();
assert!(
    tools.iter().any(|t| t["name"] == "search_patterns"),
    "search_patterns must be present"
);
// repeat for the other expected tools
```

The snapshot test (`tools_list_returns_all_five_tools` uses `insta::assert_json_snapshot!`) still
catches both additions and removals via the snapshot file diff. The hardcoded count was redundant
with the snapshot.

## Trade-off

The test name `tools_list_returns_all_five_tools` becomes a misnomer if the test no longer asserts a
specific count. Either rename the test (`tools_list_includes_expected_tools`) or accept the
misnomer.

## References

- Adversarial finding (confidence 0.88): tool count brittleness
