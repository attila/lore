---
title: "Preserve-on-`None` branches inherit parser normalisation — pin the asymmetry, don't fight it"
date: 2026-05-19
category: design-patterns
module: ingest
problem_type: design_pattern
component: tooling
severity: medium
applies_when:
  - "Designing a three-way `Option<&[T]>` argument (absent preserves, empty clears, non-empty replaces) for a serialised list field"
  - "Two sibling fields share the same three-way semantics but their parsers normalise input differently (lowercasing, NFC, percent-decoding)"
  - "Reviewing a preserve branch that round-trips an existing on-disk value through a parser before re-rendering"
  - "Auditing whether a body-only update silently canonicalises file content the user never explicitly changed"
tags:
  - canonicalisation
  - preserve-branch
  - parser-asymmetry
  - three-way-semantics
  - intentional-asymmetry
  - pin-the-contract
---

# Preserve-on-`None` branches inherit parser normalisation — pin the asymmetry, don't fight it

## Context

PR #63 added a `language: Option<&[&str]>` argument to `ingest::update_pattern` mirroring the
existing three-way semantics of `tags`:

- `None` → preserve the existing frontmatter list (avoids the de-language footgun on body-only
  rewrites)
- `Some(&[])` → clear
- `Some(&[...])` → replace wholesale

Implementation copied the `tags` precedent: read the existing file, parse out the existing list via
`parse_frontmatter_language_list`, feed the parsed list into `build_file_content`'s re-renderer.

A correctness reviewer flagged a subtle divergence: the language parser **lowercases** tokens at
read time (`src/chunking.rs::parse_frontmatter_language_list`), so a body-only `update_pattern`
against an existing `language: [Rust]` file rewrites it as `language: [rust]`. The `tags` parser is
case-preserving — its preserve branch is byte-stable, but the language preserve branch is not.

Two possible reads of the divergence:

- **As a bug** — the preserve semantics promise to keep the existing list verbatim; lowercase
  rewriting violates that promise.
- **As intentional asymmetry** — the canonical form for language tokens _is_ lowercase (the
  `LANGUAGES` table keys are lowercase, the DB has stored lowercased tokens since PR #50, the
  retrieval gate matches lowercased tokens). The preserve branch making the file converge to the
  canonical form is consistent behaviour, not divergent behaviour.

The intentional-asymmetry read won. The asymmetry was pinned with a documentation comment in
`update_pattern` and a regression-pinning test
(`update_pattern_preserve_lowercases_existing_mixed_case_language`). A future change in either
parser's casing posture surfaces at the call site rather than passing silently.

## Guidance

When two sibling fields share three-way preserve/clear/replace semantics on a list, their preserve
branches re-render through their respective parsers. Whatever transformations the parsers apply
**will surface on the preserve path**. Three concrete rules:

### 1. Compare parser normalisation between sibling fields explicitly

Before assuming a preserve branch is byte-stable, audit the parser:

```
For each sibling field with preserve-on-`None` semantics:
  1. What does the parser return when called on the existing file?
  2. Is the return canonicalised (lowercased, NFC, sorted, deduped) or verbatim?
  3. If two sibling fields have different canonicalisation postures, the preserve branches
     WILL diverge in their on-disk effect.
```

For PR #63: `parse_frontmatter_tag_list` is verbatim-with-quote-stripping.
`parse_frontmatter_language_list` is lowercase-with-quote-stripping-with-canonical-token-validation.
The two preserve branches were guaranteed to behave differently on mixed-case input from the moment
the language parser landed in PR #50 — the asymmetry just wasn't visible until `update_pattern`
started exercising it.

### 2. Decide whether the asymmetry is a bug or intentional — then pin it

The decision turns on whether the parser's normalisation **is** the canonical form for the field:

- If the parser normalises because the field has a canonical form (lowercase for language tokens
  matched against a canonical table; NFC for filenames; percent-decoded URLs), the preserve branch
  converging to that form is intentional. The file shape converges to match what the DB / engine
  already stores. **Pin the convergence as intentional with a regression test that names the
  asymmetry.**
- If the parser normalises by accident (an over-eager trim, an unnecessary lowercase on a
  case-sensitive field), the preserve branch silently mutates the user's content. **Fix the parser,
  not the preserve branch.**

The pin shape, when intentional:

```rust
#[test]
fn update_pattern_preserve_lowercases_existing_mixed_case_language() {
    // Intentional contract pin: the language parser lowercases at read time
    // (the canonical form for `LANGUAGES`-table lookups), so a body-only
    // update against an existing `language: [Rust]` file rewrites the line
    // as `language: [rust]`. Asymmetric with `tags`'s case-preserving
    // preserve branch by design — see the explanatory comment in
    // `update_pattern` for the rationale.
    //
    // A future change to either parser's casing posture lands here.
    let result = update_pattern(/* file with language: [Rust, Kotlin] */ ..);
    let content = fs::read_to_string(...).unwrap();
    assert!(content.contains("language: [rust, kotlin]"));
}
```

The test does two things at once: it asserts the current behaviour is **what the rewrite intends**,
and it documents _why_ by naming the parser asymmetry and the canonical-form rationale. A future
parser change in either direction (language parser stops lowercasing, or tags parser starts) fails
this test and forces a conscious decision about whether the new behaviour is desired.

### 3. Co-locate the explanation with the code, not just the test

The user-visible behaviour is in `update_pattern`. A reader inspecting the preserve branch needs the
asymmetry rationale in-line, not buried in a test file three layers deeper. Add an in-tree comment
at the preserve branch:

```rust
// Intentional asymmetry with `tags`'s preserve branch: the language
// parser lowercases tokens at read time, so a body-only update against
// an existing `language: [Rust]` file rewrites the line as
// `language: [rust]`. The DB has stored the lowercased form since the
// parser landed; the file now converges to match rather than drifting.
// `tags`'s preserve path keeps the original casing because the tags
// parser does not lowercase — language tokens are validated against a
// canonical table where lowercase IS the canonical form, tags are free-form.
// Pinned by `update_pattern_preserve_lowercases_existing_mixed_case_language`.
```

Comment + test together: a reader finds the _why_ at the code, and the _guard_ at the test.

## Why This Matters

The class of bug this guards against is _the future review where someone reads the asymmetry as a
regression_. Without the pin, that review path is:

1. Reviewer reads the preserve branch, notices lowercase divergence from `tags`.
2. Reviewer flags as "inconsistent with `tags` precedent — should preserve verbatim."
3. A well-meaning fix re-introduces case-preservation for the language preserve branch.
4. Now the file's `language: [Rust]` no longer matches the DB's lowercased `["rust"]` after a
   body-only update — and the retrieval gate's structural-match-on-DB-lowercased starts missing
   patterns that look declared on disk.

The pin breaks the cycle: the regression test names the asymmetry as intentional, so the
well-meaning fix surfaces as a failing assertion that forces a conscious "do we still want this
asymmetry?" conversation. The intentional asymmetry survives review cycles because it's documented
_as_ intentional, not implied.

The deeper observation is that **parser normalisation is a leaky abstraction for preserve
semantics.** Preserve-branch behaviour is determined by the parser, not by the preserve branch's own
code. Pattern this means: review the parser before reviewing the preserve branch, and pin the
parser's contribution to the preserve branch's behaviour as a contract.

## When to Apply

Apply this practice when:

- A new field gains preserve-on-`None` semantics on a serialised list
- A sibling field with the same semantics already exists, and you are tempted to "copy the pattern"
- The parser for the new field normalises (lowercase, NFC, sort, dedup) and the sibling parser does
  not, or vice versa
- A correctness review flags the preserve branch as "inconsistent with the sibling field" — the
  question to answer is _is the asymmetry intentional or accidental?_

Skip when:

- Both sibling fields use parsers with identical normalisation posture (genuine symmetry)
- The new field has no canonical form distinct from the user's raw input — preserve really does mean
  verbatim, the parser is a passthrough
- The field is so short-lived that pinning is overhead

## Examples

### lore PR #63 — `language` vs `tags` preserve-branch asymmetry

| Field      | Parser canonicalises?                                              | Preserve branch behaviour               |
| ---------- | ------------------------------------------------------------------ | --------------------------------------- |
| `tags`     | No (`parse_frontmatter_tag_list` is verbatim-with-quote-stripping) | Byte-stable on preserve                 |
| `language` | Yes — lowercases against canonical `LANGUAGES` table               | Converges to canonical form on preserve |

- **Pin:** `update_pattern_preserve_lowercases_existing_mixed_case_language` in
  `src/ingest.rs::tests` asserts that a file with `language: [Rust, Kotlin]` rewrites to
  `language: [rust, kotlin]` after a body-only update.
- **In-tree comment:** the preserve branch in `update_pattern` carries a 6-line comment naming the
  asymmetry, the rationale (canonical-form for `LANGUAGES` lookups, DB has stored lowercased since
  PR #50), and the pinning test by name.
- **Reviewer expectation:** a future correctness review that flags the asymmetry surfaces the
  in-tree comment first, then the pinning test — both name the rationale before the reviewer can
  propose a "fix."

### Generic template

Whenever two sibling fields share three-way semantics on a list:

```
1. Read both parsers. Note any transformation each applies (case, NFC, trim, sort, dedup).
2. If the transformations differ, the preserve branches will diverge — find the divergence point.
3. Ask: "is the diverging field's parser transformation the canonical form?"
   - Yes → intentional asymmetry. Pin with a regression test and an in-tree comment naming
     the rationale.
   - No → accidental asymmetry. Fix the parser to match the sibling, or accept the divergence
     with a documented rationale.
4. If intentional: co-locate the rationale at the preserve branch (comment) and the pinning
   test (regression guard). Both are required — the comment without the pin drifts; the pin
   without the comment confuses future readers.
```

## Related

- [`round-trip-discriminator-canonicalise-both-sides-2026-05-10.md`](round-trip-discriminator-canonicalise-both-sides-2026-05-10.md)
  — adjacent design pattern on the _other_ direction of canonicalisation: discriminator equality
  comparisons require **symmetric** canonicalisation on both input and stored sides. This doc says
  "asymmetric canonicalisation can be intentional in preserve branches"; that doc says "asymmetric
  canonicalisation in discriminators is a bug." Both apply to the same parser; the question is what
  surface the parser feeds.
- `docs/plans/2026-05-19-001-feat-mcp-language-arg-plan.md` — the plan whose correctness review
  surfaced this asymmetry. The Risk Analysis section's R1 (originally a SHOULD-FIX) was resolved by
  pinning the intent rather than fixing it.
- `src/ingest.rs::update_pattern` preserve branch — the in-tree comment carrying the rationale.
- `src/ingest.rs::tests::update_pattern_preserve_lowercases_existing_mixed_case_language` — the
  pinning test.
