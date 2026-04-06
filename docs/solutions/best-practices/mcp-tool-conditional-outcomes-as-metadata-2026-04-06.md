---
title: Expose MCP tool conditional outcomes as structured metadata
date: 2026-04-06
category: best-practices
module: mcp-server
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - Designing a new MCP tool with conditional behaviour (e.g. a tool that commits to git only when the knowledge base is a git repository)
  - Adding a fallback or degraded path to an existing MCP tool
  - Reviewing MCP tool descriptions for accuracy against runtime behaviour
  - Exposing operational state to agents that consume the MCP server programmatically
tags:
  - mcp
  - agent-native
  - tool-design
  - structured-output
  - response-shape
  - observability
---

# Expose MCP tool conditional outcomes as structured metadata

## Context

When an MCP tool has conditional behaviour — "commits to git when in a git repository, otherwise
writes the file without committing" — three surfaces must stay in sync:

1. **The tool description** that agents read in `tools/list`
2. **The response shape** that agents read after calling the tool
3. **The tests** that pin both above against future refactors

If any of the three lag behind reality, agents make wrong assumptions. The lore project hit this
when correcting documentation that incorrectly required a git repository: the underlying code had
always supported plain directories with `CommitStatus::NotCommitted`, but agents calling
`add_pattern` saw a description claiming "commits to git" and a text response saying "Pattern saved
to foo.md (3 chunks indexed)" — and concluded the commit had happened. There was no programmatic way
to detect the degraded state without parsing the human-readable suffix `, committed to git`, which
wasn't even appended in the non-git case.

The fix wasn't complicated. It was visible only when someone asked the basic question "is git
actually required?" and traced the answer through the docs, the tool description, the response
shape, and the test suite.

## Guidance

When an MCP tool's behaviour branches on environment state, follow all three of:

### 1. State both branches in the tool description

The `tools/list` description is the contract agents read first. Soften unconditional claims:

```rust
// BEFORE (misleading):
"description": "Create a new pattern in the knowledge base. Use only when the user explicitly \
                asks to save, record, or document a pattern. Creates a markdown file, indexes it, \
                and commits to git."

// AFTER (accurate):
"description": "Create a new pattern in the knowledge base. Use only when the user explicitly \
                asks to save, record, or document a pattern. Creates a markdown file and indexes \
                it; the change is committed to git when the knowledge base is a git repository, \
                otherwise the file is written without a commit."
```

### 2. Surface the actual outcome as a structured metadata field

The MCP spec is permissive about extra fields on `result`. Add a `metadata` sibling to the existing
`content` block. Existing consumers reading `result.content[0].text` continue to work unchanged —
the change is purely additive.

```rust
fn text_response_with_metadata(
    req: &JsonRpcRequest,
    text: &str,
    metadata: &Value,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: Some(json!({
            "content": [{ "type": "text", "text": text }],
            "metadata": metadata,
        })),
        error: None,
    }
}
```

For the conditional outcome itself, use a tagged union (a `kind` discriminator) so adding new
variants in the future is mechanical:

```rust
/// Render a `CommitStatus` as a structured JSON value for MCP response metadata.
///
/// The match is intentionally exhaustive (no wildcard arm) so adding a new
/// `CommitStatus` variant fails to compile until this function is updated.
/// MCP consumers branch on `commit_status.kind`, so a new variant must
/// produce a documented JSON shape rather than silently mapping to "unknown".
fn commit_status_metadata(status: &CommitStatus) -> Value {
    match status {
        CommitStatus::NotCommitted => json!({ "kind": "not_committed" }),
        CommitStatus::Committed => json!({ "kind": "committed" }),
        CommitStatus::Pushed { branch } => json!({ "kind": "pushed", "branch": branch }),
    }
}
```

The exhaustive match (no wildcard arm) is load-bearing: it makes "add a variant without updating the
metadata renderer" fail to compile, so the contract cannot drift silently.

Wire it into each handler that produces the conditional outcome:

```rust
Ok(result) => {
    let metadata = json!({
        "file_path": result.file_path,
        "chunks_indexed": result.chunks_indexed,
        "embedding_failures": result.embedding_failures,
        "commit_status": commit_status_metadata(&result.commit_status),
    });
    text_response_with_metadata(
        req,
        &format!(
            "Pattern \"{}\" saved to {} ({} chunks indexed{}).",
            title, result.file_path, result.chunks_indexed, commit_note(&result.commit_status)
        ),
        &metadata,
    )
}
```

### 3. Pin the metadata contract with a test that asserts on the structured field

Tests that only assert on `content[0].text` cannot catch a regression where the metadata field is
dropped, renamed, or restructured. Add an explicit assertion on `result.metadata`:

```rust
#[test]
fn add_pattern_response_metadata_pins_commit_status_for_non_git_dir() {
    // The TestHarness uses a plain tempdir for knowledge_dir (no `git init`),
    // so add_pattern should report commit_status = "not_committed" in the
    // structured metadata block. Agents reading the metadata field can detect
    // the degraded state without parsing the human-readable content text.
    let h = TestHarness::new();
    let resp = h.request_value(/* ... add_pattern call ... */);

    let metadata = &resp["result"]["metadata"];
    assert_eq!(metadata["commit_status"]["kind"], "not_committed");
    assert!(metadata["chunks_indexed"].as_u64().unwrap() >= 1);
    assert!(metadata["file_path"].is_string());
}
```

Pin both branches if both are reachable in the test environment. For lore, the positive case lives
in `add_pattern_commits_in_git_repo` (which runs `git init` in a tempdir), and the negative case
lives in the test above.

## Why This Matters

The cost of getting any of the three surfaces wrong is invisible until something downstream breaks.

- **Tool description wrong** → agents form wrong mental models, plan wrong workflows
- **Response shape opaque** → agents cannot verify the outcome, may proceed on the wrong assumption
- **Test missing** → a future refactor silently drops the metadata field, no one notices

The lore project had all three problems at once for several weeks. The git advisory in `cmd_init`,
the conditional in the tool descriptions, and the structured metadata field all landed in the same
PR because they are _the same fix viewed from three angles_. A documentation-only correction would
have closed the gap for human readers but left agents in the dark.

A second-order benefit: pinning the metadata contract in tests means the documentation can refer to
specific field names (`metadata.commit_status.kind`) with confidence that they won't go stale.
Without the test, the docs become aspirational; with it, they become a contract.

## When to Apply

- Designing a new MCP tool whose behaviour depends on environment state (filesystem, network,
  configuration, mode flags, feature toggles)
- Adding a fallback or graceful-degradation path to an existing MCP tool that previously assumed one
  runtime context
- Reviewing MCP tool descriptions during a code review and noticing the description omits a
  conditional that the implementation handles
- Correcting documentation that overpromised behaviour — also audit the corresponding tool
  descriptions and response shapes
- Onboarding a new agent or MCP client that needs to detect operational state programmatically

## Examples

### Before: tool description and response with no observability

```jsonc
// tools/list
{
  "name": "add_pattern",
  "description": "Creates a markdown file, indexes it, and commits to git.",
  "inputSchema": { /* ... */ }
}

// tools/call response
{
  "content": [{ "type": "text", "text": "Pattern \"Foo\" saved to foo.md (3 chunks indexed)." }]
}
```

An agent calling this tool against a non-git directory has no signal that the commit was skipped.

### After: tool description and response with structured outcome

```jsonc
// tools/list
{
  "name": "add_pattern",
  "description": "Creates a markdown file and indexes it; the change is committed to git when the knowledge base is a git repository, otherwise the file is written without a commit.",
  "inputSchema": { /* ... */ }
}

// tools/call response — non-git directory
{
  "content": [{ "type": "text", "text": "Pattern \"Foo\" saved to foo.md (3 chunks indexed)." }],
  "metadata": {
    "file_path": "foo.md",
    "chunks_indexed": 3,
    "embedding_failures": 0,
    "commit_status": { "kind": "not_committed" }
  }
}

// tools/call response — git directory with successful commit
{
  "content": [{ "type": "text", "text": "Pattern \"Foo\" saved to foo.md (3 chunks indexed, committed to git)." }],
  "metadata": {
    "file_path": "foo.md",
    "chunks_indexed": 3,
    "embedding_failures": 0,
    "commit_status": { "kind": "committed" }
  }
}
```

Agents now branch on `metadata.commit_status.kind` directly. The text content is for humans; the
metadata is for machines.

### Companion: a status tool for pre-flight checks

When a tool has conditional behaviour driven by environment state, also consider exposing that state
via a dedicated read-only tool. lore added a `lore_status` MCP tool alongside the metadata field so
agents can verify the knowledge base is in the expected state _before_ calling write tools, rather
than discovering the degraded state from the metadata field after the fact:

```jsonc
// tools/call lore_status — non-git directory
{
  "content": [
    {
      "type": "text",
      "text": "Knowledge base: /tmp/patterns — 0 chunks across 0 sources. Git repository: no. Delta ingest: unavailable (full ingest only). Inbox workflow: not configured.",
    },
  ],
  "metadata": {
    "knowledge_dir": "/tmp/patterns",
    "git_repository": false,
    "delta_ingest_available": false,
    "inbox_workflow_configured": false,
    "last_ingested_commit": null,
    "chunks_indexed": 0,
    "sources_indexed": 0,
  },
}
```

The pre-flight check and the post-action metadata serve different purposes — `lore_status` lets
agents _plan_, the metadata field lets agents _verify_. Both are needed for full agent-native
parity.

## Related

- [Suppress stderr diagnostics when --json mode is active](cli-suppress-stderr-in-json-mode-2026-04-03.md)
  — sibling pattern about machine-consumer contracts at the CLI layer. Both learnings express the
  same principle (machine consumers need explicit, structured contracts) but at different layers.
- [CLI data commands should output to stdout, not stderr](cli-data-commands-should-output-to-stdout-2026-04-02.md)
  — foundational convention that the CLI counterpart refines.
- PR #30 (`feat/git-optional-knowledge-base`) — the change that produced this learning, including
  the eight commits across documentation, MCP server, hook prompt, and tests.
