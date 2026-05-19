---
title: "UAT through the real binary catches inference-path bugs unit tests cannot"
date: 2026-05-19
category: best-practices
module: testing
problem_type: best_practice
component: tooling
severity: high
applies_when:
  - "Shipping a data or helper change to a system that picks between multiple input sources via priority chains (Option chains, fallback ladders, first-non-empty selectors)"
  - "Adding entries to an inference pipeline whose real-world inputs come from an external producer (agent harness, hook system, IDE plugin)"
  - "Reviewing whether a PR's test suite covers what production traffic looks like, not just what the developer can imagine"
  - "Debugging a feature that 'works in tests' but silently no-ops for real users"
tags:
  - testing
  - uat
  - integration
  - inference
  - option-chain
  - production-input
---

# UAT through the real binary catches inference-path bugs unit tests cannot

## Context

PR #61 expanded the `LANGUAGES` static slice from 6 to 27 entries, with 743 unit tests covering
entry-level shape, sweep membership, shared-signal multi-membership, contested-signal resolution,
and (after `ce-testing-reviewer` review) end-to-end `language_from_bash` calls for the new shared
keywords. Everything green.

User-acceptance testing against a built `target/release/lore` binary then revealed three production
gaps no unit test caught:

1. **The `description` vs `command` Option-chain bug**: `infer_languages()` reads `description` when
   set and falls back to `command`. Claude Code's Bash tool calls nearly always carry both. So the
   language gate computes against the agent's English prose description, not the actual shell
   command. `gradle build` with description "build the project" infers no languages; `gradle build`
   with no description correctly infers `{java, kotlin, groovy}`. In practice the gate silently
   no-ops for most Bash tool calls.

2. **The `./gradlew` path-prefix bug**: the whitespace tokeniser preserves leading `./`, so
   `./gradlew assembleDebug` never matches the `gradlew` command keyword. Same issue with
   `/usr/local/bin/gradle`, `~/.cargo/bin/cargo`, etc.

3. **The `add_pattern` MCP language-arg gap**: the MCP authoring tool silently drops a `language`
   argument because the input schema does not list it. Agents creating patterns via MCP cannot
   declare a language at all.

None of these are bugs _in_ the new languages. They are pre-existing inference-path bugs the slice
expansion made operationally relevant. Unit tests structurally could not have caught any of them,
because:

- The `description`/`command` chain is invisible at the helper boundary — `language_from_bash(s)` is
  unit-tested in isolation. The orchestrator `infer_languages(ctx)` _also_ has unit tests, but they
  construct `CallContext`s synthetically. A developer writing those tests imagines realistic inputs;
  nobody imagines "Claude Code always sends both fields".
- The `./gradlew` case requires knowing that real producers prefix relative commands with `./`. The
  tokeniser's unit tests use bare keywords.
- The `add_pattern` schema gap is a contract test that nobody writes — the schema _successfully_
  accepts unknown arguments by ignoring them.

## Problem

Helper functions are tested in isolation against developer-imagined inputs. The producer of
real-world inputs is an external system (agent harness, hook, IDE) whose actual input shapes are not
visible to the developer at the time they write the helper's tests. So:

- **Option chains** (`description.or(command)`) are tested with each branch populated separately.
  Nobody tests "what happens when _both_ are populated" — yet that's the production case.
- **Tokenisers** are tested with clean input. Real producers introduce prefixes, env vars,
  redirections, quoting. The tokeniser's unit tests don't reflect that.
- **Schemas** are tested for the arguments they accept. Arguments they _silently ignore_ never get a
  test because the developer doesn't know to write one.

The common thread: **unit tests reflect the developer's model of the input, not the producer's
actual output.** Even careful unit-test discipline (see
[[slice-shape-tests-are-not-pipeline-tests]]) cannot close this gap, because the gap is in the input
shape, not the helper logic.

## Solution

Drive the production binary end-to-end against realistic producer input, isolated from the
developer's environment. The lore-specific shape:

1. Build `target/release/lore` from the branch under review.
2. Create an isolated XDG environment under `tmp/uat-<topic>/`:
   - `XDG_CONFIG_HOME=$UAT/xdg-config`
   - `XDG_DATA_HOME=$UAT/xdg-data`
   - `XDG_STATE_HOME=$UAT/xdg-state`
3. Build a small fixture knowledge directory (10–20 markdown patterns) covering the feature surface,
   committed to a fresh git repo. Pattern bodies should deliberately _avoid_ the feature's keywords
   as literal text, so structural gating (not FTS coincidence) is what's exercised.
4. Run `lore init --repo $UAT/kb`, then `lore status`, then exercise the feature path via the actual
   producer's input format — for hook-based features, pipe Claude Code-shaped JSON into `lore hook`;
   for MCP, drive `lore serve` via JSON-RPC.
5. Enable `trace.enabled = true` in the config and inspect the trace JSONL files to see what query
   the hook actually built.

The trace inspection is the cheap diagnostic that closes the unit-test gap: the trace records the
_composed_ query — `(clang OR cpp) AND (foo)` or `elixir AND (mix OR deps)` — so a
production-realistic input either produces the expected gate or doesn't. If it doesn't, the
inference path is broken regardless of what the unit tests say.

For this PR, that diagnostic immediately surfaced:

```
{"query": "multi OR module OR dependency", ...}
```

when the expected query was `(java OR kotlin OR groovy) AND (gradle OR build)`. No language anchor,
no structural gate. Cause: the hook input carried `description: "multi module dependency"` and
`command: "gradle build"`, and the Option chain preferred description. Three minutes from suspicion
to diagnosis.

## Why this matters

Unit tests scale to thousands of assertions cheaply but stay inside the developer's model of the
input. UAT against the real binary is expensive relative to a single unit test but is the only place
producer-shape mismatches surface before users hit them. The cost-benefit flips when:

- The producer is an external system whose input shape is not part of the developer's normal review
  surface (Claude Code, an LSP client, a CI tool).
- The feature is data-driven and the failure mode is "silently does nothing" (no exception, no log
  line, no test failure — just no behaviour).
- The component sits behind multiple layers of indirection (hook → engine → helper → slice) where
  each layer is unit-tested in isolation.

For lore specifically: **every PR that changes hook-driven retrieval should include UAT against the
real binary with a fixture knowledge directory.** The per-PR manual smoke discipline
(`feedback_smoke_testing_discipline`) is exactly this; what this learning adds is the diagnostic
flow (enable trace, inspect composed query) that makes the smoke productive instead of opaque.

## Detection signal

When reviewing a PR that touches an inference pipeline:

- Does the diff include a `Option::or` / `unwrap_or` / first-non-empty chain between multiple input
  sources? If yes, the unit tests almost certainly don't cover the both-populated case.
- Does the helper consume tokenised input from an external producer (shell command, file path, query
  string)? If yes, the test inputs are probably cleaner than production.
- Does the feature have a schema that silently accepts unknown arguments? If yes, no test will catch
  the silently-ignored argument.

Each of these is a UAT trigger. The cost is one isolated tmp dir and a fixture knowledge base; the
payoff is catching a bug before it ships.

## Related

- [[slice-shape-tests-are-not-pipeline-tests]] — the layer below: even pipeline tests run with
  developer-imagined inputs
- [[hand-enumerated-test-canaries-are-landmines-in-data-driven-slices]] — a different failure mode
  of data-driven test discipline
