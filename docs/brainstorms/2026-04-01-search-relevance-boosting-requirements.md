---
date: 2026-04-01
topic: search-relevance-boosting
---

# Search Relevance Boosting

## Problem Frame

When searching the knowledge base, tag and title matches carry no more weight than incidental body
text matches. Searching for "typescript" returns Rust patterns because the word appears in their
body text, while patterns explicitly tagged `typescript` rank no higher. This affects both FTS and
vector search — FTS treats all columns equally (default BM25 weights), and vector embeddings are
generated from body text only, so tags and titles don't influence semantic similarity at all.

## Requirements

**FTS5 Column Weighting**

- R1. FTS5 search uses weighted BM25 scoring: title matches weighted highest, tag matches weighted
  high, body matches at baseline, source_file at zero
- R2. Weights are hardcoded (not user-configurable) — the ratio between columns is a product
  decision, not a tuning knob
- R3. The weighted BM25 is used in both standalone FTS search (`search_fts`) and as the FTS
  component of hybrid search

**Embedding Input Enrichment**

- R4. Embedding vectors at ingest time are generated from a composite string that includes title,
  tags, and body — not body alone
- R5. The query embedding at search time remains the user's raw query (no synthetic enrichment)
- R6. The composite string format is consistent across all ingest paths (bulk ingest, add_pattern,
  update_pattern, append_to_pattern)

**Existing Behavior Preserved**

- R7. The `min_relevance` threshold and Ollama fallback warning continue to work unchanged
- R8. All existing tests pass (some may need updated assertions if score values shift)

## Success Criteria

- Searching "typescript" returns patterns tagged `typescript` at top ranks, not Rust patterns that
  mention the word incidentally
- Searching for a pattern's exact title returns that pattern first
- Hybrid search benefits from both improvements — FTS ranks by weighted BM25, vector search is
  informed by title/tag content

## Scope Boundaries

- No score normalization (0–1 range) — deferred to a follow-up
- No new frontmatter fields — tags and title are sufficient for now
- No user-configurable column weights
- No changes to the chunking strategy or FTS5 schema
- No changes to the RRF algorithm or k parameter
- Re-ingesting after upgrade is expected and acceptable (no migration path needed)

## Key Decisions

- **Hardcoded weights over configurable:** The column weight ratios (title > tags > body >
  source_file) are a product-level ranking decision. Making them configurable adds editorial surface
  for something users shouldn't need to tune. If the defaults are wrong, we fix them — not expose a
  knob.
- **Composite embed string over separate embeddings:** A single embedding of
  `"{title} {tags} {body}"` is simpler than maintaining separate embeddings per field and merging
  them. The embedding model naturally handles the combined context.
- **Raw query over enriched query:** The user's search query should be embedded as-is. Synthetic
  enrichment (prepending guessed tags or context) would add complexity and unpredictability. The
  improvement comes from the indexed side — richer embeddings on the patterns — not the query side.
- **FTS weighting does not diminish semantic search:** RRF operates on ranks, not raw scores — a
  massive BM25 advantage from a title match only moves the result to rank 0, contributing the same
  `1/(k+1)` as any rank-0 result. When a synonym query misses the exact title ("conventions" vs
  "patterns"), vector search compensates by ranking it higher. Both inputs to RRF become more
  accurate (FTS via column weights, vector via enriched embeddings), which improves the fusion
  rather than making it lopsided.

## Dependencies / Assumptions

- FTS5 `bm25()` function accepts per-column weights as arguments — this is a documented FTS5
  feature, not a custom extension
- Changing embed input requires re-ingest (`lore init`), which rebuilds the entire database — this
  is acceptable for pre-release development
- `nomic-embed-text` handles the composite `"{title} {tags} {body}"` format well — embedding models
  generally handle concatenated text without special formatting

## Outstanding Questions

### Deferred to Planning

- [Affects R1][Technical] Exact weight values for title, tags, body columns — starting point is
  title=10.0, tags=5.0, body=1.0, source_file=0.0 but may need tuning based on test results
- [Affects R4][Technical] Exact format of the composite embed string — `"{title}\n{tags}\n{body}"`
  vs `"{title} {tags} {body}"` vs other separators. May affect embedding quality marginally
- [Affects R4][Technical] Whether `index_single_file` (used by write operations) needs the same
  change or only the bulk ingest path
- [Affects R8][Technical] Which existing tests need updated score assertions after the BM25 weight
  change

## Future Considerations

During integration-layer work (PreToolUse domain hooks, auto-invocable skills), we may want to
introduce additional frontmatter fields for machine-readable scoping (e.g.,
`applies_to:
[typescript, react]`, `domain: frontend`). Tags already serve this purpose to a degree,
but dedicated fields could enable more precise hook matching without polluting the tag namespace.
This should be evaluated during the integration brainstorm, not here.

Score normalization (mapping RRF scores to a 0–1 range) is planned as a separate follow-up after
this work lands, so that normalization operates on a scoring system that already ranks correctly.

## Next Steps

-> `/ce:plan` for structured implementation planning
