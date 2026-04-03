---
title: "fix: Dogfooding findings — search relevance, pattern strengthening, memory migration"
type: fix
status: deferred
date: 2026-04-03
---

# fix: Dogfooding findings — search relevance, pattern strengthening, memory migration

## Overview

Deferred findings from the dogfooding evaluation during delta ingest (#19). The immediate bugs (FTS5
hyphen crash, frontmatter chunk noise) are tracked in
`docs/plans/2026-04-03-001-fix-dogfooding-findings-plan.md`. This plan covers the remaining
improvements: search relevance gaps, pattern content strengthening, hook injection evaluation, and
memory→lore migration.

## Status Updates

- `LORE_DEBUG=1` verbose logging landed in PR #20 — Improvement 2 (hook injection coverage
  evaluation) is now unblocked
- FTS5 porter stemming is a roadmap item that addresses Bug 2 from the code side

## Findings

### Bug 2: Search relevance inconsistency across query formulations

**Severity:** Medium — users may not find patterns that exist.

**Evidence:**

- `"testing sqlite fake embedder"` → **no results** (0 hits)
- `"testing strategy real dependencies fake externals"` → **perfect hit** (1.0 relevance)
- `"unattended agent work"` → **no results**
- `"agent unattended composite shell"` → **correct hit** (0.98 relevance)

Both pairs target the same pattern. The shorter, more natural queries fail while verbose queries
that happen to overlap with the pattern body succeed.

**Root cause:** Primarily a content problem — query terms don't overlap with the pattern's indexed
vocabulary. "fake" doesn't match "fakes" (no stemming), "sqlite" and "embedder" don't appear in the
target pattern at all. FTS5 scoring/thresholds are secondary. Two complementary solutions:

1. **FTS5 porter stemming** (code) — enables "fake"→"fakes", "test"→"testing" matching. Roadmap
   item.
2. **Pattern authoring guide** (content) — guidance on query-friendly vocabulary. Roadmap item.

**Action:** Add search relevance regression tests for these specific query pairs. These serve as a
baseline to measure improvement from stemming and better-authored patterns.

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

### Improvement 2: Evaluate hook injection coverage

**Context:** During the delta ingest implementation session, lore hook injections were not
noticeably impactful. This could mean: (a) they fired but blended into existing knowledge, (b) they
didn't fire because query extraction didn't produce good search terms, or (c) they fired but were
filtered by dedup or relevance threshold.

**Action:** Use `LORE_DEBUG=1` (now available via PR #20) during a real working session to trace
exactly what happens. Log hook queries, search results, dedup decisions, and injection content.

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
   right moment. Use `LORE_DEBUG=1` to verify.
2. **Compliance gap** — the pattern injected but used suggestive language ("use X instead of Y")
   rather than prohibitive language ("NEVER use Y"). Agent reasoning may deprioritize suggestions
   under time pressure. Data point for the pattern authoring guide.

## Scope Boundaries

- Pattern strengthening is limited to appending sections, not rewriting patterns
- Memory retirement is a separate future step after injection reliability is confirmed

## Implementation Units

- [ ] **Unit 2: Add search relevance regression tests for query reformulation**
- [ ] **Unit 4: Strengthen `rust/tooling.md` with behavioral mandates**
- [ ] **Unit 5: Strengthen `workflows/git-branch-pr.md` with merge ownership context**
- [ ] **Unit 6: Re-evaluate memory→lore substitution after strengthening**

## Sources & References

- Dogfooding session: delta ingest implementation (PR #19, 2026-04-02)
- Related roadmap item: FTS5 porter stemming
- Related roadmap item: Pattern authoring guide
- Memories evaluated: `project_quality_standards.md` (retired), `feedback_git_workflow.md`,
  `feedback_run_just_ci_before_push.md`, `feedback_dprint_hook.md`
