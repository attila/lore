---
title: "bin and lib are separate crates for #[non_exhaustive] semantics"
date: 2026-06-04
category: best-practices
module: crate-structure
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "A Cargo package has both a `src/lib.rs` and `src/main.rs` (or other `[[bin]]` targets)"
  - "Adding `#[non_exhaustive]` to a public enum or struct in the library"
  - "Refactoring an internal type to a public one in preparation for future variants"
  - "Reading a confusing 'unreachable pattern' warning on a wildcard arm in a same-crate match"
tags:
  - rust
  - non-exhaustive
  - cargo
  - crate-boundaries
  - lib-bin
  - pattern-matching
---

# bin and lib are separate crates for #[non_exhaustive] semantics

## Context

PR #67 added `#[non_exhaustive]` to a public enum (`ProbeOutcome`) defined in `src/lib.rs`,
following the standard advice that the attribute prevents downstream breakage when variants are
added later. Inside the library — `src/server.rs`, which is in the same module tree as the enum — a
`match` over the four known variants compiled cleanly without a wildcard arm. Inside `src/main.rs`,
the same `match` failed to compile:

```
error[E0004]: non-exhaustive patterns
   |
   = note: ProbeOutcome is marked as non-exhaustive, so a wildcard `_` is necessary
   = note: the matched value is of type `&ProbeOutcome`
```

The expectation was that `#[non_exhaustive]` only affected downstream crates and that both
`src/lib.rs` and `src/main.rs` were "inside the same crate" because they live in the same Cargo
package. They don't. From Cargo's perspective, a package with a library target and one or more
binary targets compiles as multiple crates: one library crate (`lib`) and one binary crate per
`[[bin]]` target (each named after the package). The binary crate depends on the library crate the
same way any external dependency would.

So `src/main.rs` is downstream of `src/lib.rs`, and `#[non_exhaustive]` enums imported from
`use lore::provision::ProbeOutcome;` (the explicit-extern path) trigger the wildcard requirement in
`match` arms.

Inside `src/lib.rs` and its module tree, the same enum's variants are all visible, the
non-exhaustive marker has no effect, and the compiler treats an explicit wildcard as
`unreachable_patterns`.

## Guidance

When adding `#[non_exhaustive]` to a public enum in a package that also has a binary target, treat
the bin and lib as separate crates for pattern-matching purposes:

- **Inside the library** (`src/lib.rs` and the modules it owns): match all known variants
  explicitly. Do not add a wildcard arm — the compiler will warn about it as unreachable, and adding
  a new variant inside the lib will produce a desired non-exhaustive-match error at the
  defining-crate boundary rather than silently routing to the wildcard.
- **Inside any binary** (`src/main.rs`, `examples/*.rs`, integration tests): match all known
  variants explicitly **and** add a wildcard arm with a sensible default. The wildcard is the bin's
  only protection against the lib growing a new variant that the bin doesn't yet render.

The two sites of the same `match` look slightly different on purpose: the lib version is exhaustive
without wildcard, the bin version is exhaustive with wildcard. This asymmetry is the correct shape,
not a stylistic inconsistency.

## Why This Matters

The Rust reference's wording on `#[non_exhaustive]` says "no effect within the defining crate" but
defines "crate" by compilation unit, not by Cargo package. The natural reading of "I'm in the same
package, so I'm in the same crate" is wrong for any package that ships both a library and one or
more binaries.

The trap surfaces in three specific ways:

- **Confusing compile error.** `error[E0004]` on a `match` over a same-package enum makes the
  developer reach for "did I miss a variant?" rather than "the enum is annotated non-exhaustive
  across this boundary."
- **Inconsistent wildcard requirements.** The same `match`-on-the-same-enum compiles in one module
  without a wildcard and errors in another module — even when both modules look superficially
  "internal."
- **Silent regression risk on the bin side.** A developer who removes the wildcard arm to silence an
  `unreachable_patterns` warning during refactoring removes the bin's only forward-compatibility
  surface. The next added variant compiles in the lib (the lib match catches it as non-exhaustive)
  but the bin still compiles too — until a runtime call hits the new variant and falls through to
  whatever Rust does with no matching arm (which is to say: the bin must be matching exhaustively or
  have a wildcard, so this case only arises if the bin removed the wildcard prematurely).

The fix is mechanical once the lib-vs-bin distinction is internalised: a public enum that wants to
be forward-compatible across both internal callers (the lib) and the package's own binaries (the
bin) needs the wildcard arm in the bin sites, not the lib sites.

## When to Apply

Apply this awareness when:

- Adding `#[non_exhaustive]` to a public enum or struct used by both `src/lib.rs` and `src/main.rs`
  in the same package
- Reading an "unreachable pattern" warning on a wildcard arm and feeling tempted to delete it
- Reviewing a PR that adds `#[non_exhaustive]` to an existing public type — verify both sides of the
  boundary updated their matches correctly
- Onboarding to a multi-target Cargo project (one with both lib and bin) and reading inconsistent
  match shapes for the same enum across files

Skip when:

- The package has a library only (single crate, `#[non_exhaustive]` has no internal effect anywhere)
- The package has binaries only (no library, no cross-crate boundary inside the package)
- The annotated type is not pattern-matched in any binary in the package

## Examples

### Lib site (no wildcard — adding the wildcard warns as unreachable)

```rust
// In src/server.rs (lib crate), same module tree as ProbeOutcome.
fn runtime_outcome_to_json(outcome: &ProbeOutcome) -> Value {
    match outcome {
        ProbeOutcome::Ok => json!({ "state": "ok" }),
        ProbeOutcome::NotChecked => json!({ "state": "not_checked" }),
        ProbeOutcome::Skipped => json!({ "state": "skipped" }),
        ProbeOutcome::Failed(err) => json!({ "state": "failed", "...": "..." }),
        // No `_ => ...` arm. Adding one emits:
        // warning: unreachable pattern
    }
}
```

### Bin site (wildcard required — adding a new variant in lib compiles, this is the bin's safety net)

```rust
// In src/main.rs (bin crate), downstream of the lib that defines ProbeOutcome.
fn render_runtime_line(outcome: &ProbeOutcome, full: bool) {
    match outcome {
        ProbeOutcome::Ok => eprintln!("  Runtime:      ✓ inference OK"),
        ProbeOutcome::NotChecked => eprintln!("  Runtime:      —  (run --full ...)"),
        ProbeOutcome::Skipped => { /* ... */ }
        ProbeOutcome::Failed(err) => { /* ... */ }
        // ProbeOutcome is `#[non_exhaustive]` from this crate's perspective;
        // future variants render as a neutral em-dash until the renderer is updated.
        _ => eprintln!("  Runtime:      —"),
    }
}
```

### The asymmetry seen side by side

```text
src/lib.rs       defines  pub enum ProbeOutcome { Ok, NotChecked, Skipped, Failed(_) }
                          #[non_exhaustive]

src/server.rs    match    Ok | NotChecked | Skipped | Failed(_)
                          (no wildcard — adding one warns unreachable)

src/main.rs      match    Ok | NotChecked | Skipped | Failed(_) | _
                          (wildcard required — non_exhaustive applies across crate boundary)
```

Both `match`es are correct for their site. The bin's extra arm isn't a style choice; it's the
boundary contract.

## Related

- [The Rust Reference — `#[non_exhaustive]`](https://doc.rust-lang.org/reference/attributes/type_system.html#the-non_exhaustive-attribute)
  — defines the attribute's effect in terms of crate boundaries; "crate" is per compilation unit,
  not per Cargo package.
- [The Cargo Book — Package layout](https://doc.rust-lang.org/cargo/guide/project-layout.html) —
  confirms that `src/lib.rs` and `src/main.rs` produce separate crates (the library crate and the
  binary crate of the same name).
- `src/lib.rs`, `src/main.rs`, `src/provision.rs`, `src/server.rs` in lore — all four sites of the
  `ProbeOutcome` pattern this lesson generalises.
