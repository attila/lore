---
title: Dogfood-validate scope before extending an engine workstream
date: 2026-05-08
category: workflow-issues
module: planning/dogfooding
problem_type: workflow_issue
component: development_workflow
severity: medium
applies_when:
  - A multi-track plan has shipped track 1 and track 2 is still scoped from the
    original brainstorm
  - The feature interacts with user-authored content such as patterns, prompts, or
    configurations
  - Engine changes are being scoped before measurement against a real corpus
  - Early signals suggest most noise is data-shaped rather than logic-shaped
related_components:
  - documentation
  - tooling
tags:
  - dogfooding
  - planning
  - workstream-scoping
  - pattern-authoring
  - lore
  - predicate
---

# Dogfood-validate scope before extending an engine workstream

## Context

A workstream had been scoped as two tracks against an observed problem: context pollution from
per-tool-call pattern re-injection. Track 1 added an engine primitive (an `applies_when` predicate
gating universal patterns) and shipped cleanly. Track 2 was provisionally scoped as a parallel
engine extension to address "keyword bleed" for non-universal patterns. Before extending the engine
again, the smaller Track 1 piece was dogfooded against the actual production pattern repository — 26
patterns, 107 chunks of authored content — to observe what kinds of bleed remained and which engine
primitives would address them.

## Guidance

When a workstream is scoped as "extend engine X to fix observed problem Y", do not commit to the
engine extension on the strength of the original framing. Ship the smallest engine slice first,
dogfood it against real data, and classify each residual occurrence of Y by root cause before
extending further. Separate routing problems (which engine primitives can fix) from content-quality
problems (which only authoring can fix). Reprioritise the remaining tracks based on the observed
mix, not the pre-dogfood hypothesis. Treat the dogfood as a scope-validation gate, not a polish step
at the end of the workstream.

## Why This Matters

Engine work is expensive and compounds — every primitive becomes surface area to maintain, document,
and reason about. Authoring fixes are cheap, reversible, and often unblock the same symptoms faster.
Without a dogfood gate, a workstream framed around engine extension will deliver engine extensions,
even when most of the observed pain lives in the content layer. The cost is double: time spent on
engine work that does not move the metric, plus continued tolerance of authoring debt that the
engine cannot mask. Dogfooding before extending recognises that routing improvements help but
content quality is the dominant input — garbage-in still produces garbage-out, however sharp the
router.

## When to Apply

- A workstream is framed as "extend engine X to address observed problem Y" and the next track
  proposes more engine surface area.
- The system has a content or data layer authored by humans (patterns, prompts, rules,
  configurations) feeding into the engine being extended.
- Y has been observed but not yet classified by root cause across a representative sample of real
  data.
- A smaller, related engine slice has just shipped or can be shipped quickly to enable measurement.
- The proposed engine extension is non-trivial relative to an authoring audit of the same surface
  area.

## Examples

The original brainstorm framed context pollution as Track 1 (universal patterns re-firing on
irrelevant tool calls, addressed by an `applies_when` predicate) and Track 2 ("keyword bleed" on
non-universal patterns, scoped as a parallel engine extension). After Track 1 shipped, dogfooding
against the live pattern repository surfaced seven bleed cases. Five were authoring debt:

- A Rust SQLite pattern whose body keywords matched any `sqlite3` CLI invocation, even when the
  project had no Rust SQLite code in scope.
- A pull-request-description pattern whose two-letter keyword "PR" matched the substring
  "predicate".
- A documentation pattern with keywords broad enough to match any documentation operation.
- An Atlassian MCP pattern whose use of "MCP" collided with the host MCP server's own context.
- A multi-surface consistency pattern with vague scope.

Only one case — a JavaScript pattern's `src` token firing inside a Rust repository — was a genuine
engine-primitive problem requiring repository-language awareness. Roughly 70–80 % authoring, 20–30 %
engine. Track 2 was reordered as a result: pattern authoring audit first (using the existing
`/coverage-check` skill), a per-class relevance floor knob second, and engine extension of the
predicate to non-universals last, scoped only to the residual. The reframe was only visible after
Track 1 shipped and was exercised against real content; the original brainstorm could not have
produced it from first-principles analysis.

## Related

- [`docs/solutions/best-practices/coverage-check-query-source-must-simulate-hook-not-llm-2026-04-08.md`](../best-practices/coverage-check-query-source-must-simulate-hook-not-llm-2026-04-08.md)
  — same content-quality-over-engine-cleverness theme, framed as a tool-design rule rather than a
  workstream-scoping heuristic.
- [`docs/solutions/best-practices/short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md`](../best-practices/short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md)
  — argues that for short hook queries, vocabulary in the content beats engine sophistication.
- [`docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md`](../logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md)
  — predates `applies_when`; its mitigation guidance now has a real predicate-based gating mechanism
  worth referencing in a refresh pass.
