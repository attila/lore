---
title: "MCP single-file writes should bump last_ingested_commit"
priority: P2
category: correctness
status: ready
created: 2026-04-22
source: PR #34 smoke test (db-sole-read-surface)
files:
  - src/ingest.rs (ingest_single_file, add_pattern, update_pattern, append_to_pattern)
  - src/ingest.rs (META_LAST_COMMIT constant + its existing writers)
related_pr: feat/db-sole-read-surface
related_learning: docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md
---

# MCP single-file writes should bump last_ingested_commit

## Context

`add_pattern`, `update_pattern`, and `append_to_pattern` commit the written markdown to git and
re-index the affected file via `ingest_single_file`. Neither path currently updates
`META_LAST_COMMIT` in `ingest_metadata`. Only the bigger ingest paths (`full_ingest`,
`delta_ingest`) do.

The gap becomes visible when an MCP-authored file's full lifecycle happens between two `ingest`
invocations and the net git diff over `last_ingested_commit..HEAD` is empty — delta-ingest sees "no
files changed" and the orphan `patterns` + `chunks` rows stay behind. Reproduced during PR #34 smoke
testing; full writeup at
`docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md`.

## Proposed fix

After the git commit inside each of the three MCP write tools, capture the resulting HEAD SHA and
persist it:

```rust
// after try_commit succeeds and returns CommitStatus::Committed
if let CommitStatus::Committed = commit_status
    && let Ok(head) = git::head_commit(knowledge_dir)
    && let Err(e) = db.set_metadata(META_LAST_COMMIT, &head)
{
    // Match the warning shape the other ingest paths use; don't fail the tool.
    eprintln!("Warning: failed to update last_ingested_commit: {e}");
}
```

This is the lightest possible fix. It doesn't address DB-tamper scenarios (someone editing
`knowledge.db` with `sqlite3` directly, restoring a stale backup, etc.) — those would need the
"delta reconciles against the filesystem walk" option from the learning doc. Start with the cheap
fix; escalate only if a real case arises.

## Acceptance test

Integration test in `tests/single_file_ingest.rs`:

1. `full_ingest --force` on a fresh repo with one committed pattern.
2. Record `last_ingested_commit` (should match HEAD).
3. Call `add_pattern` for a new file.
4. Assert `last_ingested_commit` now equals the new HEAD (not the pre-add one).
5. `git rm` the new file + commit.
6. Run `lore ingest` (delta). Assert `files_processed == 1` (the deletion) and the `patterns` row
   for the deleted file is gone.

Without the fix, step 4 fails. With it, step 6 reconciles correctly because the delta window starts
from the post-add checkpoint.

Also add a regression test for `update_pattern` and `append_to_pattern` that writes a file, makes an
unrelated commit elsewhere, then runs delta ingest and confirms no spurious re-indexing of the
MCP-touched file (checkpoint should have moved past it).

## Out of scope

- **Bidirectional filesystem reconciliation on every delta run.** Option (2) in the learning doc.
  More invasive; escalate if needed later.
- **Delete semantics via an `mcp_delete_pattern` tool.** Today there is no MCP delete. If one lands,
  it should update `META_LAST_COMMIT` too after its git commit.
- **DB-tamper recovery.** Users who edit `knowledge.db` with external tools are on their own;
  `lore ingest --force` is the escape valve.

## When to do this

- Next time the ingest module is touched for any reason.
- Before any serious use of MCP write tools in automated flows where rebuild cost matters.
- Not urgent enough to block an unrelated release.
