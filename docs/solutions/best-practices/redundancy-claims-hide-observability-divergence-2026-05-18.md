---
title: "Redundancy claims in plans hide observability divergence between guards"
date: 2026-05-18
category: best-practices
module: planning-workflow
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "A plan or deepening pass labels two guards, branches, or filters as redundant and proposes removing one"
  - "One of the candidate guards has side effects beyond its return value — counter increments, cap or budget consumption, summary aggregates, operator-facing diagnostics"
  - "The redundancy claim is justified by behavioural equivalence on the dominant axis (what gets filtered) without separately verifying equivalence on the side-effect axes"
  - "A plan has been through multiple doc-review rounds plus a deepening pass and a structural simplification still feels safe to ship"
tags:
  - planning
  - deepening-pass
  - code-review
  - redundancy
  - observability
  - guard-removal
  - review-discipline
  - design-pattern
---

# Redundancy claims in plans hide observability divergence between guards

## Context

Rust refactor (PR #60 in the `lore` CLI, branch `feat/trace-walk-predicate`) introduced a shared
`walk::is_real_trace_file` predicate consumed by `TraceStats::compute` and
`maintenance::enumerate_trace_files`. The plan at
`docs/plans/2026-05-16-001-feat-trace-walk-predicate-plan.md` went through three doc-review rounds
plus an architectural deepening pass and shipped with high confidence. A post-implementation
code-review pass on the diff caught a behavioural regression that none of the plan reviews surfaced.

## Guidance

When a plan or deepening pass declares two filter guards "redundant" and deletes one, the redundancy
claim must be qualified on every axis the guards touch — not just the dominant filtering behaviour.
Two guards that exclude the same input set can still diverge on:

- counter increments and summary aggregates,
- cap or budget consumption,
- observability and operator-facing diagnostics.

In this instance, `src/trace/maintenance.rs::run_pass` had two guards on the compress loop:

1. an explicit `if path.extension().is_none_or(|e| e != "jsonl") { continue; }` at the call site,
   and
2. `gzip_file`'s internal early-return when the extension is already `gz`.

The deepened plan's Decision 6 declared (1) redundant with (2) and removed it, citing a clean
bidirectional contract between the walk predicate's accept-set and `gzip_file`'s short-circuit.

What was missed: `gzip_file` short-circuits with `Ok(())`, which the call site reads as a successful
compression — `summary.compressed += 1`. Removing guard (1) caused the loop to count `.jsonl.gz`
no-ops as compressions, consume `compress_cap` slots, and — depending on read-dir ordering —
potentially starve real `.jsonl` work on a given pass. The bidirectional contract logic itself was
sound; the framing of the call-site guard as "redundant" was not.

## Why This Matters

Deepening passes reason about decisions in isolation and trace dominant behaviour. They reliably
miss:

- counter and aggregate side effects (`summary.compressed`, error tallies, telemetry counts),
- cap and budget consumption (`compress_cap`, retry budgets, rate-limit slots),
- effects that surface only in operator diagnostics, not in user-facing correctness.

A single focused code-review pass with both functions on screen catches what plan review cannot. The
transferable patterns:

1. "Redundant" must always be qualified — redundant _for what_? Filtering? Counters? Caps? Logs? All
   of them?
2. A short-circuit's return shape determines composition. `Ok(())` reads as success at the call
   site; if the intent is "silently skipped", the return shape needs to encode that (e.g.
   `Result<bool>` where `false` means skipped, or a dedicated `Skipped` variant).
3. Plan review with rationale prose is strong at "did we consider X". Code review with the diff in
   hand is strong at "does the code do what the prose claims".

## When to Apply

- Reviewing a plan that removes a filter or guard on grounds of redundancy with another guard
  further down the call chain.
- Running a deepening pass over a plan with multi-stage filter chains.
- Auditing refactors that merge two functions filtering the same input.
- After implementation lands: always run a focused code-review pass on the diff, even if the plan
  has been deepened multiple times. The plan-review-as-substitute-for-code-review failure mode is
  real and load-bearing here.

A useful deepening-pass check to add when a "redundant" framing appears: write down what each
candidate guard _does_ in three columns — filtering, side effects, observability — and require the
redundancy claim to hold across all three columns before removal lands in the plan.

## Examples

Concrete instance from PR #60:

- Before (the line Decision 6 removed):
  `if path.extension().is_none_or(|e| e != "jsonl") { continue; }` at the head of the compress loop
  in `src/trace/maintenance.rs::run_pass`.
- After (review fix in commit `86f1b4e`): the guard reinstated, plus a regression test
  `run_pass_compress_phase_does_not_recount_already_gzipped` pinning that an aged directory
  containing only `.jsonl.gz` files produces `summary.compressed == 0`.

The deepening notes correctly identified the bidirectional contract between `walk.rs`'s accept-set
and `gzip_file`'s `.gz` short-circuit — the contract logic was sound. What failed was applying
"redundant" to the call-site guard without separately checking counter and cap side effects.

## References

- Plan: `docs/plans/2026-05-16-001-feat-trace-walk-predicate-plan.md` (Key Technical Decisions,
  Decision 6).
- PR: #60 in the `lore` repo. Commit `df24a5e` for the original implementation, `86f1b4e` for the
  review fix.

## See also

- `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  — sibling pattern: an "orthogonality" claim in a plan creates a review blind spot that adversarial
  post-implementation review catches. This doc is the redundancy-claim analogue of the
  orthogonality-claim cascade; both are plan-level architectural-claim review hazards.
- `docs/solutions/best-practices/catalogue-index-undermines-pinned-body-absence-assertions-2026-05-08.md`
  — closest structural sibling on the redundancy-is-not-equivalence axis: two co-located surfaces
  look equivalent, one silently undermines the other's stated guarantee.
- `docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`
  — same "short-circuit returns Ok that the caller interprets as success" failure mode at a
  different layer.
- `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`
  — related prior art on filter-pipeline subtlety; same family of "filter behaviour has a
  non-obvious second dimension that's easy to miss".
