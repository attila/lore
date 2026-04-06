---
title: "Composition cascades: new write paths can be silently undone by existing tracking-based passes"
date: 2026-04-06
category: best-practices
module: ingest
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Adding a new write or mutation path to a system that already has a tracking-based reconciliation pass (delta sync, garbage collector, cache invalidator, schema migrator)"
  - "Designing a feature that deliberately bypasses an existing state-tracking mechanism for orthogonality"
  - "Reviewing a PR whose plan says the new path is `orthogonal to` or `does not touch` an existing tracking mechanism"
  - "Running adversarial review on a feature that adds a second route to mutate something the system already mutates from another route"
tags:
  - composition
  - cascade
  - reconciliation
  - adversarial-review
  - delta
  - hazard-test
  - design-pattern
---

# Composition cascades: new write paths can be silently undone by existing tracking-based passes

## Context

The lore project shipped `lore ingest --file <path>` (PR #31) so pattern authors could index a
single markdown file without committing it first. The plan called for the new path to be
**orthogonal to walk-based delta state**: never touch `META_LAST_COMMIT` or `META_LOREIGNORE_HASH`,
never trigger reconciliation, just upsert one file's chunks. The implementation honoured every line
of that contract. Tests covered every guard. ce-review's correctness, testing, maintainability, and
project-standards reviewers all gave it green checks on the orthogonality invariant.

Then the **adversarial reviewer** constructed this scenario from scratch:

1. User runs `lore ingest` (delta) to record `META_LAST_COMMIT = A`.
2. User `git rm`s `draft.md` and commits, producing `HEAD = B`.
3. User recreates `draft.md` in the working tree (uncommitted) and runs
   `lore ingest --file draft.md`. Single-file ingest does what it promises: upserts the chunks, does
   not touch `META_LAST_COMMIT`, leaves it at `A`.
4. User runs `lore ingest` (delta). Walk-based delta computes `git diff --name-status A..HEAD`, sees
   `draft.md` as `Deleted` in the diff range, and calls `db.delete_by_source("draft.md")` — silently
   wiping the chunks single-file ingest just inserted.

Neither path is buggy in isolation. Single-file ingest correctly avoided git state because that was
the design. Walk-based delta correctly observed git history and reconciled the database to match.
The bug lives entirely in the **interaction** between two correct components.

The plan did not consider this scenario. The four "in-isolation" reviewer personas did not catch it.
Every test passed. The cascade was discovered only because the adversarial persona was instructed to
construct specific user scenarios rather than check either path against patterns.

## Guidance

When you add a new write or mutation path that **deliberately bypasses** an existing tracking
mechanism (delta cursor, content hash, version vector, cache TTL, log-structured tail), **the
existing tracking-based pass becomes a hazard**. Whatever assumptions the existing pass makes about
"I am the only thing that mutates this" are now wrong, and its next run will reconcile the state you
just wrote against its (now stale) understanding of reality.

The default outcome is that the existing pass **silently undoes** the new path's work, because
reconciliation is designed to enforce convergence with the tracked source of truth — and the tracked
source of truth doesn't know about the new path.

Three practices catch this class of bug before it ships:

### 1. Adversarial composition test during review

When reviewing a feature that adds a write path bypassing tracking state, explicitly ask:

> What happens when the existing tracking-based pass runs **after** this new path has written
> something the tracked source-of-truth does not know about?

Trace the existing pass's logic with the post-new-path state as input. If the existing pass would
delete, overwrite, or invalidate the new path's output — even though both paths are individually
correct — you have found a composition cascade. This question is mechanical and worth adding to the
review checklist for any PR that introduces a second mutation route to data the system already
mutates.

ce:review's `adversarial-reviewer` persona is the natural home for this check. It ran on lore PR #31
because the diff was over 50 LOC and the system has multiple mutation paths; that combination should
be the trigger.

### 2. Hazard-pin test at the cascade boundary

When the fix is "out of scope" for the current PR — because it requires a product decision, a
provenance-tracking schema change, or a redesign of the existing pass — capture the hazard with a
**regression test that pins the current behaviour as failing**. The test asserts the cascade happens
as expected:

```rust
#[test]
fn subsequent_delta_ingest_wipes_single_file_upsert_of_git_deleted_file() {
    // ... setup that reproduces the cascade ...
    assert!(
        db.source_files().unwrap().is_empty(),
        "current behaviour: delta ingest wipes single-file chunks of a git-deleted file. \
         If this test fails, update the pattern-authoring guide interaction note."
    );
}
```

The point is not that the current behaviour is correct — it isn't. The point is that any future
refactor that **changes** the cascade behaviour (whether to fix it or to break it differently) will
fail this test and force a conscious update to the documentation that warned users about it. Without
the pin, a future "improvement" could silently flip the failure mode and the docs would drift out of
sync with reality.

This pattern — pin the hazard, document the workaround, defer the real fix — is appropriate when the
fix is genuinely out of scope and a permanent fix would balloon the PR. It is not appropriate when
the fix is small and local.

### 3. Document the safe workflow at the user-facing entry point

If users can trigger the cascade, the documentation for the new feature must say so. Not in the
plan, not in a commit message, not in a code comment — in the user-facing docs the user will read
when learning the feature. lore put an explicit "Interaction hazard" block in
`docs/pattern-authoring-guide.md` next to the Vocabulary Coverage Technique that prescribes the new
feature, with the safe workflow spelled out:

> **Interaction hazard — `lore ingest` can wipe chunks you just upserted.** Single-file ingest is
> orthogonal to git state, but walk-based delta ingest is not. … The safe workflow is to finish
> iterating with `lore ingest --file`, commit the file to git, and only then run `lore ingest`.

The doc warning is not a substitute for fixing the cascade. It is a substitute for **silence** while
the cascade is unfixed. Users will accept a documented sharp edge; they will not accept a silent
footgun.

## Why This Matters

The cost of finding a composition cascade in production is much higher than the cost of finding it
in review:

- **In review:** ten minutes of adversarial reasoning, one hazard-pin test, one doc paragraph.
- **In a user session:** a confused author whose pattern silently disappeared from search, followed
  by a frustrated bug report whose root cause involves explaining git internals.
- **In a CI agent:** a Pattern QA loop that runs the new feature successfully, then runs the old
  feature and produces noise about missing chunks, with no signal about why.

The adversarial review pattern is cheap. The hazard-pin test is cheap. The doc paragraph is cheap.
None of them require fixing the underlying cascade. They convert a silent failure mode into a loud
one and buy time for the real fix.

The deeper lesson is that **"orthogonal to existing state"** is not a safety property; it is a
**design choice that creates a new compositional risk**. Plans that say "this path is orthogonal to
X" should also say "this path's interaction with the X-tracking pass is Y, tested by Z."

## When to Apply

Apply this practice when:

- Adding a new write or mutation route to data that an existing reconciliation pass also touches
- Adding a CLI command, MCP tool, or API endpoint that produces side effects bypassing an existing
  state-tracking mechanism
- Reviewing a plan whose Scope Boundaries say "does not update <tracking field>" or "orthogonal to
  <existing process>"
- Designing a feature that explicitly opts out of an existing convergence mechanism for performance,
  atomicity, or user-experience reasons
- Adding a test-only or maintenance-only path (a "back door") that production code does not know
  about

Skip the check when:

- The new path **does** participate in the existing tracking mechanism — composition is by
  construction
- The existing pass is purely additive (it never deletes, overwrites, or invalidates) — there is
  nothing for it to undo
- The new path writes to a different namespace, table, or key prefix that no existing pass touches

## Examples

### lore — single-file ingest cascade (the example above)

- **New path:** `ingest::ingest_single_file` writes one file's chunks, does not touch
  `META_LAST_COMMIT`.
- **Existing pass:** `delta_ingest` consumes `META_LAST_COMMIT`, computes `git diff` against `HEAD`,
  and calls `delete_by_source` for any file that was removed in the diff range.
- **Cascade:** Single-file ingest a file the user `git rm`'d but did not yet recommit. Delta ingest
  sees it as `Deleted` and wipes it.
- **Mitigation in PR #31:** hazard-pin test
  (`subsequent_delta_ingest_wipes_single_file_upsert_of_git_deleted_file`) + Interaction hazard
  block in `docs/pattern-authoring-guide.md` + a follow-up todo for the real fix
  (`docs/todos/single-file-ingest-embedding-failure-rollback.md` does not cover this; a separate
  provenance-tracking todo is needed if the cascade itself is to be fixed).

### Generic template

Whenever a plan proposes a feature whose contract includes "does not touch X", run this checklist on
the PR:

```
1. Identify every existing process that READS X.
2. For each one, ask: "What does this process do when it sees state created by the new path
   but not reflected in X?"
3. If the answer is "deletes it", "overwrites it", "invalidates it", "marks it stale", or
   anything other than "leaves it alone" — you have a cascade.
4. Either fix it now, or capture it as a hazard-pin test + a user-facing doc warning + a
   follow-up todo.
```

## Related Observations from the Same PR

Two smaller findings surfaced alongside the cascade in PR #31's ce-review. They do not warrant their
own learnings yet but are worth noting here so future readers see the cluster:

- **Two-phase initialization smell.** `ingest_single_file` initialised
  `IngestMode::SingleFile { path: String::new() }` as a placeholder and overwrote it on the success
  path. Three reviewers (correctness, adversarial, maintainability) independently flagged that
  early-return error paths left `path` empty, producing a contradictory
  `Done (single-file): → 0 chunks` line on stderr. **Rule:** if a result struct carries a field
  that's only known after work completes, either populate it from the input upfront, use `Option`,
  or use a distinct enum variant for the "not yet known" state — never a sentinel value that lies.
- **CLI binary plumbing gap.** Library-level integration tests against `ingest_single_file` passed
  exhaustively, but the entire dispatch chain (clap → `cmd_ingest` → `Config::load` → `WriteLock` →
  `dispatch_ingest` → `print_ingest_summary` success branch → exit 0) had no end-to-end test until
  PR #31 added one in `tests/ollama_integration.rs`. **Rule:** if you have a CLI that wraps a
  library, library tests do not catch binary plumbing regressions even when the library is
  exhaustively tested. At least one happy-path test must exercise the actual binary, even if it has
  to be gated on an external dependency like a running Ollama.

## Related Documentation

- `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`
  — adjacent learning about `.loreignore` reconciliation needing both add and remove directions.
  Same family of bugs (delta-pipeline composition failures), different specific shape.
- `docs/plans/2026-04-06-002-feat-single-file-ingest-plan.md` — plan for the feature whose ce-review
  surfaced this cascade. The Completion Notes section captures the full ce-review context.
- `docs/pattern-authoring-guide.md` — Vocabulary Coverage Technique section ends with the
  user-facing "Interaction hazard" block prescribed by this learning.
- `tests/single_file_ingest.rs::subsequent_delta_ingest_wipes_single_file_upsert_of_git_deleted_file`
  — the hazard-pin test that locks the current behaviour.
