---
title: "Add a schema version field to the MCP initialize response"
priority: P3
category: api-contract
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/server.rs:initialize_response
related_pr: feat/git-optional-knowledge-base
---

# Add a schema version field to the MCP initialize response

## Context

The lore MCP server responds to `initialize` with a static `serverInfo`:

```json
{
  "name": "lore",
  "version": "0.1.0"
}
```

Agents that consume the MCP server have no programmatic way to detect when the response shape, tool
list, or metadata schema changes between lore versions. They must either:

- Trust the `version` field (which tracks crate version, not API contract)
- Diff snapshot files manually
- Read the changelog (no agent-readable changelog exists yet)

This branch added new metadata fields and a new tool. A future agent that relies on the
`metadata.commit_status` field has no way to know whether a specific lore version supports it.

The lore project is in active development with no external MCP consumers, so this is not
load-bearing yet — but it becomes harder to retrofit once consumers exist.

## Proposed fix

Add a `schemaVersion` field to the initialize response. Use a date-based version that increments
only when the API contract changes (not the crate version):

```rust
fn initialize_response(req: &JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: Some(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "lore",
                "version": env!("CARGO_PKG_VERSION"),
                "schemaVersion": "2026-04-06-v1"
            }
        })),
        error: None,
    }
}
```

The `schemaVersion` value should be bumped only when:

- A new MCP tool is added or removed
- A response shape changes (field added, removed, or renamed)
- A tool description changes meaningfully (not just typo fixes)

A consumer that pins `schemaVersion: "2026-04-06-v1"` knows exactly which contract they are coding
against.

## Test surface

Update the existing `initialize_response` snapshot in `src/server.rs::tests` to include the new
field. The test already uses `insta::assert_json_snapshot!` so the change is one snapshot accept.

## When to do this

Defer until the first external MCP consumer appears (or until a breaking change is necessary).
Premature versioning creates churn without benefit.

## References

- Agent-native finding (confidence 0.65): no agent-facing changelog
- API-contract residual risk: schema versioning strategy
