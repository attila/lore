---
title: "feat: Boost search relevance via FTS5 column weights and embedding enrichment"
type: feat
status: completed
date: 2026-04-01
origin: docs/brainstorms/2026-04-01-search-relevance-boosting-requirements.md
---

# feat: Boost search relevance via FTS5 column weights and embedding enrichment

## Overview

Improve search ranking so that title and tag matches dominate over incidental body text matches. Two
changes: (1) apply weighted BM25 scoring to FTS5 so title and tag columns rank higher, and (2)
enrich the embedding input at ingest time to include title and tags alongside body text, so vector
search also carries domain signal.

## Problem Frame

Searching for "typescript" returns Rust patterns because FTS5 treats all columns equally and vector
embeddings are generated from body text only. Tags and titles — the strongest relevance signals —
carry no weight in either search path. (see origin:
docs/brainstorms/2026-04-01-search-relevance-boosting-requirements.md)

## Requirements Trace

- R1. FTS5 uses weighted BM25: title highest, tags high, body baseline, source_file zero
- R2. Weights are hardcoded, not configurable
- R3. Weighted BM25 used in both standalone FTS and hybrid search
- R4. Embeddings generated from composite `"{title} {tags} {body}"` at ingest time
- R5. Query embedding remains the user's raw query
- R6. Composite string consistent across all ingest paths
- R7. `min_relevance` threshold and Ollama fallback warning unchanged
- R8. Existing tests pass (updated assertions where needed)

## Scope Boundaries

- No score normalization — deferred follow-up
- No new frontmatter fields
- No user-configurable weights
- No changes to chunking, FTS5 schema, RRF algorithm, or k parameter
- Re-ingesting after change is expected

## Context & Research

### Relevant Code and Patterns

- `src/database.rs` — FTS5 table:
  `patterns_fts USING fts5(title, body, tags, source_file,
  chunk_id UNINDEXED)`. Current query
  uses `rank AS score` with default BM25 weights (all 1.0). Column order is title, body, tags,
  source_file — weights map positionally to this order
- `src/database.rs` `search_fts` — the FTS query with `rank AS score` and `ORDER BY rank`
- `src/database.rs` `search_hybrid` — calls `search_fts` then `search_vector`, merges via RRF
- `src/ingest.rs:118` — bulk ingest: `embedder.embed(&chunk.body)` — has full `Chunk` with title and
  tags available
- `src/ingest.rs:387` — `index_single_file`: `embedder.embed(&chunk.body)` — same, full `Chunk`
  available
- `src/server.rs` `handle_search` — embeds raw query via `ctx.embedder.embed(query)` (unchanged)
- `src/main.rs` `cmd_search` — embeds raw query via `ollama.embed(query)` (unchanged)
- `src/chunking.rs` — `Chunk` struct has `title: String`, `tags: String`, `body: String`
- `tests/e2e.rs` — e2e lifecycle tests go through the full ingest pipeline, will automatically pick
  up embedding changes
- `tests/ollama_integration.rs` — Ollama integration tests use real embeddings through ingest

## Key Technical Decisions

- **BM25 weights: title=10.0, body=1.0, tags=5.0, source_file=0.0:** FTS5 `bm25()` takes positional
  weights matching column order (title, body, tags, source_file). Title at 10x body makes exact
  title matches dominate. Tags at 5x body makes domain-scoped matches outrank incidental mentions.
  source_file at 0 prevents path-based noise. These are starting values — tunable by changing the
  constants if evaluation shows different ratios work better. (see origin: Key Decisions —
  "Hardcoded weights over configurable")

- **Composite embed string with newline separators:** Format: `"{title}\n{tags}\n{body}"`. Newline
  separators help the embedding model distinguish sections. When tags are empty, this produces
  `"{title}\n\n{body}"` which is harmless — embedding models handle blank lines fine.

- **Helper function for embed text:** A small function `embed_text(chunk) -> String` that builds the
  composite string. Used at both ingest call sites to ensure R6 consistency. Placed in `ingest.rs`
  since that's where both call sites live.

## Open Questions

### Resolved During Planning

- **Exact weight values:** title=10.0, body=1.0, tags=5.0, source_file=0.0. Positional order matches
  FTS5 column definition order.

- **Composite string format:** `"{title}\n{tags}\n{body}"` with newline separators. Simple,
  embedding-model-friendly, no special formatting needed.

- **Does `index_single_file` need the change?** Yes — R6 requires all ingest paths to be consistent.
  Both `ingest()` (line 118) and `index_single_file()` (line 387) must use the composite string.

- **Which tests need updated assertions?** The FTS score values will change (different BM25 weights
  produce different rank values). Tests that assert on specific score values or compare scores may
  need updating. Tests that assert on result presence/absence or ordering should be unaffected.
  Server unit tests that manually insert chunks with `embed(&chunk.body)` are testing the search
  handler, not embedding content — they remain valid because FakeEmbedder produces consistent
  vectors for any input.

### Deferred to Implementation

- Exact score value changes in existing snapshot or assertion tests — depends on seeing the actual
  new values

## Implementation Units

- [x] **Unit 1: FTS5 weighted BM25 scoring**

  **Goal:** Replace default BM25 ranking with weighted column scoring so title and tag matches rank
  higher than body matches.

  **Requirements:** R1, R2, R3

  **Dependencies:** None

  **Files:**
  - Modify: `src/database.rs` (`search_fts`)

  **Approach:**
  - Replace `rank AS score` with `bm25(patterns_fts, 10.0, 1.0, 5.0, 0.0) AS score`
  - Replace `ORDER BY rank` with `ORDER BY score`
  - The `bm25()` function returns negative values (more negative = more relevant), same as `rank`,
    so ordering direction is unchanged
  - The weights are positional, matching FTS5 column order: title, body, tags, source_file

  **Patterns to follow:**
  - Existing `search_fts` query structure in `database.rs`

  **Test scenarios:**
  - Happy path: Insert two chunks — one with "typescript" in title/tags, one with "typescript" only
    in body. FTS search for "typescript" returns the tagged chunk first
  - Happy path: Insert a chunk with title "Error Handling". Search for "Error Handling" returns it
    at rank 0
  - Happy path: Existing `insert_and_fts_search` test still passes (result found, ordering may
    differ)
  - Edge case: Chunk with empty tags — search still works, body matches contribute normally

  **Verification:**
  - `cargo test -- database::tests` passes
  - FTS results demonstrate title/tag preference over body-only matches

- [x] **Unit 2: Embedding input enrichment**

  **Goal:** Generate embeddings from title + tags + body so vector search carries domain signal.

  **Requirements:** R4, R5, R6, R7, R8

  **Dependencies:** None (independent of Unit 1, but both needed for full improvement)

  **Files:**
  - Modify: `src/ingest.rs` (both embed call sites + add helper function)

  **Approach:**
  - Add a function `embed_text(chunk: &Chunk) -> String` that returns
    `format!("{}\n{}\n{}", chunk.title, chunk.tags, chunk.body)`
  - Replace `embedder.embed(&chunk.body)` with `embedder.embed(&embed_text(chunk))` at both call
    sites: `ingest()` (line 118) and `index_single_file()` (line 387)
  - Query embedding in `server.rs` and `main.rs` remains `embedder.embed(query)` (R5)
  - The `min_relevance` threshold logic is unaffected — it filters on RRF scores, not raw embeddings
    (R7)

  **Patterns to follow:**
  - Existing embed call pattern in `ingest.rs`
  - Existing helper function patterns in `ingest.rs` (e.g., `build_file_content`, `extract_title`)

  **Test scenarios:**
  - Happy path: `embed_text` with title "Error Handling", tags "rust, anyhow", body "Use anyhow"
    returns `"Error Handling\nrust, anyhow\nUse anyhow"`
  - Edge case: `embed_text` with empty tags returns `"Title\n\nBody"` (no crash, no special handling
    needed)
  - Integration: Full e2e lifecycle test (`tests/e2e.rs::full_lifecycle`) still passes — ingest uses
    the new composite string, search finds results
  - Integration: The `min_relevance` threshold test still passes (R7)

  **Verification:**
  - `just ci` passes
  - Both ingest paths use the same `embed_text` helper (R6 consistency)

## System-Wide Impact

- **Interaction graph:** Unit 1 changes `search_fts` which feeds into `search_hybrid`. All search
  entry points (MCP `handle_search`, CLI `cmd_search`) use `search_hybrid`, so both benefit. Unit 2
  changes the two embed call sites in `ingest.rs` — all ingest paths (bulk, add, update, append)
  flow through these.
- **Error propagation:** Unchanged. Embed errors are still captured by the existing error handling
  in both ingest paths.
- **Unchanged invariants:** FTS5 schema, RRF algorithm, `min_relevance` threshold, Ollama fallback
  warning, MCP tool schemas, write operations, `search_vector` query (unchanged — only the indexed
  vectors change), query embedding (remains raw user query).
- **API surface parity:** Both MCP and CLI search benefit from both changes.

## Risks & Dependencies

| Risk                                                       | Mitigation                                                                                                        |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| BM25 weight ratios may not be optimal                      | Starting values are reasoned (10/5/1); can be tuned by changing constants after evaluation                        |
| Composite embed string may degrade embedding quality       | Embedding models handle concatenated text well; newline separators help; verifiable with Ollama integration tests |
| Some test assertions may break due to changed score values | Expected and acceptable per R8; fix during implementation                                                         |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-04-01-search-relevance-boosting-requirements.md](../brainstorms/2026-04-01-search-relevance-boosting-requirements.md)
- Related code: `src/database.rs` (search_fts, search_hybrid), `src/ingest.rs` (embed call sites),
  `src/chunking.rs` (Chunk struct)
- SQLite FTS5 docs: `bm25()` function with per-column weights
