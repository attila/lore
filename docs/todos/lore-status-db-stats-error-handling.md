---
title: "lore_status silently maps db.stats() errors to null"
priority: P2
category: agent-native
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/server.rs:670-685
related_pr: feat/git-optional-knowledge-base
---

# lore_status silently maps db.stats() errors to null

## Context

`handle_lore_status` in `src/server.rs` calls `ctx.db.stats().ok()` when building the response
metadata. When stats retrieval fails (locked database, disk error, schema corruption), the
`chunks_indexed` and `sources_indexed` fields become `null` and the human-readable summary renders
them as `?`. This is indistinguishable from a successful read against an empty database.

An agent reading the metadata cannot tell:

- "Knowledge base is empty (chunks: 0, sources: 0)" — normal state after init
- "Knowledge base read failed (chunks: null, sources: null)" — needs intervention

Both states currently surface as a successful tool response with similar content. Three reviewers
flagged this independently (adversarial 0.82, correctness testing gap 0.65, api-contract residual
risk).

## Proposed fix

Pick one of:

1. **Return an MCP error response on `db.stats()` failure.** Most explicit. Trade-off: agents lose
   access to other lore_status fields (git_repository, inbox_workflow_configured) when only one
   piece of state is broken.

2. **Add a `database_ok: bool` field to the metadata.** Lets agents detect the degraded state
   without losing the rest of the response. Recommended.

3. **Distinguish via a `database_error: string | null` field** that captures the underlying error
   message. Most diagnostic but more surface area.

If option 2 is chosen, also update the human-readable summary to say "database: error" rather than
"?" so terminal users see the same signal.

## Test surface

Add `lore_status_reports_database_error_when_stats_fail` in `src/server.rs::tests`. Hardest part is
getting `db.stats()` to fail in a test — easiest is to drop the `chunks` table from the in-memory DB
after init, then call lore_status.

## References

- Adversarial finding: response can mask failure modes
- Correctness testing gap: handle_lore_status with database errors
- API-contract residual risk: lore_status null semantics
