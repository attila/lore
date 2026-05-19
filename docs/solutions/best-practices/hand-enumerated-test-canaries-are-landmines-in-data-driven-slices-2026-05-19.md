---
title: "Hand-enumerated test canaries are landmines in data-driven slices"
date: 2026-05-19
category: best-practices
module: testing
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Adding a new entry to a static data slice (e.g. LANGUAGES, STOP_WORDS, a feature-flag table)"
  - "Writing a negative-shape test that needs a 'definitely not in the set' canary value"
  - "Reviewing a PR that expands a shared data structure consumed by tests across the crate"
  - "Auditing why a test that has nothing to do with a feature change still breaks after the change"
tags:
  - testing
  - data-driven
  - canary
  - anti-pattern
  - hazard-test
---

# Hand-enumerated test canaries are landmines in data-driven slices

## Context

The `LANGUAGES` static slice in `src/engine/languages.rs` started at six entries (PR #50). Tests
across the crate used real language tokens as "unknown" canaries to assert the negative shape of
helpers: `unknown_command_keyword_returns_empty` used `"bundle"` because no entry registered it; the
unknown-token tests in `languages.rs`, `status.rs`, and `chunking.rs` all used `"kotlin"` because no
entry claimed it.

PR #61 expanded the slice from 6 to 27 entries. Two of those entries — Ruby (claiming `bundle` as a
command keyword) and Kotlin (a canonical token in its own right) — silently invalidated the canary
assumption in four pre-existing tests, breaking the build in ways unrelated to the feature change.

## Problem

A test that uses a real token as its "definitely not in the set" canary makes an implicit
assumption: **that token will never be added to the set**. The assumption is invisible at the call
site. Future contributors expanding the set have no signal that adding a specific token will break
unrelated tests. The break surfaces only at CI time, and the connection between the new entry and
the failing test is non-obvious.

Worse, the failure looks like a regression in the test's subject (e.g.
"`unknown_command_keyword_returns_empty` is failing — did the helper break?") when actually the
test's _fixture_ is stale.

## Solution

Use synthetic canary strings that cannot legitimately enter the data slice:

- `"nosuchcmd"`, `"xyzzy"`, `"__missing__"` for non-keyword canaries
- Strings that violate the slice's documented constraints (e.g. for FTS5-safe tokens, a string with
  disallowed punctuation)

When a real token _must_ be used as a canary (because the test is exercising how the system handles
a specific known-deferred case), pin **why it's safe** in a comment alongside the assertion. For PR
#61 we picked `matlab` as the canary for "still-unknown token" because MATLAB is the deferred `.m`
contestation owner — origin Key Decisions explicitly state MATLAB is not a planned addition, so the
canary is contractually stable.

```rust
#[test]
fn is_known_token_rejects_unknown_and_display_names() {
    assert!(!is_known_token("rrust"));
    // MATLAB is the deferred `.m` contestation owner (origin Key
    // Decisions): `.m` is claimed for `objectivec`, MATLAB-pattern
    // authors hit the R12 unknown-token-warn path indefinitely.
    assert!(!is_known_token("matlab"));
    assert!(!is_known_token("Rust"));
    assert!(!is_known_token("Go"));
}
```

## Why this matters

Data-driven slices are exactly the structures most likely to grow. Tests that hardcode "what's not
in the slice" age badly precisely because the slice's purpose is to accumulate entries. The cost of
a synthetic canary is one function call's worth of indirection; the cost of a real-token canary is a
broken CI build the first time someone adds that token.

The pattern generalises beyond `LANGUAGES`: any closed-set membership test (feature flags, role
permissions, supported file extensions, allowed locales) that uses a real value as the negative
canary carries the same hazard.

## Detection signal

When reviewing a PR that adds entries to a static set, grep for the new tokens across the whole test
surface — not just the file housing the slice. Any hit in an _unrelated_ test file (e.g. a new
language token appears in a status formatting test) is a canary risk. Convert before merge or risk
an opaque CI failure on the next addition.

## Related

- [[slice-shape-tests-are-not-pipeline-tests]] — the sibling discipline: data additions also need
  through-the-pipeline coverage, not just slice-shape assertions
