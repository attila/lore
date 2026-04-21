---
title: "Wrap `index_single_file` delete + inserts in a single transaction"
priority: P3
category: correctness
status: ready
created: 2026-04-21
source: ce-review (feat/universal-patterns)
files:
  - src/ingest.rs:1164-1204
  - src/database.rs:175-246
related_pr: feat/universal-patterns
---

# Wrap `index_single_file` delete + inserts in a single transaction

## Context

During ce-review of the universal-patterns PR, the data-integrity reviewer flagged that
`index_single_file` at `src/ingest.rs:1164-1204` performs:

1. `db.delete_by_source(&rel_path)` — own transaction, commits immediately
   (`src/database.rs:175-193`).
2. For each chunk: `db.insert_chunk(chunk, ...)` — **separate** transaction per chunk
   (`src/database.rs:200-246`).

A crash, panic, or process kill between step 1 and the Nth completion of step 2 leaves the DB with a
proper subset of the new chunks (0..N-1 present). The source is under-indexed until the next
invocation. A concurrent reader during the loop sees a partial state.

For the universal-patterns PR specifically: the `is_universal` flag does not mix within the
surviving subset — all surviving rows from one file agree, because the chunker sets the flag once
per file and every new insert uses the fresh value. So there is no flag-inconsistency bug. The issue
is pre-existing and orthogonal to universal patterns, just made more visible because users now run
`lore ingest --force` more often after schema changes.

## Proposed fix

Move embedding calls out of the DB-write path (they involve network I/O and should never hold a
write transaction), then wrap the DELETE + all INSERTs in a single outer transaction.

```rust
// 1. Compute embeddings outside the transaction (network I/O, potentially slow).
let embeddings: Vec<Option<Vec<f32>>> = chunks
    .iter()
    .map(|chunk| embedder.embed(&embed_text(chunk)).ok())
    .collect();

// 2. Open one transaction for all DB writes.
let tx = db.begin_write_tx()?;
delete_by_source_in_tx(&tx, &rel_path)?;
for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
    insert_chunk_in_tx(&tx, chunk, embedding.as_deref())?;
}
tx.commit()?;
```

This requires exposing transaction-scoped variants of `delete_by_source` and `insert_chunk`. Both
currently use `self.conn.unchecked_transaction()` internally (`database.rs:176, 201`) — the refactor
extracts the body into a `&Transaction`-accepting function and keeps the public `self.conn`-variant
as a thin wrapper for call sites that don't need outer-txn semantics.

## Alternative: savepoints

Rusqlite's `unchecked_transaction` supports nesting via SAVEPOINT when `sqlite_sequence` is enabled.
Simpler to write but harder to reason about on failure — the middle savepoint's `ROLLBACK TO`
semantics can leak partial state into the outer transaction if not handled. Prefer the explicit
`&Transaction` threading.

## Test surface

Add tests in `tests/single_file_ingest.rs`:

1. `index_single_file_rolls_back_delete_on_insert_failure` — use a chunker that produces a chunk
   whose body violates a CHECK constraint (e.g., an invalid is_universal value injected via test
   harness), confirm the previous chunks from the same source are still present after the failed
   insert.
2. `index_single_file_concurrent_reader_sees_old_or_new_state_not_partial` — spawn a reader thread
   that issues `search_patterns` while `index_single_file` runs; verify every snapshot shows either
   all old chunks or all new chunks, never a mix.

The second test is flaky by nature; accept the brittleness or skip it in CI and run locally when
touching this path.

## Trade-offs

- **Transaction held across potentially-slow ops.** Moving embeddings outside the transaction keeps
  the lock window bounded to pure-DB work (microseconds). Good.
- **Signature churn in `KnowledgeDB`.** Each write method gets a `*_in_tx` sibling. Manageable —
  three methods affected (`delete_by_source`, `insert_chunk`, `clear_all`). Public API stable.
- **Behaviour change on partial failure.** Today, a mid-loop crash leaves under-indexed state. After
  the fix, a mid-loop crash leaves the _pre-delete_ state intact. The second is strictly better for
  correctness; tag-removal semantics change only at the file-atomicity level (not flag level).
- **Doesn't fix the `full_ingest` atomicity gap.** That gap (pre-existing, documented in
  `docs/plans/2026-04-20-001-feat-universal-patterns-plan.md` Dependencies & Risks) is a separate
  follow-up: `full_ingest` needs backup-and-swap of the whole DB file around `clear_all`. That is a
  different fix.

## When to do this

Pick up when:

- A user reports a pattern going missing after a `lore ingest` crash or OS reboot mid-ingest.
- Any PR refactors `index_single_file` or `delete_by_source` for another reason.
- The `full_ingest` backup-and-swap follow-up lands — aligns both paths on transaction-scoped
  writes.

## References

- ce-review synthesis (2026-04-21) — data-integrity reviewer flagged as non-blocking P3.
- `src/ingest.rs:1164-1204` — `index_single_file` loop.
- `src/database.rs:175-193` — `delete_by_source` (own transaction).
- `src/database.rs:200-246` — `insert_chunk` (own transaction per chunk).
- Plan: `docs/plans/2026-04-20-001-feat-universal-patterns-plan.md` Dependencies & Risks section
  mentions the related `full_ingest` atomicity gap.
