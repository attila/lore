---
title: Search silently falls back to FTS when Ollama is unreachable
type: finding
severity: low
date: 2026-04-01
area: search
---

# Search silently falls back to FTS when Ollama is unreachable

## Context

Discovered during MCP integration testing with Claude Code. When Ollama is stopped while
`lore serve` is running, `search_patterns` falls back to FTS-only search silently.

## Observed behavior

- Search returns results (good — graceful degradation)
- MCP server stays up (good — no crash)
- Relevance scores are negative (FTS-only scoring, no vector component)
- No warning that Ollama is unreachable and embeddings were skipped

## Expected behavior

The response should include a warning line indicating that semantic search was unavailable and
results are FTS-only. Something like:

```
Warning: Ollama unreachable — showing text-match results only (no semantic ranking).
```

This helps the agent (and human reviewing tool output) understand why results may be less relevant
than usual.

## Reproduction

1. `lore init --repo <knowledge-dir>`
2. `brew services stop ollama` (or equivalent)
3. Call `search_patterns` via MCP — observe no warning in response

## Suggested fix

In `search_patterns` handler: if embedding fails, include a warning prefix in the response text. The
search itself already works (FTS fallback) — this is purely about surfacing the degraded state.
