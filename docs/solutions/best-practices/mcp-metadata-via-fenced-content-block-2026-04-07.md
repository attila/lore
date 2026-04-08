---
title: Carry MCP tool metadata in a fenced content block, not a result sibling
date: 2026-04-07
category: best-practices
module: mcp-server
problem_type: best_practice
component: tooling
severity: high
supersedes: docs/solutions/best-practices/mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md
applies_when:
  - Designing a new MCP tool whose agent consumer needs structured (non-prose) fields from the response
  - Adding machine-readable observability to an existing MCP tool used by Claude Code
  - Reviewing a tool response schema that relies on a `metadata` sibling on `result`
  - Onboarding a skill or plugin that consumes MCP tool output programmatically from inside Claude Code
tags:
  - mcp
  - claude-code
  - agent-native
  - tool-design
  - structured-output
  - response-shape
  - client-surfacing
---

# Carry MCP tool metadata in a fenced content block, not a result sibling

## Context

Lore's MCP server used to attach a `metadata` sibling to `result` on every tool response
(`text_response_with_metadata(req, text, metadata)`). The sibling carried structured fields (file
paths, chunk counts, commit status, search result ranks, search mode flags) that agents were
supposed to read directly without parsing the human-readable prose. The pattern was documented in
the superseded learning
[`mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md`](mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md)
and the project's unit tests verified the wire format by reading `resp["result"]["metadata"][...]`
directly from the `JsonRpcResponse` returned by `handle_request`.

During PR #32 (coverage-check skill) a real-run test on a separate machine found that Claude Code's
MCP client **strips the `metadata` sibling from `result` before forwarding the response to the
agent**. The agent only sees the `content[]` array. The server was emitting the sibling correctly
(verified by piping `printf '...' | lore serve | jq '.result | keys'`, which returned
`["content", "metadata"]`), but by the time the agent saw the tool output inside a Claude Code chat
session, the metadata was gone.

This made every lore MCP tool that used `text_response_with_metadata` silently broken for the
primary consumer:

- `search_patterns` could not expose `mode` (`hybrid` / `fts_fallback` / `fts_only`), per-row
  `rank`, `source_file`, or `score`.
- `lore_status` could not expose `knowledge_dir`, `git_repository`, `chunks_indexed`,
  `loreignore_active`, or the other status fields.
- `add_pattern`, `update_pattern`, `append_to_pattern` could not expose the written `file_path`,
  `chunks_indexed`, or `commit_status` discriminated union.

Unit tests passed because they asserted against the wire format directly. End-to-end tests from
inside Claude Code would have caught this, but the project had none — the assumption that "metadata
sibling on `result` is accessible to the agent" was baked into the plan, the brainstorm, the code,
and the learning doc without anyone verifying it against Claude Code specifically.

## Finding

An MCP tool response has two places the server can put structured data:

1. **Sibling field on `result`** (e.g. `result.metadata`). This is an extension beyond the core MCP
   spec, which defines `content[]` but does not require additional sibling fields to be forwarded.
   **Claude Code strips it.** Other MCP clients may or may not forward it — no client in the lore
   project's current consumer set surfaces it to agents.

2. **Inside `content[0].text`** (or any `content[i].text` block). This is the standard,
   spec-mandated text surface. **Every spec-compliant MCP client forwards it.** Claude Code does —
   and concatenates multiple `content[i].text` entries into a single flat string with no delimiter,
   so even a multi-block design collapses to "append text to the first block's body".

The lesson: **agent-readable structured data in MCP tool responses must travel inside
`content[0].text`, never in a sibling on `result`.** The sibling is a debugging and testing surface,
not a production channel.

## Guidance

For any MCP tool whose agent consumer needs structured fields from the response, use the **fenced
content block** pattern. The production implementation in lore lives at
[`src/server.rs`](../../../src/server.rs) — see `maybe_append_lore_metadata_fence`,
`include_metadata_arg`, and the `LORE_METADATA_FENCE_TAG` constant.

### 1. Add an `include_metadata: bool` parameter to the tool schema

Make the fenced block opt-in. Default callers (skills that only want the prose body, hook-injected
queries, general-purpose agent tool calls) pay no token cost for the embedded JSON. Only callers
that need the structured channel pass `include_metadata: true`.

```rust
// In tool_definitions():
{
    "name": "search_patterns",
    "description": "Search the knowledge base for software patterns, ...",
    "inputSchema": {
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Natural language search query" },
            "top_k": { "type": "number", "description": "Number of results to return" },
            "include_metadata": {
                "type": "boolean",
                "description": "When true, appends a `lore-metadata` fenced code block \
                                to the end of the response containing machine-readable JSON \
                                with per-row rank/source_file/score and top-level mode. \
                                Defaults to false. Opt-in because the fenced block bloats \
                                the response and most callers only need the prose.",
                "default": false
            }
        },
        "required": ["query"]
    }
}
```

### 2. Append a fenced block to the prose when requested

Use a unique language tag that will not collide with pattern body content (`lore-metadata` in the
lore project). The fence must start with a blank line so it renders cleanly regardless of how the
prose body ended:

````rust
const LORE_METADATA_FENCE_TAG: &str = "lore-metadata";

fn maybe_append_lore_metadata_fence(
    prose: String,
    metadata: &Value,
    include_metadata: bool,
) -> String {
    if !include_metadata {
        return prose;
    }
    let json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
    format!("{prose}\n\n```{LORE_METADATA_FENCE_TAG}\n{json}\n```")
}
````

`serde_json::to_string` escapes newlines as `\n`, so the serialised JSON payload contains no real
newline characters. That matters: the client-side extractor looks for a newline followed by
triple-backtick as the closing fence marker, and would otherwise false-match on any newline inside a
JSON string value. With escaped newlines, the first newline-then-fence after the opening marker is
unambiguously the closing fence.

### 3. Read the parameter in the handler and call the helper

```rust
fn handle_search(req: &JsonRpcRequest, ctx: &ServerContext<'_>, args: &Value) -> JsonRpcResponse {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    let include_metadata = include_metadata_arg(args);
    // ... build `prose` and `metadata` as before ...
    let prose_with_fence = maybe_append_lore_metadata_fence(prose, &metadata, include_metadata);
    text_response(req, &prose_with_fence)
}

fn include_metadata_arg(args: &Value) -> bool {
    args.get("include_metadata")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
```

Every handler that previously called `text_response_with_metadata` now builds its metadata `Value`
as before, passes it through the helper, and calls `text_response` with the possibly-augmented
prose. The `text_response_with_metadata` helper is removed entirely.

### 4. Pin the contract with tests

Tests that previously asserted on `resp["result"]["metadata"][field]` now need to extract the JSON
from the fenced block. Add a test helper:

````rust
#[cfg(test)]
fn extract_lore_metadata_fence(text: &str) -> Option<Value> {
    let opening = format!("\n\n```{LORE_METADATA_FENCE_TAG}\n");
    let start = text.rfind(&opening)?;
    let after_opening = &text[start + opening.len()..];
    let end = after_opening.find("\n```")?;
    let json_str = &after_opening[..end];
    serde_json::from_str(json_str).ok()
}
````

And use it in test assertions:

```rust
#[test]
fn search_patterns_response_metadata_pins_hybrid_shape() {
    // Arrange
    let h = TestHarness::new();
    // ... insert chunk ...

    // Act
    let resp = h.request_value(
        r#"{
            "jsonrpc":"2.0","id":10,"method":"tools/call",
            "params":{
                "name":"search_patterns",
                "arguments":{"query":"cargo deny","include_metadata":true}
            }
        }"#,
    );

    // Assert
    assert!(resp["error"].is_null());
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let metadata = extract_lore_metadata_fence(text).expect("expected lore-metadata fence");
    assert_eq!(metadata["mode"], "hybrid");
    assert_eq!(metadata["results"][0]["rank"], 1);
    // ...
}
```

Add a companion test that verifies the default path (without `include_metadata: true`) omits the
fence entirely:

````rust
#[test]
fn search_patterns_omits_metadata_fence_by_default() {
    // ... same setup ...
    let resp = h.request_value(r#"{ /* ... no include_metadata ... */ }"#);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        !text.contains("```lore-metadata"),
        "default response must not contain a lore-metadata fence"
    );
    assert!(extract_lore_metadata_fence(text).is_none());
}
````

Both tests pin the opt-in contract: the fence appears only when the caller asks for it, and when it
does appear its shape is stable.

## Skill-side consumption

A Claude Code skill (or any MCP client) that needs the structured data extracts it from the tool
response's `content[0].text` using the same recipe:

1. Locate the last opening marker — a blank line followed by a triple-backtick fence with language
   tag `lore-metadata`.
2. Advance past the opening marker.
3. Read forward until the next closing marker — a triple-backtick fence on its own line.
4. Parse the intervening text as JSON.

The coverage-check skill at
[`integrations/claude-code/skills/coverage-check/SKILL.md`](../../../integrations/claude-code/skills/coverage-check/SKILL.md)
uses this pattern in steps 2 (R1a pre-flight reading `knowledge_dir` from `lore_status`), 6 (cascade
detection reading `mode` from `search_patterns`), and 8 (per-query state classification reading
`results[].rank` and `results[].source_file` from each batch response).

## Why this matters

The failure mode was invisible until production testing. Unit tests that dispatched through
`handle_request` directly saw the metadata sibling and passed. Integration tests that used the MCP
server over stdio (if any existed) would have also seen it, because the stdio wire format carries
the sibling. Only tests that ran **inside a Claude Code session and inspected what the agent
actually receives** could have caught the stripping. The project had none.

Three surfaces need to stay in sync for structured MCP data:

1. The tool description in `tools/list` (unchanged from the superseded learning — still describe
   both branches of conditional behaviour).
2. The transport channel used to carry the structured data (`content[0].text` with a fenced block,
   not `result.metadata`).
3. The agent-side extractor that parses the channel.

Regressions in any of the three fail silently. Tests that assert end-to-end from inside a real MCP
client are the only way to catch client-side stripping like Claude Code's.

## When to apply

- Any new MCP tool design where an agent needs structured fields from the response.
- Any existing MCP tool that relies on a `metadata` sibling on `result` and has Claude Code as a
  consumer — migrate it to the fenced block.
- Any review of a tool response schema that exposes structured data through a non-`content` channel
  — flag it as a client-compatibility hazard.

## What carries over from the superseded learning

Several principles from
[`mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md`](mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md)
are still correct and still apply:

- **State both branches of conditional behaviour in the tool description.** A tool whose behaviour
  depends on environment state (filesystem, network, configuration) should document both paths in
  its `tools/list` description.
- **Use a tagged union (`kind` discriminator) for conditional outcomes.** The `commit_status` field
  in lore's write tools still uses `{ "kind": "not_committed" | "committed" | "pushed" }`. Adding a
  variant is mechanical because the Rust serialiser uses an exhaustive `match` with no wildcard arm
  — adding a new `CommitStatus` variant fails to compile until `commit_status_metadata()` is
  updated.
- **Pin the contract with tests.** The discipline of asserting on the structured field (not just the
  prose) still applies. Only the channel changed — the tests now extract from the fenced block
  instead of reading `result.metadata` directly, but they still exercise the same contract.

## Related

- [Expose MCP tool conditional outcomes as structured metadata](mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md)
  (superseded) — the original learning this one replaces. Still useful for the reasoning behind
  exposing structured data at all and the tagged-union discriminator pattern.
- [CLI data commands should output to stdout, not stderr](cli-data-commands-should-output-to-stdout-2026-04-02.md)
  — foundational convention at the CLI layer. The same principle applies: structured data for
  machines travels on a standardised channel every consumer knows to read.
- `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md` § "Design pivot: layer 2 finding" —
  the PR that produced this learning. Contains the full diagnostic walk-through from the initial
  confused agent report through the `lore serve | jq` wire-format check, the exploratory
  multi-content-block patch, and the production fix.
- PR #32 (`feat/coverage-check-skill`) — the PR where the pivot landed, across three commits: the
  Rust server change, the coverage-check skill update, and this learning with its companion
  supersession note.
