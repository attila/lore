---
title: "Slice-shape tests are not pipeline tests"
date: 2026-05-19
category: best-practices
module: testing
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Adding entries to a static data slice that feeds an inference, retrieval, or routing pipeline"
  - "Writing per-entry tests that assert struct-field contents without invoking the consuming helpers"
  - "Reviewing a PR that claims test coverage for a new feature via entry-level assertions alone"
  - "Auditing why a data change passes 700+ unit tests but silently no-ops in production"
tags:
  - testing
  - data-driven
  - integration
  - coverage-gap
  - inference
---

# Slice-shape tests are not pipeline tests

## Context

PR #61 expanded the `LANGUAGES` static slice from 6 to 27 entries. The plan called for 21
entry-level tests (`<token>_entry_has_expected_signals`) each asserting the canonical token
resolves, the display name matches, the extension list contains at least one expected value, and so
on. The shape:

```rust
#[test]
fn java_entry_has_expected_signals() {
    let e = entry_for("java");
    assert_eq!(e.display_name, "Java");
    assert!(e.extensions.contains(&"java"));
    assert!(e.command_keywords.contains(&"javac"));
    assert!(e.marker_filenames.contains(&"pom.xml"));
}
```

All 21 entry-level tests passed. Sweep tests (`is_known_token_accepts_canonical_tokens`,
`display_name_for_resolves_known_tokens`) covered the helpers. Shared-signal tests pinned
multi-membership. Total: 743 tests passing.

Yet `ce-testing-reviewer` (T1) flagged a gap:

> No `language_from_bash` smoke-tests for any of the 21 new command keywords. The entry-level tests
> in `languages.rs` assert that keywords exist in the struct slice, but they bypass the
> `language_from_bash` token-splitting / lowercasing pipeline in `query.rs`. The only new
> `language_from_bash` test added is the `bundle→ruby` regression. AE3
> (`gradle build → {java, kotlin,
> groovy}`) is the most visible gap — it has full data-table
> coverage but zero call-path coverage.

UAT against the production binary later confirmed the gap was real, and revealed that one of the new
keywords could be unreachable in production without any unit test catching it. See
[[uat-through-real-binary-catches-inference-path-bugs]] for the specific description-vs-command
Option-chain failure mode that this kind of coverage would have caught.

## Problem

A data-driven inference pipeline has two layers:

1. **The slice**: a static array of structs that declares "what counts as X".
2. **The helpers**: functions that read the slice and apply tokenisation, normalisation, splitting,
   lowercasing, and filtering to map runtime input onto entries.

**Slice-shape tests prove the data exists.** They iterate `LANGUAGES`, find the entry, and assert
its fields. They cannot prove the pipeline reaches that entry under realistic inputs — because they
never invoke the pipeline.

**Pipeline tests prove the data is reachable.** They feed realistic input strings (paths, bash
commands, query strings) into the helper functions and assert the helper returns the expected
entry's token. They cover splitting, lowercasing, filtering, and the order in which signals are
evaluated.

The two test surfaces cover orthogonal failure modes. A typo in the slice fails both. But:

- A regression in `split_whitespace` ordering, lowercasing logic, or whole-token matching fails only
  the pipeline tests.
- A bug where an `Option`-chain prefers the wrong input source fails only the pipeline tests.
- A new canonical keyword silently shared with an existing entry's keyword fails only the pipeline
  tests.

## Solution

For every new slice entry that participates in inference, write **at least one pipeline test per
signal type the entry uses**:

```rust
#[test]
fn bash_gradle_build_yields_java_kotlin_groovy() {
    // AE3 end-to-end (covers R4 Gradle three-way through the bash
    // pipeline, not just the slice).
    let langs = language_from_bash("gradle build");
    assert!(langs.contains(&"java".to_string()));
    assert!(langs.contains(&"kotlin".to_string()));
    assert!(langs.contains(&"groovy".to_string()));
}
```

The pipeline test feeds a realistic input string and asserts the helper returns the expected set.
For multi-membership claims, assert each member explicitly; for single-owner claims, assert length
and membership.

For bulk coverage, a single table-driven test can sweep many entries:

```rust
#[test]
fn bash_single_owner_keywords_resolve_to_one_entry() {
    for (cmd, expected) in [
        ("mvn package", "java"),
        ("cabal build", "haskell"),
        ("composer install", "php"),
        // ...
    ] {
        let langs = language_from_bash(cmd);
        assert_eq!(langs, vec![expected.to_string()]);
    }
}
```

## Why this matters

The cost of pipeline tests is small: each test is 3–5 lines and runs in microseconds. The cost of
missing them is a feature that compiles, passes its unit tests, ships, and silently does nothing in
production — surfacing only when an operator notices that retrieval is not gating on the new signal.

This is particularly insidious for data-driven inference systems where the slice is the "feature":
the slice grows, the entry-level tests grow with it, the suite stays green, and confidence in the
suite stays high. But the pipeline path remains exercised only by whatever subset of legacy entries
already had pipeline tests. The newly-added entries are dead code from the helper's perspective
until somebody triggers the failure in production.

## Detection signal

When reviewing a data-only PR:

1. Grep for the helper functions that consume the slice (`language_from_*`, `lookup_*`,
   `resolve_*`).
2. For each new slice entry, check whether at least one test invokes those helpers with input that
   would resolve to the new entry.
3. If only entry-level (`.contains(&"x")`) assertions exist, the pipeline layer is untested for the
   new entries.

The check takes under a minute and catches the gap before merge.

## Related

- [[hand-enumerated-test-canaries-are-landmines-in-data-driven-slices]] — the sibling discipline:
  when expanding a data slice, also audit pre-existing tests that hardcoded "definitely not in the
  slice" values
- [[uat-through-real-binary-catches-inference-path-bugs]] — the broader principle: even pipeline
  tests miss inference-path bugs that only surface with production input shapes
