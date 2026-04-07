---
title: "Single-file ingest leaves partial state on embedding failure"
priority: P2
category: correctness
status: ready
created: 2026-04-06
source: ce-review (feat/single-file-ingest)
files:
  - src/ingest.rs:805-821
  - src/ingest.rs:1048-1080
related_pr: feat/single-file-ingest
---

# Single-file ingest leaves partial state on embedding failure

## Context

During ce-review of the single-file ingest PR, the correctness reviewer flagged a P2 inconsistency
between the "atomic" framing of single-file ingest and the actual behaviour on embedding failure.

`ingest_single_file` delegates to `index_single_file`, which on embedding failure still inserts the
chunk into the DB with `embedding: None`:

```rust
// src/ingest.rs:1068-1077
let embedding = if let Ok(emb) = embedder.embed(&embed_text(chunk)) {
    Some(emb)
} else {
    embedding_failures += 1;
    None
};
db.insert_chunk(chunk, embedding.as_deref())?;
```

`ingest_single_file` then reports `files_processed = 1` and pushes an error into `result.errors`:

```rust
// src/ingest.rs:805-815
match index_single_file(db, embedder, &canonical_dir, &canonical, strategy) {
    Ok((chunks, embedding_failures)) => {
        result.files_processed = 1;
        result.chunks_created = chunks;
        if embedding_failures > 0 {
            result.errors.push(format!(
                "{embedding_failures} embedding failure(s) while indexing {rel_path}"
            ));
        }
        // ...
    }
}
```

`cmd_ingest` sees non-empty `errors` and bails with exit 1 — but the chunks **have already been
written to the DB with null embeddings**. The file is now searchable by FTS5 (text search) but not
by vector similarity. This contradicts the "single-file ingest is atomic: any error means the one
requested file did not land" claim in the exit-code comment and in the plan.

The state is recoverable: the next invocation of `ingest_single_file` calls `delete_by_source`
before re-inserting, so a retry produces the correct state. But between the first failed call and
the retry, the DB is in an inconsistent state.

## Proposed fix

Two options, pick one:

### Option A — Roll back on embedding failure (strict atomic)

In `ingest_single_file`, if `embedding_failures > 0`, call `db.delete_by_source(&rel_path)` to undo
the partial insert, then set `files_processed = 0` and `chunks_created = 0`:

```rust
match index_single_file(db, embedder, &canonical_dir, &canonical, strategy) {
    Ok((chunks, embedding_failures)) if embedding_failures == 0 => {
        result.files_processed = 1;
        result.chunks_created = chunks;
    }
    Ok((_, embedding_failures)) => {
        // Partial success — roll back to preserve atomic semantics.
        let _ = db.delete_by_source(&rel_path);
        result.errors.push(format!(
            "{embedding_failures} embedding failure(s) while indexing {rel_path}; \
             rolled back to preserve atomicity"
        ));
    }
    Err(e) => {
        result.errors.push(format!("Failed to index {rel_path}: {e}"));
    }
}
```

This is the clean atomic story. Downside: walk-based ingest (`full_ingest`, `delta_ingest`) has the
opposite policy — it tolerates embedding failures because a partial index is better than no index,
and many small embed failures should not kill a whole ingest run. So this option creates a
behavioural divergence between single-file and walk-based paths.

### Option B — Downgrade embedding failures to warnings (permissive)

Don't push embedding failures into `result.errors`; only emit them through `on_progress` as
warnings. Keep `files_processed = 1` and `chunks_created = chunks`. `cmd_ingest` no longer bails.
The "Done" line honestly reports what landed, and the user sees a warning in the progress output
about missing embeddings.

This matches walk-based ingest behaviour. Downside: "atomic" is no longer quite true — a single-file
ingest that completed "successfully" may have chunks with null embeddings. The exit code is now 0
even when the vector-search side of the index is broken for that file.

### Option C — Hybrid (proposed)

- Embedding failures in single-file ingest are treated as errors (current behaviour), triggering
  exit 1 as today.
- `ingest_single_file` rolls back on embedding failure (Option A).
- Walk-based ingest keeps its tolerant behaviour unchanged.

The rationale: single-file ingest is always explicitly user-initiated against one specific file, so
"the one thing you asked for failed" is the right posture. Walk-based ingest may touch hundreds of
files and has legitimate reasons to tolerate per-file failures.

## Test surface

Add unit tests in `src/ingest.rs` using the `FailingEmbedder` at line 200 (or a new
`PartiallyFailingEmbedder` that fails on the second chunk but not the first):

1. `ingest_single_file_rolls_back_on_embedding_failure` — embedder fails on all chunks, assert
   `db.source_files()` does not contain the file AND the returned error mentions the rollback.
2. `ingest_single_file_rolls_back_on_partial_embedding_failure` — embedder fails on chunk 2 of 3,
   assert the DB has no chunks for the file (rolled back as a unit).
3. `full_ingest_tolerates_embedding_failure` (regression) — walk-based behaviour unchanged.

## Trade-offs

- **Behavioural divergence between single-file and walk-based paths.** Option C is the cleanest
  framing but requires explaining the split in the doc comment.
- **Rollback may itself fail.** If `delete_by_source` errors after the insert, the state is even
  worse than before. Need to consider whether to retry or just surface both errors.
- **Low-probability scenario.** `FakeEmbedder` and production `OllamaClient` both succeed in the
  common case. This only matters when Ollama is flaky, overloaded, or the embedding model is
  cold-loading. Still, the inconsistency is real and worth a principled fix.

## When to do this

Defer until either:

- A user reports their DB having files with missing embeddings that are searchable via FTS only
- Ollama reliability becomes a frequent pain point for pattern authoring
- Any PR that refactors `index_single_file` or touches chunk insertion

## References

- ce-review run artifact:
  `.context/compound-engineering/ce-review/2026-04-06-single-file-ingest/summary.md`
- Plan: `docs/plans/2026-04-06-002-feat-single-file-ingest-plan.md` (Key Technical Decisions —
  atomic error semantics)
- `src/ingest.rs:1048-1080` — `index_single_file` embed-or-insert loop
- `src/ingest.rs:200` — `FailingEmbedder` test double (reuse for tests)
