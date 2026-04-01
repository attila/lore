---
title: "fix: Improve search quality signals"
type: fix
status: completed
date: 2026-04-01
---

# fix: Improve search quality signals

## Overview

Two improvements to search result quality signaling: (1) warn the caller when Ollama is unreachable
and search falls back to FTS-only, and (2) filter results below a configurable relevance threshold
so nonsensical queries return "No matching patterns found" instead of noise.

## Problem Frame

When Ollama is stopped while `lore serve` is running, `search_patterns` silently falls back to
FTS-only search. The caller (agent or human) receives results with negative relevance scores and no
indication that semantic search was skipped. Separately, when searching for irrelevant terms, the
search returns low-scoring noise labeled "Found matching patterns" — misleading to an agent that
trusts tool output.

Both issues were discovered during MCP integration testing with Claude Code. See:

- `docs/findings/2026-04-01-search-silent-fallback-when-ollama-down.md`
- `docs/findings/2026-04-01-search-returns-noise-on-irrelevant-queries.md`

## Requirements Trace

- R1. When embedding fails during search, the MCP response includes a warning that results are
  text-match only
- R2. When embedding fails during CLI search, a warning is printed to stderr
- R3. Configurable `min_relevance` threshold in `[search]` config
- R4. Results below the threshold are filtered out before formatting
- R5. When all results are filtered, the response says "No matching patterns found"
- R6. The threshold applies only to hybrid RRF scores — FTS-only mode (by config or by fallback)
  skips filtering, since FTS scores are on a different scale (negative BM25 rank)
- R7. Existing behavior is preserved when `min_relevance` is 0.0

## Scope Boundaries

- No score normalization across search modes — each mode keeps its own scale
- No per-mode thresholds — single threshold that applies to hybrid RRF scores
- No changes to the search algorithms themselves (FTS, vector, RRF)
- No changes to ingest, write operations, or provisioning
- CLI and MCP paths both get the improvements

## Context & Research

### Relevant Code and Patterns

- `src/server.rs:335` — `ctx.embedder.embed(query).ok()` discards embed errors silently
- `src/server.rs:348-381` — result formatting loop and "Found matching patterns" / "No matching
  patterns found" summary logic
- `src/main.rs:272` — same `.ok()` pattern in CLI `cmd_search()`
- `src/database.rs:286-289` — `search_hybrid()` falls back to FTS when embedding is `None`
- `src/config.rs:29` — `SearchConfig { hybrid: bool, top_k: usize }` — no threshold field
- `src/embeddings.rs:147` — `FakeEmbedder` always succeeds, cannot test failure paths
- `src/server.rs:524` — `embedding_note()` helper exists for write ops but not for search
- `src/ingest.rs:50` — `WriteResult.embedding_failures` tracks embed errors asymmetrically

### Score Semantics

| Mode                   | Score range  | Direction                     | When used                      |
| ---------------------- | ------------ | ----------------------------- | ------------------------------ |
| FTS BM25 rank          | Negative     | More negative = more relevant | `hybrid = false` or fallback   |
| Vector cosine distance | 0.0–2.0      | Lower = more relevant         | Internal to hybrid             |
| RRF                    | ~0.001–0.033 | Higher = more relevant        | `hybrid = true` with embedding |

Observed during testing: irrelevant query noise scores ~0.016 (single-list rank 0 in FTS), good
matches score ~0.033 (rank 0 in both lists).

## Key Technical Decisions

- **Capture embed error, don't propagate it:** Search should still work when Ollama is down — the
  FTS fallback is valuable. Change `.ok()` to a `match` that preserves the error for the warning
  message but still passes `None` to `search_hybrid()`.

- **Warning in response text, not a JSON-RPC error:** The search succeeded (FTS results are valid).
  A JSON-RPC error would signal failure. A warning prefix in the response text is the right semantic
  — degraded but functional.

- **Threshold applies to hybrid RRF scores only:** FTS scores are negative BM25 rank values on a
  completely different scale. Applying a positive threshold to FTS results would filter everything.
  When operating in FTS-only mode (by config or by embedding failure), skip threshold filtering.
  This means: in degraded mode, the user gets the warning text and unfiltered FTS results — which is
  the right tradeoff (some signal is better than none when the system is already degraded).

- **FakeEmbedder failure mode via wrapper:** Rather than adding a `should_fail` flag to
  `FakeEmbedder` (which changes its API for all callers), add a `FailingEmbedder` newtype in the
  test module that wraps `FakeEmbedder` for dimensions but always errors on `embed()`. Minimal
  surface, single purpose.

## Open Questions

### Resolved During Planning

- **What default for `min_relevance`?** Use 0.02 as default. With k=60, max RRF score is ~0.033
  (rank 0 in both lists). A single-list-only result at rank 0 scores ~0.016. A threshold of 0.02
  requires a result to appear in both FTS and vector lists to survive, which is the right signal for
  "this is a real match" in hybrid mode. Observed: noise scored ~0.016, good matches ~0.033.

- **Should the threshold be separate for each mode?** No. The threshold only applies to hybrid RRF
  scores. FTS-only results (negative scores) bypass the filter entirely. This keeps config simple.

### Deferred to Implementation

- Exact warning text wording — should be concise and informative
- Whether `cmd_search` stderr warning should include the underlying error message or just the
  degraded-mode notice

## Implementation Units

- [x] **Unit 1: Config — add `min_relevance` to `SearchConfig`**

  **Goal:** Add a configurable relevance threshold to the search configuration.

  **Requirements:** R3, R7

  **Dependencies:** None

  **Files:**
  - Modify: `src/config.rs`

  **Approach:**
  - Add `pub min_relevance: f64` to `SearchConfig` with `#[serde(default)]`
  - Default value: 0.02 (set in a `Default` impl or `default_with`)
  - Update `Config::default_with` to include the new field

  **Patterns to follow:**
  - Existing `SearchConfig` fields (`hybrid`, `top_k`) and their defaults in `default_with`
  - Existing `#[serde(default)]` pattern on optional config sections

  **Test scenarios:**
  - Happy path: Config with `[search] min_relevance = 0.05` round-trips through save/load
  - Happy path: Config without `min_relevance` loads with default value 0.02
  - Happy path: Config with `min_relevance = 0.0` loads and represents "no filtering"
  - Edge case: Existing configs without `min_relevance` field still load (serde default)

  **Verification:**
  - `round_trip_save_and_load` passes with and without the new field
  - Existing tests unchanged

- [x] **Unit 2: Search — embed failure warning and relevance filtering**

  **Goal:** Surface embedding failures as warnings in search responses, and filter low-relevance
  results before formatting.

  **Requirements:** R1, R2, R4, R5, R6

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `src/server.rs` (`handle_search`)
  - Modify: `src/main.rs` (`cmd_search`)
  - Modify: `src/embeddings.rs` (test-only: add `FailingEmbedder`)

  **Approach:**

  _Embed failure warning (R1, R2):_
  - In `handle_search`: replace `ctx.embedder.embed(query).ok()` with a `match`. On `Err`, set a
    `embed_failed = true` flag and continue with `None` embedding
  - After formatting results, prepend a warning line to the response text when `embed_failed` is
    true
  - In `cmd_search`: same pattern, print warning to stderr via `eprintln!`
  - Add `FailingEmbedder` in `src/embeddings.rs` under
    `#[cfg(any(test, feature =
    "test-support"))]` — implements `Embedder` with `embed()` always
    returning `Err` and `dimensions()` delegating to an inner `FakeEmbedder`

  _Relevance filtering (R4, R5, R6):_
  - After getting results from `search_hybrid`/`search_fts`, determine if threshold applies:
    threshold applies when `config.search.hybrid` is true AND embedding succeeded (`!embed_failed`)
  - When threshold applies, filter results where `score < config.search.min_relevance`
  - Use existing "No matching patterns found" message when filtered list is empty
  - When threshold does not apply (FTS-only by config, or FTS fallback due to embed failure), return
    results unfiltered

  **Patterns to follow:**
  - `src/server.rs` — existing `handle_search` structure, `text_response` helper
  - `src/server.rs` — `TestHarness` for unit tests
  - `src/embeddings.rs` — `FakeEmbedder` gating pattern

  **Test scenarios:**

  _Embed warning (server):_
  - Happy path: Search with `FailingEmbedder` returns results AND response text contains warning
    about degraded mode
  - Happy path: Search with working `FakeEmbedder` returns results without warning text
  - Integration: Search with `FailingEmbedder` still returns FTS results (not an error response)

  _Relevance filtering (server):_
  - Happy path: Insert chunks with known scores, set `min_relevance` high enough to filter some —
    verify filtered results are excluded
  - Happy path: Set `min_relevance = 0.0` — all results returned (R7)
  - Happy path: All results below threshold — response says "No matching patterns found"
  - Edge case: FTS-only mode (`hybrid = false`) — threshold not applied, results returned regardless
    of score
  - Edge case: Embed failure + hybrid mode — threshold not applied (FTS fallback), warning shown

  _CLI (`cmd_search`):_
  - The CLI uses the same search logic; verify warning is printed to stderr when embedding fails
    (this can be tested via the Ollama integration tests or manually)

  **Verification:**
  - `just ci` passes
  - Server snapshot tests updated for new response formats
  - `FailingEmbedder` is excluded from release builds

## System-Wide Impact

- **Interaction graph:** Changes touch `handle_search` (MCP) and `cmd_search` (CLI) — the two search
  entry points. No other handlers, callbacks, or observers affected. Write operations (`handle_add`,
  `handle_update`, `handle_append`) are unchanged.
- **Error propagation:** Embed errors during search are captured but not propagated — they become a
  warning in the response text. This preserves the existing graceful degradation behavior.
- **Unchanged invariants:** Search algorithms (FTS, vector, RRF), database schema, ingest pipeline,
  MCP tool schemas (input parameters), and write operations are all unchanged. The MCP tool response
  text changes shape (warning prefix, possible "no matches" where previously there were always
  results), but this is the intended fix.
- **API surface parity:** Both MCP and CLI search paths get the same improvements.

## Risks & Dependencies

| Risk                                                          | Mitigation                                                                                       |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Default `min_relevance` too aggressive, filters valid results | Default 0.02 requires both-list presence; configurable; only applies to hybrid RRF               |
| Warning text confuses agents that parse response text         | Keep warning concise and on its own line; agents should handle gracefully                        |
| FTS-only results bypass threshold, still return noise         | Acceptable — FTS-only is already a degraded mode with a warning; some signal is better than none |

## Sources & References

- Findings: `docs/findings/2026-04-01-search-silent-fallback-when-ollama-down.md`
- Findings: `docs/findings/2026-04-01-search-returns-noise-on-irrelevant-queries.md`
- Related code: `src/server.rs` (handle_search), `src/main.rs` (cmd_search), `src/database.rs`
  (search_hybrid), `src/config.rs` (SearchConfig), `src/embeddings.rs` (Embedder trait)
