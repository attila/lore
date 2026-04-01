---
title: "feat: Normalize RRF scores to 0–1 range"
type: feat
status: completed
date: 2026-04-01
---

# feat: Normalize RRF scores to 0–1 range

## Overview

Normalize hybrid search RRF scores to a 0–1 range so that relevance thresholds and displayed scores
are intuitive. Update the `min_relevance` default to match the new scale. FTS-only scores (negative
BM25) are left as-is — FTS-only is a degraded mode that already carries a warning.

## Problem Frame

RRF scores currently range from ~0.001 to ~0.033. A threshold of 0.02 and displayed relevance of
0.0325 are opaque — users and agents can't intuit what these numbers mean. Normalizing to 0–1 makes
the scores self-explanatory: 1.0 = perfect match (rank 0 in both lists), 0.5 = single-list match,
0.0 = no match.

## Requirements Trace

- R1. RRF scores are normalized to 0–1 by dividing by the maximum possible score (`2/(k+1)`)
- R2. The `min_relevance` default is updated to the 0–1 equivalent of the current 0.02 threshold
- R3. Displayed `relevance:` values in MCP and CLI responses reflect the normalized scores
- R4. FTS-only scores (negative BM25 rank) are unchanged — no normalization applied
- R5. Existing filtering logic works unchanged (threshold comparison, FTS bypass)

## Scope Boundaries

- FTS-only scores are not normalized (different scale, rare degraded mode)
- No changes to the RRF algorithm, k parameter, or column weights
- No changes to search behavior — only how scores are represented

## Key Technical Decisions

- **Normalize in RRF function, not in display:** Dividing by max at the source
  (`reciprocal_rank_fusion`) means all downstream consumers (threshold filtering, response
  formatting, CLI display) get normalized scores without changes. The alternative — normalizing at
  each display point — would be fragile and repetitive.

- **FTS-only scores left as-is:** FTS-only mode is degraded (Ollama down or `hybrid = false`). BM25
  rank has no fixed max, so meaningful normalization would require relative scaling within the
  result set, which is misleading. The existing warning already tells the caller results are
  text-match only.

- **`min_relevance` default: 0.6:** Current 0.02 on the raw scale corresponds to roughly 0.6 on the
  normalized scale (`0.02 / 0.0328 ≈ 0.61`). A default of 0.6 preserves the same filtering behavior:
  results must appear in both FTS and vector lists to survive.

## Open Questions

### Resolved During Planning

- **Where to normalize:** In `reciprocal_rank_fusion` where `r.score = s` is assigned. Change to
  `r.score = s / max_rrf` where `max_rrf = 2.0 / (k + 1.0)`.

- **New default threshold:** 0.6 — mathematically equivalent to the current 0.02 on raw scores.

### Deferred to Implementation

- Whether the `rrf_merges_two_ranked_lists` test needs updated score assertions — depends on whether
  it asserts exact score values

## Implementation Units

- [x] **Unit 1: Normalize RRF scores and update threshold**

  **Goal:** Normalize RRF output to 0–1 and update the default `min_relevance` to match.

  **Requirements:** R1, R2, R3, R4, R5

  **Dependencies:** None

  **Files:**
  - Modify: `src/database.rs` (`reciprocal_rank_fusion`)
  - Modify: `src/config.rs` (`default_min_relevance`, `SearchConfig`)

  **Approach:**
  - In `reciprocal_rank_fusion`: compute `max_rrf = 2.0 / (k + 1.0)` and normalize each score with
    `r.score = s / max_rrf` instead of `r.score = s`
  - In `config.rs`: change `default_min_relevance` from 0.02 to 0.6
  - No changes to `server.rs`, `main.rs`, or display formatting — they already use `r.score` and the
    threshold from config

  **Patterns to follow:**
  - Existing `reciprocal_rank_fusion` function in `database.rs`
  - Existing `default_min_relevance` function in `config.rs`

  **Test scenarios:**
  - Happy path: RRF with a result at rank 0 in both lists produces score ~1.0
  - Happy path: RRF with a result at rank 0 in one list only produces score ~0.5
  - Happy path: Config default `min_relevance` is 0.6
  - Happy path: Config round-trip with `min_relevance = 0.8` works
  - Integration: `search_filters_low_relevance_results` test still filters noise (threshold now 0.6,
    scores now 0–1, same filtering behavior)
  - Integration: `search_with_zero_threshold_returns_all` still returns all results

  **Verification:**
  - `just ci` passes
  - RRF scores in test output are in 0–1 range
  - Filtering behavior unchanged (same results pass/fail threshold as before)

## System-Wide Impact

- **Interaction graph:** Only `reciprocal_rank_fusion` and `default_min_relevance` change. All
  downstream consumers (handle_search, cmd_search, threshold filtering) work unchanged because they
  already operate on `score` and `min_relevance` — only the scale changes, and both change together.
- **Unchanged invariants:** FTS-only scores, search algorithms, column weights, embedding
  enrichment, Ollama fallback warning, MCP tool schemas, write operations.

## Risks & Dependencies

| Risk                                          | Mitigation                                                       |
| --------------------------------------------- | ---------------------------------------------------------------- |
| Existing configs with `min_relevance = 0.02`  | Pre-release, no external users; re-running `lore init` resets it |
| Test assertions on exact RRF scores may break | Expected; update assertions to match normalized values           |

## Sources & References

- Related code: `src/database.rs` (reciprocal_rank_fusion), `src/config.rs` (default_min_relevance)
- Prior work: `docs/plans/2026-04-01-002-fix-search-quality-signals-plan.md` (introduced
  min_relevance)
