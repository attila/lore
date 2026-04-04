---
date: 2026-04-04
topic: fts5-porter-stemming
---

# FTS5 Porter Stemming

## Problem Frame

Hook-based search has recall gaps for naturally-worded queries. FTS5's default unicode61 tokenizer
requires exact token matches, so "fake" doesn't match "fakes", "test" doesn't match "testing", and
"configure" doesn't match "configuration". This was identified during dogfooding (see
`docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md`, Bug 2) and is a roadmap item.

Porter stemming reduces tokens to their root form before indexing and querying, so morphological
variants match automatically.

## Requirements

**Tokenizer**

- R1. The FTS5 virtual table (`patterns_fts`) uses the porter stemming tokenizer
- R2. Porter wraps the existing unicode61 tokenizer (preserving current tokenization behavior for
  punctuation, case folding, etc.)

**Migration**

- R3. Existing databases automatically get the new tokenizer on next `init()` without manual
  intervention — the FTS table is recreated and repopulated from the `chunks` table
- R4. Fresh databases get the new tokenizer immediately

**Search Quality**

- R5. Stemmed queries improve recall: "fake" matches "fakes", "test" matches "testing", "configure"
  matches "configuration"
- R6. Existing structured FTS5 queries (AND, OR, parentheses) continue to work
- R7. No regression in search relevance for exact-match queries

## Success Criteria

- The existing search relevance tests in `tests/search_relevance.rs` continue to pass
- New stemming-specific test cases demonstrate improved recall for morphological variants
- `lore search` on an existing database picks up the new tokenizer after `lore ingest` without the
  user needing to do anything special

## Scope Boundaries

- No custom tokenizer or stemming algorithm — use FTS5's built-in porter tokenizer
- No changes to the query construction logic in `src/hook.rs` — porter stemming is transparent to
  query builders
- No changes to the vector search or RRF scoring — this only affects the FTS5 leg of hybrid search
- Pattern authoring guide is a separate roadmap item, not in scope here

## Key Decisions

- **Auto-migration over manual rebuild**: Detect tokenizer mismatch in `init()` and recreate the FTS
  table automatically. FTS data is always derivable from `chunks`, so this is safe and lossless.
  This is better than requiring users to run `lore ingest --force` or a separate migration command.

## Outstanding Questions

### Deferred to Planning

- [Affects R3][Technical] How to detect whether the existing FTS table uses the old tokenizer vs the
  new one — options include checking `PRAGMA` output, storing a schema version in metadata, or
  unconditionally dropping/recreating on every init
- [Affects R5][Needs research] Whether porter stemming interacts with the FTS5 column weights
  already configured for title/body/tags ranking

## Next Steps

→ `/ce:plan` for structured implementation planning
