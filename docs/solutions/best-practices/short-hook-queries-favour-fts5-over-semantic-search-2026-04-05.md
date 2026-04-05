---
title: "Short hook queries favour FTS5 keyword matching over semantic search"
date: 2026-04-05
category: best-practices
module: search
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Debugging why a pattern with good vocabulary does not surface via hook injection"
  - "Deciding whether to invest in semantic search tuning or keyword coverage"
  - "Writing patterns intended to surface during tool-driven hook queries"
tags: [search, fts5, semantic, embeddings, hook, query, vocabulary]
---

# Short hook queries favour FTS5 keyword matching over semantic search

## Context

Lore supports hybrid search combining FTS5 lexical matching and Ollama vector similarity via
Reciprocal Rank Fusion. Users might reasonably expect that semantic search compensates for missing
vocabulary — if a pattern says "creating PRs" and the agent queries "edit," embeddings should
understand the relationship.

Investigation during pattern authoring guide development revealed that this assumption does not hold
for hook-injected queries.

## Guidance

When diagnosing pattern discoverability issues in the hook pipeline, treat FTS5 keyword matching as
the dominant search signal, not semantic search.

Hook-injected queries are typically three to five terms extracted from a tool call (file extension,
filename components, Bash command terms). Queries of this length produce weak embedding signals —
there is not enough semantic content for the vector search to reliably identify conceptual
relationships. FTS5 keyword matching carries most of the weight for these precise, tool-driven
lookups.

This does not mean semantic search is useless. It helps with:

- Longer queries entered via `lore search` on the command line
- Queries enriched by transcript tail context (last user message)
- Conceptual matches when keyword overlap is partial but embedding similarity is strong

But for the typical hook-injection path — three to five terms from a tool call — relying on semantic
search to compensate for missing vocabulary is unreliable. Including the terms directly in the
pattern body or tags is a certainty.

## Why This Matters

Authors who assume semantic search handles synonyms may under-invest in vocabulary coverage. A
pattern that says "creating PRs" but never mentions "editing" or "updating" will score zero for an
agent running `gh pr edit`, even with hybrid search enabled. The embedding similarity between a
three-term query and a pattern body is too weak to overcome the FTS5 miss.

## When to Apply

- When writing patterns: include the verbs and nouns agents would use, do not rely on semantic
  search to bridge synonym gaps
- When debugging discoverability: check FTS5 keyword overlap first, not embedding similarity
- When tuning `min_relevance`: lowering the threshold helps semantic search but does not fix
  fundamental vocabulary gaps in short queries

## Examples

A pattern about GitHub pull requests contained "when creating PRs with `gh pr create`." An agent ran
`gh pr edit --body`, producing the query terms "edit" and "body." Neither appeared in the pattern.
Semantic search did not compensate — the query was too short for meaningful embedding similarity.
The pattern scored zero.

Adding "editing" and "updating" to the pattern body resolved the issue through direct keyword
matching, without any change to the search engine configuration.

## Related

- [Pattern Authoring Guide](../../pattern-authoring-guide.md) — "Why not semantic search?" aside in
  the Discoverable Vocabulary section
- [Search Mechanics Reference](../../search-mechanics.md) — full pipeline documentation
- `docs/solutions/database-issues/fts5-query-construction-for-hook-based-search-2026-04-02.md`
