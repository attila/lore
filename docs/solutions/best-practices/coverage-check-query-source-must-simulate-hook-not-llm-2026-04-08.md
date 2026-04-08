---
title: Coverage-check query source must simulate the hook, not ask the LLM
date: 2026-04-08
category: best-practices
module: coverage-check
problem_type: best_practice
component: skills
severity: high
applies_when:
  - Designing a skill or agent tool that evaluates a pattern's production discoverability
  - Building any self-evaluation loop where the same agent reads the subject and writes the
    test inputs
  - Adding a vocabulary-coverage or recall check whose signal must be independent of the
    author's own wording
tags:
  - skills
  - coverage-check
  - pattern-authoring
  - paraphrase-bias
  - hook-simulation
  - agent-native
---

# Coverage-check query source must simulate the hook, not ask the LLM

## Context

The coverage-check skill automates the manual Vocabulary Coverage Technique from
`docs/pattern-authoring-guide.md`. Its job is to catch vocabulary gaps between a pattern's wording
and the queries the lore PreToolUse hook will actually synthesise from agent tool calls at runtime.
The first implementation asked the LLM to brainstorm 5-12 candidate queries from the pattern body,
guided by an FTS5 rubric that forbade known failure modes (command-name-only queries,
sub-three-character tokens, stop-words).

## The problem

The same agent that reads the pattern body also writes the queries. The queries paraphrase the body
nearly losslessly: every term in the body is a candidate token for a query, and the agent picks the
ones that are most likely to surface the pattern because that is what the rubric asks for. Coverage
was trivially high on every pattern tested, and the report told the author nothing about whether the
pattern would be found at runtime by the actual hook.

Worse, the FTS5 rubric's forbidden list was precisely the wrong list. It forbade
`Bash just ci`-shaped tool calls because they produce zero queryable terms after hook-style cleaning
— but those are exactly the tool calls whose zero-term output is the most valuable signal the skill
could surface. A pattern whose discoverability depends on `just ci`-shaped hook triggers has
structurally weak production coverage, and the v0 skill was actively hiding that finding.

## What to do instead

Derive the candidate query set from **hook simulation**, not from LLM brainstorm. Specifically:

1. Inspect the target pattern's tags, headings, concrete filenames, and fenced code blocks to infer
   3-6 synthetic tool calls an agent would plausibly issue when this pattern applies.
2. Present the inferred tool-call list to the author and ask for confirmation, edit, or replacement
   before running any extraction. This author checkpoint is the paraphrase-bias mitigation: the
   author is correcting tool calls, not writing queries.
3. For each confirmed tool call, pipe a thin `{tool_name, tool_input}` JSON envelope through a Rust
   subcommand (`lore extract-queries`) that wraps the PreToolUse hook's own
   `extract_query(&HookInput) -> Option<String>` logic. The subcommand prints the FTS5 query the
   hook would inject (or nothing if no terms survive cleaning).
4. Treat empty stdout as a **diagnostic signal**, not an error. It means the hook would inject
   nothing for that tool call, which is a real discoverability finding the skill should surface, not
   hide.
5. Offer an optional `qa_simulations` frontmatter field for authors to override the inferred
   tool-call list when automatic inference cannot find good signals (unusual patterns,
   workflow-focused patterns) or when reproducibility across runs matters.

The candidate queries are now byte-for-byte identical to what the runtime hook would see if an agent
actually issued the same tool calls. Paraphrase bias is reduced from "the agent reworded the pattern
body" to "the agent picked which tool calls to simulate" — a much smaller and author-auditable bias.

## Why this works

The lore PreToolUse hook's `extract_query` is pure, deterministic, and cheap to call. It takes a
`HookInput` (the same shape Claude Code hands the hook at runtime) and returns an `Option<String>`.
Wrapping it in a CLI subcommand costs a few dozen lines of Rust and gives the skill access to the
exact query strings the hook would synthesise. No LLM judgment enters the query string itself; the
only LLM step is choosing which synthetic tool calls to simulate, and that step happens behind an
author confirmation prompt.

The three residual biases are explicit and auditable:

1. **Tool-call selection bias.** The agent picks the tool calls. Mitigation: show the list to the
   author for confirmation before extraction runs.
2. **Inference heuristic bias.** The tag-to-tool-call lookup tables embedded in the skill prompt are
   heuristics. Mitigation: concrete filenames named in the pattern body take precedence over
   inferred ones, and the `qa_simulations` frontmatter override is the escape hatch.
3. **Empty-result treatment.** Tool calls that produce empty queries are a finding, not a failure.
   The skill records the fact and continues.

## Anti-pattern to avoid

Do not try to patch LLM-brainstormed queries with stricter rubrics. Every additional forbidden
pattern makes the paraphrase bias worse because it forces the agent to stay closer to the body's own
terms. The failure modes the rubric forbids are exactly the findings a real coverage check should
surface.

## Related

- `docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md` —
  the underlying finding about command-name-only tool calls.
- `docs/solutions/best-practices/short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md`
  — why the hook uses FTS5 term extraction in the first place.
- `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md` — "Design pivot: query source"
  section documents the diagnostic walk-through and the implementation steps.
