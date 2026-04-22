---
title: "Out-of-band writers bypass the delta-ingest checkpoint"
date: 2026-04-22
category: best-practices
module: ingest
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Adding a new write path (MCP tool, background worker, hot-reload hook) to a system that also runs a delta pipeline gated on a commit/generation checkpoint"
  - "Reviewing why 'already up to date — no files changed' skips apparent work"
  - "Designing an integration test that interleaves out-of-band writes with delta runs"
tags:
  - ingest
  - delta
  - reconciliation
  - mcp
  - checkpoint
  - design-pattern
related:
  - docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md
  - docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md
---

# Out-of-band writers bypass the delta-ingest checkpoint

## Context

`lore`'s delta ingest gates on a single stored checkpoint — `last_ingested_commit` — and works out
which files to process by diffing that commit against the current `HEAD`. The path works cleanly
when every write to the knowledge base goes through ingest itself: each ingest run bumps the
checkpoint, the next run sees only truly new changes.

The MCP write tools (`add_pattern`, `update_pattern`, `append_to_pattern`) sit outside that loop.
They each write markdown to the patterns directory, commit it to git, and re-index the touched file
via `ingest_single_file` — which updates the `patterns` + `chunks` tables but **does not** touch
`last_ingested_commit`. That omission is invisible most of the time because the tool's own
single-file ingest keeps the DB consistent with what was just written. It becomes a problem the
moment a subsequent git operation cancels the MCP-authored sequence out.

Reproduced during PR #34 smoke testing:

1. `last_ingested_commit` was `995e944` from the last `lore ingest --force` run.
2. `add_pattern(smoke-test-db-read-surface.md, …)` — writes file, commits, indexes. Checkpoint still
   `995e944`.
3. `update_pattern(same file, …)` — overwrites file, commits, re-indexes. Checkpoint still
   `995e944`.
4. `append_to_pattern(same file, …)` — appends section, commits, re-indexes. Checkpoint still
   `995e944`.
5. Manual `git rm smoke-test-db-read-surface.md && git commit`.
6. `lore ingest` (delta): compares `995e944..HEAD` — the smoke-test file was added _and_ deleted
   inside this window, so its net git diff is empty. Delta sees "no files changed", skips, and the
   `patterns` + `chunks` rows written in steps 2-4 stay behind as orphans.

Symptom: `list_patterns` continues to return the pattern ("Smoke Test DB Read Surface") even though
the source markdown no longer exists on disk and is not in HEAD. Only `lore ingest
--force` clears
it.

## Root cause

Two design choices compose into the gap:

- **Checkpoint scoping.** `last_ingested_commit` tracks the last _full or delta_ ingest. MCP
  single-file writes intentionally don't bump it because they're not committing to having seen every
  change in the repo — they've only indexed one file.
- **Delta-ingest gate.** The delta entrypoint short-circuits when
  `git log last_ingested_commit..HEAD` is empty, because its mental model is "delta is driven by git
  diff".

When both are true, any file whose full lifecycle (create, mutate, delete) happens between two
`ingest` invocations leaves orphans in the DB that the next `ingest` will never notice. Git-diff
cannot see files that no longer exist at either endpoint.

This is a specific manifestation of the broader principle captured in
[`filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`](filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md)
— delta pipelines that gate on _diffs_ (filter changes, git changes) miss anything outside the diff
frame. That earlier doc concluded bidirectional reconciliation should be the default for filter
changes. The same conclusion applies to commit-range-driven deltas whenever an out-of-band writer
exists.

## Guidance

When a system has a delta-ingest path gated on a checkpoint _and_ an out-of-band writer that
bypasses the checkpoint, choose one:

1. **Make the out-of-band writer feed the checkpoint.** After an MCP single-file write commits, bump
   `last_ingested_commit` to the resulting commit SHA. The next delta run sees no missed work in
   that file's lifecycle because its additions have already been accounted for by the checkpoint.
   Deletions still reconcile normally via the git diff between the new checkpoint and the subsequent
   HEAD. Cheapest change; correct by construction.

2. **Make delta ingest reconcile against the source of truth on every run.** After processing the
   git-diff changes, compare `db.source_files()` against a filesystem walk. Any entry in the DB that
   doesn't exist on disk gets deleted; any file on disk missing from the DB gets indexed. This is
   the `.loreignore` reconciliation pass, generalised. More code, but also catches out-of-band
   mutations that option (1) alone wouldn't (someone editing `knowledge.db` with `sqlite3`,
   restoring a stale DB backup, etc.).

3. **Document the limitation and rely on `--force` as the escape valve.** Cheapest implementation,
   worst UX. Users hit the same confusion the `.loreignore` v1 users hit. Not recommended for new
   designs.

Prefer (1) when the writer set is bounded and well-known (the three MCP write tools). Escalate to
(2) if the project has or will gain unattributed writers, or if diagnostics matter.

## Prevention / test

For any delta pipeline with a checkpoint and at least one out-of-band writer, add an integration
test along these lines:

1. Run `full_ingest` — checkpoint set.
2. Via the out-of-band writer, create a file.
3. Via git, delete that same file (or use the writer's delete path if it has one).
4. Run delta ingest.
5. Assert the DB no longer has rows for the deleted file.

The test captures the "net-zero git diff across the checkpoint window" case by construction. `lore`
will pick this up as part of the fix for the MCP-writer-vs-delta gap when it's implemented; filing
it now means the fix can't ship without the test passing.

## When this doc applies vs `delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`

The 2026-04-05 doc is about the inverse surprise: committed-but-not-indexed changes being the _thing
you want_ (because delta ingest requires them). This doc is about committed-and-indexed changes that
later _cancel out_ from the delta frame's perspective. Both sit on the same gate, approached from
opposite ends.
