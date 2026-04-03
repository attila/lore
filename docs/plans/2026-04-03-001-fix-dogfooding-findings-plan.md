---
title: "fix: Dogfooding findings — search bugs, chunking noise, memory→lore migration"
type: fix
status: active
date: 2026-04-03
---

# fix: Dogfooding findings — search bugs, chunking noise, memory→lore migration

## Overview

During a real working session implementing delta ingest (#19), a structured evaluation of lore's own
effectiveness surfaced several bugs and improvement opportunities. This plan captures those findings
for resolution.

## Problem Frame

Lore's value proposition is that patterns injected at the right moment change agent behavior.
Dogfooding revealed that: (a) some queries crash FTS5, (b) search relevance is inconsistent across
query formulations, (c) frontmatter chunks pollute results, and (d) memories that should be
substitutable by lore patterns can't be until the patterns are strengthened and the injection proves
reliable.

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

### Bug 2: Search relevance inconsistency across query formulations

**Severity:** Medium — users may not find patterns that exist.

**Evidence:**

- `"testing sqlite fake embedder"` → **no results** (0 hits)
- `"testing strategy real dependencies fake externals"` → **perfect hit** (1.0 relevance)
- `"unattended agent work"` → **no results**
- `"agent unattended composite shell"` → **correct hit** (0.98 relevance)

Both pairs target the same pattern. The shorter, more natural queries fail while verbose queries
that happen to overlap with the pattern body succeed.

**Root cause hypothesis:** FTS5 BM25 with the current column weights (title=10, body=1, tags=5) may
be suppressing body-only matches when query terms don't appear in the title or tags. Combined with
`min_relevance=0.6` threshold filtering, marginal matches get dropped entirely.

**Action:** Add search relevance regression tests for these specific query pairs. Investigate
whether lowering `min_relevance` or adjusting column weights improves recall without hurting
precision. Consider whether the embedding (vector) side is compensating — if Ollama is unreachable
during these searches, the FTS-only path has no vector fallback.

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

### Improvement 1: Strengthen lore patterns to replace Claude Code memories

**Context:** Evaluation showed that 4 of 7 project memories overlap with lore patterns, but some
patterns lack the behavioral imperative that makes the memory actionable.

**Specific gaps:**

1. **`rust/tooling.md`** — says what `just ci` does, but doesn't say "always run it before
   committing, never substitute individual commands." The memory
   `feedback_run_just_ci_before_push.md` exists because this imperative was violated. Append a
   "Workflow rule" section with the mandate and the incident context.

2. **`rust/tooling.md`** — doesn't mention the dprint version pinning lesson. The memory
   `feedback_dprint_hook.md` records that unpinned dprint in CI pulled a newer version with
   different defaults. Append this to the dprint section.

3. **`workflows/git-branch-pr.md`** — says "never push to main" but doesn't say "only the user/owner
   merges PRs." The memory `feedback_git_workflow.md` records an incident where pushing to main
   auto-closed a draft PR. Append this context.

**After strengthening:** Re-run the substitution evaluation. If the strengthened patterns reliably
surface via hook injection during relevant tool calls, the corresponding memories can be retired
(with backup).

**Relationship to pattern authoring guide:** The gaps above are symptoms of a broader question:
_what makes a lore pattern effective?_ The patterns that failed to substitute for memories share a
common trait — they describe _what exists_ (tools, config, conventions) but not _what to do_ (always
run X, never do Y, stop and ask when Z). Effective patterns need both descriptive and imperative
content, grounded in incident context ("why this rule exists").

This insight feeds into a separate, important work stream: **a pattern authoring guide** that ships
with lore as product documentation. The guide should be based on tested principles from this
dogfooding process — not speculative advice. It should cover at minimum:

- Descriptive vs. imperative content (both are needed; imperatives drive agent behavior)
- When and how to include incident context ("why" sections with concrete failure stories)
- Tag strategy for search discoverability (how tags interact with FTS5 column weights)
- Chunking awareness (how heading structure affects what gets injected — one heading = one injection
  unit)
- Query-friendly vocabulary (natural terms a user/agent would search for vs. jargon that only
  appears in the body)
- Anti-patterns (frontmatter-only chunks, overly broad patterns, patterns that duplicate what the
  code already shows)
- Domain-specific headings — a heading like `## Formatting` is ambiguous across every language;
  `## TypeScript Formatting` lets FTS5 title weight (10x) disambiguate. Evidence: running
  `dprint fmt` in a Rust project injected JS/TS conventions because `core-coding-style.md` has a
  generic `## Formatting` heading that matched the extracted query. The query extraction was correct
  (formatting is relevant), the search was correct (found a formatting pattern), but the pattern was
  too broad to be domain-specific. This is an authoring problem, not an extraction bug.

This guide should be iterated through real dogfooding cycles — each round of memory→lore migration
produces evidence about what pattern structures work and which don't. The guide is added to the
roadmap as a separate item.

### Improvement 2: Evaluate hook injection coverage

**Context:** During the delta ingest implementation session, lore hook injections were not
noticeably impactful. This could mean: (a) they fired but blended into existing knowledge, (b) they
didn't fire because query extraction didn't produce good search terms, or (c) they fired but were
filtered by dedup or relevance threshold.

**Action:** Add a `LORE_DEBUG=1` mode (already on the roadmap) that logs hook queries, search
results, dedup decisions, and injection content to stderr. Use this during a real working session to
trace exactly what happens.

### Incident: `gh pr edit --body` failure despite existing pattern

**Context:** Agent attempted to update a PR description using `gh pr edit --body` with an inline
heredoc, then with inline escaped markdown. Both were blocked by don't-ask mode permissions. A
pattern at `agents/unattended-work.md` → "GitHub Pull Requests" explicitly says: "use
`--body-file /tmp/pr-body.md` instead of `--body` with inline strings or heredocs. Write the body to
a tmp file first, then reference it."

**Search confirmed the pattern exists** with relevance 1.0 for the query "gh pr create body file
tmp". So either: (a) the hook injected the pattern but the agent didn't comply, or (b) query
extraction from the compound heredoc command didn't produce good enough terms to find it.

**Significance:** This is a concrete case where a pattern exists, is discoverable via search, but
still failed to prevent the exact mistake it documents. Two possible root causes:

1. **Injection gap** — the query extraction from the Bash command didn't surface the pattern at the
   right moment. Needs `LORE_DEBUG=1` to verify.
2. **Compliance gap** — the pattern injected but used suggestive language ("use X instead of Y")
   rather than prohibitive language ("NEVER use Y"). Agent reasoning may deprioritize suggestions
   under time pressure. This is another data point for the pattern authoring guide: imperative
   phrasing ("always", "never") may drive stronger agent compliance than descriptive alternatives.

Without `LORE_DEBUG=1` tracing, we cannot distinguish these cases. This incident strengthens the
priority of both the debug logging roadmap item and the pattern authoring guide.

## Scope Boundaries

- This plan captures findings only — it does not propose architectural changes to the search
  pipeline or hook system
- Pattern strengthening is limited to appending sections, not rewriting patterns
- Memory retirement is a separate future step after injection reliability is confirmed

## Implementation Units

- [ ] **Unit 1: Fix FTS5 hyphen/column-name crash**
- [ ] **Unit 2: Add search relevance regression tests for query reformulation**
- [ ] **Unit 3: Suppress empty frontmatter chunks from search results**
- [ ] **Unit 4: Strengthen `rust/tooling.md` with behavioral mandates**
- [ ] **Unit 5: Strengthen `workflows/git-branch-pr.md` with merge ownership context**
- [ ] **Unit 6: Re-evaluate memory→lore substitution after strengthening**

## Sources & References

- Dogfooding session: delta ingest implementation (PR #19, 2026-04-02)
- Related roadmap item: `LORE_DEBUG=1` verbose logging
- Memories evaluated: `project_quality_standards.md` (retired), `feedback_git_workflow.md`,
  `feedback_run_just_ci_before_push.md`, `feedback_dprint_hook.md`
