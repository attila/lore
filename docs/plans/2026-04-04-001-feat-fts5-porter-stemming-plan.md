---
title: "feat: Enable FTS5 porter stemming for improved search recall"
type: feat
status: active
date: 2026-04-04
origin: docs/brainstorms/2026-04-04-fts5-porter-stemming-requirements.md
---

# feat: Enable FTS5 porter stemming for improved search recall

## Overview

Add porter stemming to the FTS5 virtual table so morphological variants match automatically
("fake"→"fakes", "test"→"testing"). Includes auto-migration for existing databases via the
`ingest_metadata` table.

## Problem Frame

Hook-based search misses patterns when query terms are morphological variants of indexed terms.
FTS5's default unicode61 tokenizer requires exact token matches. This was identified during
dogfooding — see origin document and `docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md`
(Bug 2).

## Requirements Trace

- R1. FTS5 virtual table uses porter stemming tokenizer
- R2. Porter wraps unicode61 (preserving current tokenization behavior)
- R3. Existing databases auto-migrate on next `init()` — FTS table recreated and repopulated from
  `chunks`
- R4. Fresh databases get the new tokenizer immediately
- R5. Stemmed queries improve recall for morphological variants
- R6. Existing structured FTS5 queries (AND, OR, parentheses) continue to work
- R7. No regression in exact-match search relevance

## Scope Boundaries

- No custom tokenizer — use FTS5's built-in `porter` wrapping `unicode61`
- No changes to query construction in `src/hook.rs` — porter is transparent
- No changes to vector search or RRF scoring — FTS5 leg only
- Pattern authoring guide is a separate roadmap item

## Context & Research

### Relevant Code and Patterns

- `src/database.rs:109-111`: Current FTS5 table definition (no `tokenize` clause)
- `src/database.rs:136`: `ingest_metadata` table — already used for `last_commit` tracking in delta
  ingest
- `src/database.rs:410-424`: `get_metadata`/`set_metadata` — existing key-value API for schema
  versioning
- `src/database.rs:234`: `search_fts` with BM25 column weights — unaffected by tokenizer change
- `src/database.rs:433`: `sanitize_fts_query` — preserves AND/OR/parentheses, unaffected by stemming
- `tests/search_relevance.rs`: 12 existing search relevance regression tests

### Institutional Learnings

- `docs/solutions/database-issues/fts5-query-sanitization-crashes-on-special-chars-2026-04-02.md` —
  sanitization is separate from tokenization, no interaction
- `docs/solutions/database-issues/fts5-query-construction-for-hook-based-search-2026-04-02.md` —
  query construction (language anchors, OR enrichment) is token-level, will benefit from stemming
  (query "test" now also matches "testing" in patterns)

## Key Technical Decisions

- **Metadata-based migration detection**: Store `fts_tokenizer=porter` in `ingest_metadata`. On
  `init()`, check if the key exists and matches. If not (missing or different), drop and recreate
  the FTS table with the new tokenizer config, then repopulate from `chunks`. This reuses the
  existing metadata infrastructure (see origin: R3).

- **Repopulate from chunks, not re-ingest**: The `chunks` table already has all indexed content.
  Repopulating the FTS table from `chunks` is a fast SQL operation — no Ollama calls, no filesystem
  reads. This makes migration instant even for large knowledge bases.

- **Single tokenizer string**: `tokenize = 'porter unicode61'` — porter wraps unicode61 as a
  pipeline. This is FTS5's standard composition syntax.

## Open Questions

### Resolved During Planning

- **How to detect tokenizer mismatch?** Use `ingest_metadata` key `fts_tokenizer`. Check on `init()`
  — if absent or mismatched, drop and recreate. This avoids PRAGMA introspection complexity and
  reuses an existing pattern.

- **Does porter interact with BM25 column weights?** No. Porter affects tokenization (which tokens
  match), not scoring (how matches are ranked). The `bm25()` function weights are positional and
  independent of the tokenizer.

### Deferred to Implementation

- Exact repopulation SQL — whether to use `INSERT INTO ... SELECT` from chunks or iterate in Rust.
  Both work; implementation will choose the simplest.

## Implementation Units

- [ ] **Unit 1: Add porter tokenizer and auto-migration**

  **Goal:** Change the FTS5 table to use porter stemming and auto-migrate existing databases.

  **Requirements:** R1, R2, R3, R4

  **Dependencies:** None

  **Files:**
  - Modify: `src/database.rs`
  - Test: `src/database.rs` (inline unit tests)

  **Approach:**
  - Change the FTS5 CREATE statement to include `tokenize = 'porter unicode61'`
  - In `init()`, after creating `ingest_metadata`, check `get_metadata("fts_tokenizer")`:
    - If it returns `Some("porter")` → skip (already migrated)
    - Otherwise → wrap the entire migration in a single transaction: drop `patterns_fts`, recreate
      with new tokenizer, repopulate from `chunks`, set metadata. If any step fails, the transaction
      rolls back and the old FTS table remains intact — searches continue working with the old
      tokenizer rather than returning zero results.
  - Repopulation: INSERT INTO the new FTS table by selecting from `chunks`

  **Patterns to follow:**
  - Existing `get_metadata`/`set_metadata` usage for `last_commit` in delta ingest (`src/ingest.rs`)
  - Existing `clear_all` pattern for transactional multi-table operations

  **Test scenarios:**
  - Happy path: fresh database → `init()` creates FTS table with porter tokenizer and sets metadata
    key
  - Happy path: existing database without metadata key → `init()` recreates FTS table and
    repopulates from chunks
  - Happy path: existing database with `fts_tokenizer=porter` → `init()` skips migration
    (idempotent)
  - Integration: insert chunks, run migration, verify FTS search still returns results
  - Edge case: empty chunks table → migration completes without error

  **Verification:**
  - `init()` is idempotent — running it twice produces the same result
  - FTS table uses porter tokenizer (verified by stemming behavior in Unit 2 tests)

- [ ] **Unit 2: Add stemming search relevance tests**

  **Goal:** Verify that porter stemming improves recall for morphological variants and doesn't
  regress existing queries.

  **Requirements:** R5, R6, R7

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `tests/search_relevance.rs`

  **Approach:**
  - Add test cases that query with morphological variants of indexed terms
  - Verify existing 12 tests still pass (no regression)
  - Use the existing `seed_patterns` + `FakeEmbedder` + `hybrid = false` pattern for deterministic
    FTS-only results

  **Patterns to follow:**
  - Existing test structure in `tests/search_relevance.rs` — `seed_patterns`, `setup_test_env`,
    assert on result titles

  **Test scenarios:**
  - Happy path: query "testing" matches pattern containing "test" (and vice versa)
  - Happy path: query "fakes" matches pattern containing "fake"
  - Happy path: query "configured" matches pattern containing "configuration"
  - Happy path: structured query "rust AND testing" still works with stemming
  - Integration: all 12 existing search relevance tests pass unchanged

  **Verification:**
  - New stemming tests pass
  - All existing search relevance tests pass without modification
  - `just ci` passes

## System-Wide Impact

- **Interaction graph:** Only the FTS5 table definition changes. All consumers (`search_fts`,
  `search_hybrid`, hook pipeline, CLI search) go through the same query path and benefit
  automatically.
- **Error propagation:** Migration errors in `init()` propagate via `?` as usual — same error
  handling contract as existing table creation.
- **State lifecycle risks:** The migration (drop, recreate, repopulate, set metadata) runs in a
  single transaction. If interrupted or any step fails, the transaction rolls back and the old FTS
  table remains intact — searches continue working with the unstemmed tokenizer. No window of zero
  results.
- **Unchanged invariants:** `sanitize_fts_query` behavior unchanged. Hook query construction
  unchanged. Vector search unchanged. RRF scoring unchanged. BM25 column weights unchanged.

## Risks & Dependencies

| Risk                                                          | Mitigation                                                                                                            |
| ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Porter stemming over-stems (unrelated words map to same root) | Low risk for technical vocabulary. Porter is the standard choice. Monitor search relevance tests for false positives. |
| Migration on large databases is slow                          | Repopulating FTS from chunks is a single INSERT-SELECT — fast even for thousands of chunks. No Ollama calls.          |

## Sources & References

- **Origin document:** `docs/brainstorms/2026-04-04-fts5-porter-stemming-requirements.md`
- Related: `docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md` (Bug 2)
- Related: ROADMAP.md (FTS5 porter stemming item)
- FTS5 tokenizer docs: https://www.sqlite.org/fts5.html#tokenizers
