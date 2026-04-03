---
title: "fix: FTS5 hyphen crash and frontmatter chunk noise"
type: fix
status: active
date: 2026-04-03
---

# fix: FTS5 hyphen crash and frontmatter chunk noise

## Overview

During a real working session implementing delta ingest (#19), a structured evaluation of lore's own
effectiveness surfaced several bugs. This plan covers the two highest-severity code bugs. Remaining
findings (search relevance gaps, pattern strengthening, memory→lore migration) are tracked in
`docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md`.

## Problem Frame

Lore's value proposition is that patterns injected at the right moment change agent behavior. Two
bugs undermine this: (a) certain queries crash FTS5 silently, returning no results instead of
relevant patterns, and (b) frontmatter chunks pollute search results, displacing real content.

## Findings

### Bug 1: FTS5 crash on hyphenated terms containing column names

**Severity:** High — silent search failure, no results returned.

**Reproduction:** Search for `"dprint formatting pre-commit hook"`. FTS5 returns
`no such column: commit`. The word "commit" appears as a bare token after stripping the hyphen from
"pre-commit", and FTS5 interprets it as a column reference.

**Root cause hypothesis:** The `sanitize_fts_query()` function in `database.rs` replaces dots,
slashes, colons, braces, quotes, asterisks, and carets — but NOT hyphens. When "pre-commit" is split
by FTS5's tokenizer, the hyphen is consumed and "commit" becomes a bare token. Since `commit` is not
a recognized FTS5 column (the columns are `title`, `body`, `tags`, `source_file`, `chunk_id`), this
shouldn't crash — unless FTS5 is interpreting it as an implicit column filter syntax.

**Action:** Investigate the exact FTS5 parsing rule. The fix is likely to strip or escape hyphens in
`sanitize_fts_query()`, or to quote terms that match column names. Add a regression test with
"pre-commit hook" as input.

### Bug 3: Frontmatter chunks ranking as top results

**Severity:** Low — noise in results, not incorrect behavior.

**Evidence:** Query `"clippy pedantic"` returns the raw YAML frontmatter block
(`tags: [rust, clippy, linting, code-quality]`) as result #1 with relevance 0.99, above the actual
content chunks.

**Root cause:** The heading-based chunker treats the pre-heading content (including frontmatter) as
a valid chunk. Since frontmatter contains tags that match the query, FTS5 ranks it highly due to the
tags column weight (5x).

**Action:** The chunker already strips frontmatter from the body — verify whether the frontmatter
chunk should be excluded entirely when it contains no meaningful body content (only the YAML block).
Alternatively, set a minimum body-length threshold for the root chunk.

## Scope Boundaries

- Limited to fixing the two bugs described above
- No changes to search scoring, column weights, or relevance thresholds
- No pattern content edits or memory evaluation
- Deferred work tracked separately in `2026-04-03-002-fix-dogfooding-deferred-plan.md`

## Implementation Units

- [ ] **Unit 1: Fix FTS5 hyphen/column-name crash**
- [ ] **Unit 3: Suppress empty frontmatter chunks from search results**

## Sources & References

- Dogfooding session: delta ingest implementation (PR #19, 2026-04-02)
- Deferred findings: `docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md`
