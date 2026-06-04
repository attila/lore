---
title: "Multi-surface render function pattern — one match, N user-facing surfaces"
date: 2026-06-04
category: design-patterns
module: error-rendering
problem_type: design_pattern
component: tooling
severity: medium
applies_when:
  - "An enum (or other classification) needs to render differently across two or more user-facing surfaces (CLI line, log line, JSON metadata, hook warning, HTTP response)"
  - "Adding a new variant must update every surface or risk drift between them"
  - "A reviewer flags that 'these four match arms need to stay in sync' as a future maintenance burden"
  - "Designing a structured error type that surfaces through multiple rendering paths"
tags:
  - rust
  - error-rendering
  - design-pattern
  - enum-dispatch
  - dry
  - multi-surface
---

# Multi-surface render function pattern — one match, N user-facing surfaces

## Context

PR #67 introduced `ProbeError` — a structured error from probing Ollama inference — that needed to
render across four user-facing surfaces:

- The CLI `Runtime:` status line in `lore status --full`
- The `result.errors` entry on `ProvisionResult`
- The `result.actions` (remediation hint) entry on the same struct
- The hook FTS-fallback warning's short-reason token

The naive shape was a separate `match` over `ProbeError` at each callsite, with each match producing
the surface-specific string. A reviewer flagged the obvious risk: adding a fifth variant means
updating four match arms, all in different files, and any one of them silently falling through to a
default string is a drift bug that ships unnoticed.

The chosen shape was a single `render_failure(&ProbeError) -> RenderedFailure` function — one match,
one struct with one field per surface, and every callsite reads the field it cares about. Adding a
variant touches exactly one match arm, and the compiler enforces that the new variant produces
strings for every existing surface in the same change.

A learnings-researcher pass during code review noted no prior art for this pattern in the project's
`docs/solutions/`.

## Guidance

When an enum needs to render across N user-facing surfaces, centralise the variant-to-strings match
in one function that returns an N-field struct:

```rust
pub struct RenderedFailure {
    pub short_reason: String, // hook warning
    pub status_line: String,  // CLI Runtime line
    pub error_line: String,   // result.errors entry
    pub action_line: String,  // result.actions entry
}

pub fn render_failure(err: &ProbeError) -> RenderedFailure {
    match err {
        ProbeError::RunnerFailed { body, .. } | ProbeError::InferenceError { body } => {
            let body_msg = extract_body_message(body);
            RenderedFailure {
                short_reason: "inference error".to_string(),
                status_line: format!("inference failed{body_msg}"),
                error_line: format!("Ollama reached the model but ...{body_msg}"),
                action_line: "Check 'ollama serve' logs ...".to_string(),
            }
        }
        ProbeError::Timeout => RenderedFailure { /* ... */ },
        ProbeError::Transport(msg) => RenderedFailure { /* ... */ },
        ProbeError::HttpStatus { status, .. } => RenderedFailure { /* ... */ },
    }
}
```

Then each surface site consumes the field it cares about:

```rust
// CLI Runtime line
eprintln!("  Runtime:      ✗ {}", render_failure(err).status_line);

// Provision result
result.errors.push(render_failure(err).error_line);
result.actions.push(render_failure(err).action_line);

// Hook warning short reason
let short = render_failure(err).short_reason;
```

The struct is the contract. Each callsite knows only the field name, not the variant-to-string
mapping. Adding a fifth variant requires extending the central match — the compiler enforces it
because the enum match is exhaustive and the struct's fields are all required.

## Why This Matters

Without centralisation, four files have to stay in sync. With centralisation, one file does.

The cost of the alternative — a separate match at each surface — looks small at PR-time and grows
unbounded over a project's lifetime:

- **Discoverability of every site.** Adding a variant requires grepping for every
  `match ... ProbeError::` in the codebase. Miss one and that surface silently falls through with a
  default string or fails to compile in a hard-to-localise way.
- **Inconsistent rendering of the same failure.** Surface A might call the variant "inference
  error"; surface B might call it "embed failed"; surface C might omit the body entirely. The user
  sees three names for the same condition and has to mentally reconcile them.
- **Refactoring resistance.** Renaming a variant means updating N matches; reordering string
  formatting means N edits. Each one is a chance to introduce a typo or a mismatch.

The centralised function inverts the cost: the act of adding a variant forces the author to think
about every surface at once, in one match arm. The struct's field names function as a checklist —
you cannot add a `ProbeError::FifthVariant` and ship without giving it a `status_line` and an
`error_line` and an `action_line` and a `short_reason`.

The pattern also defends the discoverability rule when the rendering grows complex. Body extraction
with truncation and JSON-error-field detection (see `extract_body_message` in the same module) is
shared logic between the `status_line` and `error_line` callers; centralising the renderer means
that helper is called from one place, not four.

## When to Apply

Apply this pattern when:

- An enum (or other classification) needs to render strings to ≥2 user-facing surfaces
- The variant-to-string mapping is non-trivial (longer than a one-line `Display` impl, or involves
  shared sub-helpers like body extraction)
- The surfaces are stable enough that adding a new surface is rare relative to adding a new variant
- The codebase already has the discoverability rule that "adding a variant should touch one obvious
  match arm, not several"

Consider lighter alternatives when:

- Only one surface needs strings — a `Display` impl is sufficient
- The mapping is genuinely one-line per variant per surface — independent `match`es are still cheap,
  and the abstraction overhead may not be worth it
- A surface needs structured access (e.g., a `severity` enum) rather than a string — the pattern
  still applies but the struct field type changes from `String` to whatever surface- specific shape

Avoid this pattern when:

- The surfaces are owned by different teams or different crates with conflicting iteration cycles —
  the centraliser then becomes a coordination bottleneck. In that case, each consumer-side `match`
  is the right shape because each consumer absorbs its own variant- update cost.

## Examples

### Adding a variant — one match arm covers every surface

```rust
// Step 1: add the variant.
pub enum ProbeError {
    // ... existing variants ...
    RateLimited { retry_after_secs: u64 },
}

// Step 2: render_failure won't compile until the variant has an arm.
pub fn render_failure(err: &ProbeError) -> RenderedFailure {
    match err {
        // ... existing arms ...
        ProbeError::RateLimited { retry_after_secs } => RenderedFailure {
            short_reason: "rate limited".to_string(),
            status_line: format!("rate limited — retry after {retry_after_secs}s"),
            error_line: format!("Ollama rate-limited the probe ({retry_after_secs}s)"),
            action_line: "Wait for the rate limit window to expire, then retry.".to_string(),
        },
    }
}
```

Every callsite (`status_line` for the CLI, `error_line` for `result.errors`, `short_reason` for the
hook, etc.) automatically picks up the new variant. No grep, no four-file diff, no risk of the hook
surface silently producing the default while the CLI surface renders correctly.

### Avoiding the trap when adding a surface

Adding a fifth surface — say, an MCP JSON-metadata field — is the one case where the pattern forces
a wider edit. Add the field to `RenderedFailure`, then update every variant arm to populate it. The
compiler will require it because the struct's fields are not optional. This upfront cost is the
price of preventing per-variant drift; it lands in one PR rather than trickling across the codebase
as forgotten arms.

```rust
pub struct RenderedFailure {
    pub short_reason: String,
    pub status_line: String,
    pub error_line: String,
    pub action_line: String,
    pub mcp_state: String, // new field — every arm must provide it
}
```

The compile error after adding the field is the change-list. Work through it and every variant gets
the new surface. This is the correct shape — the field's absence-of-default is the discoverability
mechanism.

## Related

- [`sibling-code-paths-can-reintroduce-fixed-failure-modes-2026-05-19.md`](../best-practices/sibling-code-paths-can-reintroduce-fixed-failure-modes-2026-05-19.md)
  — same family ("a feature shape doesn't survive across all code paths"); this doc is the
  enum-rendering instance, that doc is the inter-path composition instance.
- [`round-trip-discriminator-canonicalise-both-sides-2026-05-10.md`](round-trip-discriminator-canonicalise-both-sides-2026-05-10.md)
  — adjacent pattern: when serialising the same enum, canonicalise its discriminator on both sides
  of the round trip. The renderer pattern here is the consumer-side; canonicalising serialisation is
  the producer-side.
- `src/embeddings.rs` in lore — `render_failure(&ProbeError) -> RenderedFailure` is the reference
  implementation; `RenderedFailure` is declared adjacent and is `#[non_exhaustive]` so adding a
  surface field is itself forward-compatible.
