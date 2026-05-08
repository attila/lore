---
title: Catalogue-index undermines pinned-body absence assertions
date: 2026-05-08
category: best-practices
module: hook/session-context tests
problem_type: best_practice
component: testing_framework
severity: medium
applies_when:
  - The rendered artefact under test contains both a full-body section and a summary, catalogue,
    index, or table of contents that references the same items
  - A test asserts that a specific item is absent from one of those sections but not necessarily
    the other
  - The natural identifier (title, name, slug) is shared across sibling sections by design
  - Negative assertions are scoping a filter, exclusion rule, or visibility predicate over a
    partitioned output
tags:
  - testing
  - assertions
  - session-context
  - hooks
  - false-negatives
  - markers
---

# Catalogue-index undermines pinned-body absence assertions

## Context

The lore hook builds a SessionStart payload (via `format_session_context` in `src/hook.rs`, the
function that assembles the per-session prelude) by emitting two sections back to back:
`## Pinned conventions`, which inlines the full body of universal patterns, followed by
`Available patterns:`, a catalogue index that lists every pattern's title and tags on a single line.
A recent change added a SQL filter excluding predicated universal patterns from the pinned section,
and tests needed to prove a given pattern's body was no longer being pinned. The natural assertion —
search the rendered context for the pattern's title — collides with the catalogue, because the title
appears there regardless of whether the body was pinned.

## Guidance

When asserting that a chunk of content is absent from a rendered artefact, anchor the assertion on a
token that only exists inside that chunk's body. Embed a unique marker string (for example
`predicated-git-marker`) in the body fixture, then assert
`!output.contains("predicated-git-marker")`. Do not reach for the pattern's title, identifier, or
any field that is also surfaced by sibling sections such as a catalogue, table of contents, or
summary index. The marker should be recognisable as a test artefact, scoped tightly enough that it
cannot collide with unrelated rendering, and inserted at fixture-construction time so the test owns
its uniqueness rather than borrowing it from production data.

## Why This Matters

A title-based absence assertion against a payload that also renders a catalogue is vacuous: the
catalogue keeps the title alive even when the body is correctly excluded, so the test passes whether
the filter works or not. Vacuous assertions are worse than missing tests because they advertise
coverage that does not exist and let regressions through silently. In the branch where this learning
was captured, the same mistake was caught three separate times — initial F2 implementation, F4
implementation, and round-2 code review — which is the load-bearing signal: a single slip is a
mistake, three in one branch is a sticky failure mode that demands a named practice rather than
ad-hoc vigilance.

## When to Apply

- The rendered artefact under test contains both a full-body section and a summary, catalogue,
  index, or table of contents that references the same items.
- A test asserts that a specific item is absent from one of those sections but not necessarily the
  other.
- The natural identifier (title, name, slug, identifier) is shared across sibling sections by
  design.
- You are tempted to assert `!output.contains(<title>)` and the production renderer has more than
  one place that title can appear.
- Negative assertions are scoping a filter, exclusion rule, or visibility predicate over a
  partitioned output.

## Examples

Wrong, in `hook_session_start_excludes_predicated_universal_from_pinned_section`:

```rust
let ctx = format_session_context(&store)?;
assert!(!ctx.contains("Predicated Git Pattern"));
```

This passes whether or not the pinned-section filter works, because the line
`Available patterns: Predicated Git Pattern [git]` keeps the title in the output via the catalogue
index.

Right:

```rust
// fixture body contains: "...rule applies. predicated-git-marker ..."
let ctx = format_session_context(&store)?;
assert!(!ctx.contains("predicated-git-marker"));
assert!(ctx.contains("Predicated Git Pattern")); // catalogue still lists it
```

The marker only ever appears in the body, so its absence pins the exclusion. The companion positive
assertion documents that the catalogue line is intentionally preserved. The same pattern applied in
the tools-only and typo-predicate tests (`tools-only-marker`, `typo-predicate-marker`).

## Related

- [`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`](composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md)
  — adjacent test-discipline learning. Both apply when a render path or write path serves multiple
  outputs simultaneously and the test must distinguish them. Track 1B's F1 hazard-pin test on
  PostCompact's `reset_dedup` is a fresh instance of that pattern alongside this one.
- [`docs/solutions/workflow-issues/dogfood-reframes-workstream-2026-05-08.md`](../workflow-issues/dogfood-reframes-workstream-2026-05-08.md)
  — origin of the held-follow-up batch (Track 1B's F1–F4 commits) that produced this learning. The
  "three catches in one branch" recurrence signal came out of operating that batch.
- [`docs/solutions/best-practices/coverage-check-query-source-must-simulate-hook-not-llm-2026-04-08.md`](coverage-check-query-source-must-simulate-hook-not-llm-2026-04-08.md)
  — adjacent vacuous-assertion shape, different mechanism. Both warn that an assertion can pass for
  the wrong reason when the test's signal is contaminated by an upstream surface.
