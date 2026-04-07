---
title: "Add MCP `reindex_file` tool so agents can re-index working-tree edits"
priority: P2
category: agent-native
status: ready
created: 2026-04-06
source: ce-review (feat/single-file-ingest)
files:
  - src/server.rs
  - src/ingest.rs:691-825
related_pr: feat/single-file-ingest
---

# Add MCP `reindex_file` tool so agents can re-index working-tree edits

## Context

The single-file ingest PR added `lore ingest --file <path>` as a CLI command and the library
function `ingest::ingest_single_file`. The plan explicitly scoped out MCP exposure with this
rationale:

> Not exposed as an MCP tool. The MCP write tools (`add_pattern`, `update_pattern`,
> `append_to_pattern`) already produce fresh-from-disk indexing as a side effect.

The ce-review agent-native reviewer verified that claim — it is literally true for the write path.
What the rationale misses: all three MCP write tools require the agent to **supply the full file
body** as an argument. They do not expose "re-index this file that already exists on disk, as-is."

Concrete scenario this leaves unreachable via MCP:

> A user edits `patterns/foo.md` in their editor, saves, then asks the agent in the same Claude Code
> session: "does this pattern surface for the query 'retry policy'?"

To answer, the agent would need to either:

1. Read the file with the Read tool, then call `update_pattern` with the full body it just read back
   verbatim — wasteful, loses frontmatter edge cases, and susceptible to round-trip corruption in
   `build_file_content` (trailing newlines, tag normalisation, YAML quoting).
2. Shell out to `Bash(lore ingest --file patterns/foo.md)` — works, but is an orphan-feature
   pattern: an action the human has via CLI and the agent has only through a generic escape hatch.

This scenario is the exact loop the Pattern QA skill described in ROADMAP.md is built around. If
that skill is agent-driven (and the ROADMAP language implies it is), the skill will either have to
`Bash(lore ingest --file …)` or round-trip file content through `update_pattern`. Neither is ideal.

## Proposed fix

Add a `reindex_file` MCP tool that wraps `ingest::ingest_single_file` directly. Thin handler, ~30
lines in `src/server.rs`, reusing the same write-lock and path-containment guards
`ingest_single_file` already enforces.

Tool schema:

```json
{
  "name": "reindex_file",
  "description": "Re-index a single markdown file that already exists on disk, without requiring a git commit or the full file body. Use for the edit → reindex → search loop when a user has just modified a pattern file.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "source_file": {
        "type": "string",
        "description": "Path relative to the knowledge directory, e.g. 'patterns/foo.md'"
      },
      "force_override_ignore": {
        "type": "boolean",
        "description": "If true, re-index even if the file is excluded by .loreignore. Default: false."
      }
    },
    "required": ["source_file"]
  }
}
```

Response shape mirrors `add_pattern` / `update_pattern` outcomes:

```json
{
  "file_path": "patterns/foo.md",
  "chunks_indexed": 4,
  "errors": []
}
```

On error (file not found, extension rejected, outside knowledge dir, excluded by `.loreignore`),
return a structured MCP error using the conditional-outcomes-as-metadata pattern from
`docs/solutions/best-practices/expose-mcp-conditional-outcomes-as-metadata-2026-04-06.md` so the
agent can branch on the failure reason.

Acquire the same write lock the other three write handlers use.

## Test surface

Add tests in `src/server.rs::tests`:

1. `reindex_file_indexes_existing_file` — create a file, call `reindex_file`, assert the response
   lists chunks and the DB contains the file.
2. `reindex_file_rejects_path_outside_knowledge_dir` — absolute path to a sibling dir, asserts
   structured error with `error_code: "outside_knowledge_dir"`.
3. `reindex_file_respects_loreignore` — file is `.loreignore`d, asserts structured error with
   `error_code: "ignored_by_loreignore"` and a hint that `force_override_ignore: true` bypasses it.
4. `reindex_file_force_override_indexes_ignored_file` — same setup, `force_override_ignore: true`,
   succeeds.
5. `reindex_file_does_not_touch_last_ingested_commit` — seed metadata, call, verify unchanged.

Also add an end-to-end test in `tests/` that exercises the tool through `start_mcp_server`'s stdio
handler.

## Trade-offs

- **Surface expansion.** Adds a sixth MCP tool. Every tool is a maintenance surface and a thing the
  LLM has to reason about. That said, this one is a direct primitive — it has a single input, a
  clear purpose, and composes with existing tools.
- **Overlap with `update_pattern`.** For agents that do want to supply content, `update_pattern`
  still exists and is the right tool. `reindex_file` is specifically for the "already-on-disk, do
  not touch the bytes" case.
- **Plan rationale must be updated.** The plan's "MCP write tools already re-index on write" line
  should be amended to note that `reindex_file` covers the remaining gap.
- **Write lock contention.** Same lock as the other three handlers — serialises correctly, but
  increases the surface that can trigger the "write in progress" error path tracked in
  `docs/todos/write-lock-busy-error-metadata.md`.

## When to do this

Defer until either:

- The Pattern QA skill lands and needs the primitive
- A user reports that agent-driven vocabulary coverage audits are painful to implement
- Any PR that extends the MCP server's tool surface, so the schema change rides along

## References

- ce-review run artifact:
  `.context/compound-engineering/ce-review/2026-04-06-single-file-ingest/summary.md`
- Plan: `docs/plans/2026-04-06-002-feat-single-file-ingest-plan.md` (Scope Boundaries)
- ROADMAP: "Pattern QA skill" entry
- `docs/solutions/best-practices/expose-mcp-conditional-outcomes-as-metadata-2026-04-06.md` —
  pattern for structured error metadata
- `docs/todos/write-lock-busy-error-metadata.md` — related agent-native gap in write handlers
- `src/ingest.rs:691-825` — `ingest_single_file` (the primitive to wrap)
- `src/server.rs` — existing MCP write handlers (`add_pattern`, `update_pattern`,
  `append_to_pattern`) to follow as pattern
