---
title: "Extract docs/configuration.md#git-integration to a module-level constant"
priority: P2
category: maintainability
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/main.rs:155
  - src/hook.rs:439
  - tests/smoke.rs (advisory assertion)
related_pr: feat/git-optional-knowledge-base
---

# Extract docs/configuration.md#git-integration to a module-level constant

## Context

The string `docs/configuration.md#git-integration` appears as a hardcoded literal in three locations
introduced by this branch:

1. `src/main.rs:155` — the cmd_init advisory printed to stderr
2. `src/hook.rs:format_session_context` — the SessionStart notice (mentions the section by name
   without the full path)
3. `tests/smoke.rs` — the test assertion that checks the advisory contains the path

If the documentation file is renamed (e.g. moved to `docs/git-integration.md`) or the anchor
changes, three references break and only the test catches one of them. Two would fail silently — the
agent advisory text would point at a nonexistent doc.

## Proposed fix

Add a `pub(crate) const` somewhere central. Options:

```rust
// in src/lib.rs or a new src/constants.rs
pub(crate) const DOCS_GIT_INTEGRATION: &str = "docs/configuration.md#git-integration";
```

Then replace the three call sites:

```rust
// src/main.rs
eprintln!("  them. See {} for details.\n", crate::DOCS_GIT_INTEGRATION);

// src/hook.rs (SessionStart notice)
// (currently mentions the doc only by section name; consider including
// the full path here too via the same constant for consistency)

// tests/smoke.rs
assert!(
    stderr.contains(lore::DOCS_GIT_INTEGRATION),
    "expected advisory to point at the documentation reference, got: {stderr}"
);
```

The README link to `docs/configuration.md#git-integration` is a markdown link, not a code reference,
so it stays as-is. dprint will fail on broken links if the docs are moved without updating the
README.

## References

- Maintainability finding (confidence 0.82)
- Note: this is gated_auto rather than safe_auto because it requires deciding where to put the
  constant (lib.rs vs a new module) and how to expose it to the integration test.
