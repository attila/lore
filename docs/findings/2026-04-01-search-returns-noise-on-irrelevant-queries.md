---
title: Search returns low-relevance noise instead of "no matches" on irrelevant queries
type: finding
severity: low
date: 2026-04-01
area: search
---

# Search returns low-relevance noise instead of "no matches" on irrelevant queries

## Context

Discovered during MCP integration testing with Claude Code. When searching for a nonsensical query,
`search_patterns` returns results with very low relevance scores rather than indicating no matches.

## Observed behavior

- Query: `"xyzzy nonexistent pattern that surely does not exist"`
- Returns 3 results with relevance scores ~0.016
- Response says "Found matching patterns" — misleading, especially to an agent that trusts tool
  output
- By comparison, a relevant query ("testing conventions" → testing file) scores ~0.033

## Expected behavior

When no results exceed a minimum relevance threshold, the response should say "No matching patterns
found" rather than returning noise. The threshold should be configurable via `[search]` config with
a sensible default.

## Suggested fix

Add `min_relevance` field to `SearchConfig` in `lore.toml`. In the search handler, filter results
below the threshold. When the filtered list is empty, return a "No matching patterns found" message.

## Reproduction

1. `lore init --repo <knowledge-dir>` with any content
2. Call `search_patterns` with a nonsensical query
3. Observe low-relevance results returned as "Found matching patterns"
