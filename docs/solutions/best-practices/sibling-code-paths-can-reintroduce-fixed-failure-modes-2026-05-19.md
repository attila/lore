---
title: "Sibling code paths can silently reintroduce the failure mode a fix exists to address"
date: 2026-05-19
category: best-practices
module: mcp-server
problem_type: best_practice
component: tooling
severity: high
applies_when:
  - "Shipping a fix for a specific failure mode (dropped argument, silent error, lost advisory) on one of several sibling code paths"
  - "Reviewing a plan whose risk section labels a related gap as 'known limitation, follow-up'"
  - "A feature ships on the primary code path while a fast-path, short-circuit, or alternate-mode path is left out of scope"
  - "Triaging residuals after planning to decide which belong in the current PR vs a follow-up"
tags:
  - residual-triage
  - feature-parity
  - sibling-paths
  - dropped-argument
  - scope-discipline
  - adversarial-review
---

# Sibling code paths can silently reintroduce the failure mode a fix exists to address

## Context

PR #63 added a `language` argument to the MCP pattern-authoring tools because the existing tools
silently dropped the argument — the dominant failure mode it exists to fix is _"agent passes
`language`, nothing happens, no signal."_

During planning, an adversarial review flagged that the **inbox-branch short-circuit** (used when
`config.inbox_branch_prefix` is set — the most common agent-submission path in production) bypasses
`index_single_file` entirely. That meant the chunking parser's unknown-language-token advisories
never fired for inbox-branch writes: `WriteResult.language_warnings` stayed empty even when the
caller passed an unknown token. The initial plan-time response was to label this as R4 — _known
limitation, follow-up PR._

The user redirected: _"I don't want followups, talk me through both residuals."_ On the technical
walkthrough side-by-side with the rest of the work, the conclusion became obvious: the inbox-branch
path is the **same failure mode the feature exists to fix**, just on a different code path. Leaving
it out would mean the dropped-argument bug _reappears at MCP runtime_ for the most common path —
silently, with no agent-observable signal.

The fix folded into U3: each short-circuit (add, update, append) invokes
`parse_frontmatter_language_list` directly on the about-to-be-written content, the shared
`collect_language_warnings` helper emits one stderr line per unique unknown token, and
`WriteResult.language_warnings` matches what the local-write path would have produced. Closed in the
same diff. Integration tests in `tests/branch_push.rs` pin both paths.

## Guidance

When triaging a "follow-up" candidate after planning, ask one question:

> Is the limitation I'm about to defer **the same shape** as the failure mode the current PR is
> fixing?

If yes, it is not a follow-up. It is **incomplete scope**. Fold it into the current PR.

Three concrete tests for "same shape":

### 1. Identify the failure mode by its agent-observable signal

What does the current PR's failure mode look like _from the consumer's seat_? In PR #63 it was:
_agent calls a tool with `language`, nothing happens, no signal in the response or stderr._ That
signal — silent argument drop — is the canonical form. Now scan every code path the consumer can
reach and ask: _would this signal also appear here, in the same shape?_

For PR #63: inbox-branch path → agent calls `add_pattern` with `language: ["objectiv-c"]`,
short-circuit pushes to a remote branch, returns `language_warnings: []`. Same canonical signal.
Same bug.

### 2. Audit sibling code paths against the contract the fix establishes

Whenever a fix establishes a new contract on a public surface (a metadata field that's "always
present", a stderr line that "always fires for unknown tokens"), find every code path that returns
through that surface and verify the contract holds. The trace is mechanical:

| Path                               | Returns `WriteResult` | Invokes `index_single_file`? | Surfaces advisories?      |
| ---------------------------------- | --------------------- | ---------------------------- | ------------------------- |
| `add_pattern` (local)              | yes                   | yes                          | yes via parser advisories |
| `add_pattern` (inbox branch)       | yes                   | **no** — short-circuits      | **no — gap**              |
| `update_pattern` (local)           | yes                   | yes                          | yes                       |
| `update_pattern` (inbox branch)    | yes                   | **no** — short-circuits      | **no — gap**              |
| `append_to_pattern` (local)        | yes                   | yes                          | yes                       |
| `append_to_pattern` (inbox branch) | yes                   | **no** — short-circuits      | **no — gap**              |

The grid makes the gap visible. Every "no" cell is a sibling-path failure-mode reappearance.

### 3. Cost-check: is the fix cheap enough to fold in?

Sibling-path fixes are often surprisingly cheap because the canonical implementation already exists
— the sibling path just needs to call the same helper. In PR #63: `parse_frontmatter_language_list`
is a pure function (`&str, &str` in, advisories out, no DB, no embedder, no I/O); each short-circuit
needed roughly 10 lines (parse + collect + threading through `WriteResult`). One shared helper
covered all three sites.

If the fold-in is genuinely expensive (requires a schema change, a new dependency, a protocol
revision), then the follow-up label may be correct — but write a hazard-pin test for the current gap
so the next refactor surfaces it. See
[`composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`](composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md)
for the hazard-pin pattern when the real fix is genuinely out of scope.

## Why This Matters

The plan-time framing of "known limitation, follow-up PR" is correct sometimes and seductive always.
It feels disciplined — _I noticed the gap, I called it out, I'll fix it later._ That framing hides
one specific failure mode: **the residual is the same failure mode the current PR exists to fix.**
When that's true, "follow-up" means _ship the PR that's supposed to close the bug while leaving the
bug open on the dominant path._

The cost asymmetry is what makes this worth catching:

- **In the current PR:** ~10 lines per sibling path, one shared helper, one or two extra test
  scenarios. The reviewer has the full context. The bug never ships.
- **In a follow-up PR:** a new branch, a new PR, a new review round, a new round of plan
  documentation, and — between landing and the follow-up — agents in production hit the bug the
  current PR claimed to fix.

The deeper lesson is that **a fix on one code path establishes a new contract on the public surface,
not on that code path.** If `WriteResult.language_warnings` is "always present, always populated
with unknown tokens", then every `WriteResult`-producing code path owes that contract. The sibling
path didn't suddenly grow a bug; it was always broken — the new contract just made the brokenness
visible.

## When to Apply

Apply this check whenever:

- The current PR fixes a specific class of failure mode (dropped argument, silent error, lost
  advisory, missing observability) on a tool, endpoint, or public surface
- The fix establishes a new contract on the response shape (a field that's "always present", a log
  line that "always fires", a side effect that "always happens")
- The system has fast-path, short-circuit, alternate-mode, or batch-mode code paths that produce the
  same response shape via different internal logic
- An adversarial review or plan-time risk section labels a related gap as "follow-up" or "known
  limitation"

Skip when:

- The "sibling path" is genuinely a different feature with a different contract
- The fold-in requires a schema change, protocol revision, or breaking change that genuinely belongs
  in a separate PR
- The current PR is already large and the sibling-path fix can land as the literal next commit on
  the same branch within a day

## Examples

### lore PR #63 — inbox-branch language advisory parity

- **Primary fix:** `add_pattern` / `update_pattern` accept `language`, write canonical frontmatter,
  surface unknown tokens via stderr + `WriteResult.language_warnings`.
- **Sibling-path gap (initially R4 → fold-in U3):** the inbox-branch short-circuit in each of the
  three pattern-authoring functions skips `index_single_file`, so unknown-language tokens never
  surface on the agent-submission path. Same canonical failure mode as the primary fix.
- **Fold-in:** ~10 lines per short-circuit, one shared `collect_language_warnings` helper, three
  integration tests in `tests/branch_push.rs`. R4 deleted from the risk section; documented as
  closed.

### Generic template for the audit

When reviewing a PR that fixes a specific failure mode on a public surface:

```
1. Name the failure mode by its agent-observable signal.
2. Enumerate every code path that returns through that surface.
3. For each path, ask: "if a consumer triggers the same input that the primary fix addresses,
   does this path produce the same observable signal as the fixed primary path?"
4. If the answer is "no" — the sibling path silently reintroduces the failure mode.
5. Fold the fix into the current PR unless the fold-in is genuinely expensive; otherwise apply
   the hazard-pin pattern from composition-cascades-….md.
```

## Related

- [`composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`](composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md)
  — adjacent lesson on **inter-path** composition (a new path + an existing reconciliation pass).
  This doc covers **intra-feature** parity (one feature shipped only on some of its sibling code
  paths). Same family of "feature shape doesn't survive across all code paths"; different specific
  shape.
- [`mcp-metadata-via-fenced-content-block-2026-04-07.md`](mcp-metadata-via-fenced-content-block-2026-04-07.md)
  — the metadata-fence contract this PR's `language_warnings` extends. The new field's "always
  present" property is the new contract whose sibling-path coverage this lesson is about.
- `docs/plans/2026-05-19-001-feat-mcp-language-arg-plan.md` — the plan whose R4 → U3 fold-in is the
  concrete example. The Risk Analysis section before the rewrite labelled the inbox-branch gap as
  "known limitation"; after the user's redirection it became "closed inside U3."
- `tests/branch_push.rs::inbox_add_pattern_collects_unknown_language_tokens` and the two sibling
  tests around it — the integration tests that pin the closed gap.
