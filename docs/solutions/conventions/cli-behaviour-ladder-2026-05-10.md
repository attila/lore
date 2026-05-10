---
title: CLI behaviour ladder for edge cases
date: 2026-05-10
category: conventions
module: cli
problem_type: convention
component: tooling
severity: medium
applies_when:
  - Designing the response to a CLI edge case (empty inputs, missing state, partial data, conflicts)
  - Choosing between exit 1, exit 0 with stderr, or silent success
  - Reviewing a brainstorm or PR that proposes a new failure mode or opt-out flag
  - Re-evaluating an existing hard-fail to check whether warn-only would serve users better
related_components:
  - development_workflow
  - documentation
tags:
  - cli-design
  - error-handling
  - edge-cases
  - user-experience
  - exit-codes
  - decision-framework
---

# CLI behaviour ladder for edge cases

## Context

`lore` is a CLI that does two things on every run: it ingests patterns from a knowledge directory
and answers semantic search queries. Both surfaces hit edge cases — empty directories, slug
collisions, lossy filenames, repos with no `HEAD`, non-git working copies — and each one needs a
deliberate response. Early on, the slug-collision design landed on a sharp principle: **loud
failures over silent recovery**. Slugs collide when two patterns would write to the same
destination, which silently destroys one of them; failing loudly was clearly right.

The mistake was generalising that principle. When the empty-knowledge-dir brainstorm landed on
`feat/empty-knowledge-dir-validation`, it inherited the same posture: fail-fast on an
effectively-empty directory, with `--allow-empty-knowledge` as the opt-in escape hatch.
Mid-brainstorm, on a "genuine take" prompt, that design got pushed back on: an empty knowledge
directory is a coherent, recoverable state, not data destruction. Forcing a hard fail (and then
immediately weakening it with a silencer flag) was over-failing, and the silencer flag itself
recreated the silent-failure mode the warning was supposed to surface.

The fix wasn't a different design for one feature — it was articulating the **classification step**
that should precede every CLI edge-case decision. Before picking a response, ask which of three
tiers the case belongs to. The slug-collision principle is real, but it's a tier-1 principle, not a
universal one.

## Guidance

Every CLI edge-case response in `lore` belongs to exactly one of three tiers. Run the candidate
response through all three tier-tests and pick the **lowest** tier whose test returns yes.

**Tier 1 — Hard fail (exit 1)**

- _Definition:_ continuing would destroy data, commit a wrong choice on the user's behalf, or leave
  the system unrecoverable.
- _Test:_ "Would the user be angry at lore for succeeding silently here?"
- _lore examples:_ slug collisions (`src/ingest.rs:add_pattern` returns a distinct error naming the
  colliding slug); malformed config; conflicting writes.

**Tier 2 — Warn (exit 0, stderr)**

- _Definition:_ continuing produces a coherent but possibly-unintended result; the user can
  course-correct on the next run with no rework.
- _Test:_ "Would the user be confused if lore succeeded silently here, because the result tells them
  nothing about why it's empty or odd?"
- _lore examples:_ effective-empty knowledge directory (`src/ingest.rs::effective_scan_state` +
  `empty_warning_message`, emitted via `on_progress`); lossy filenames in `discover_md_files`;
  no-`HEAD` git repos; arguably the missing-git fallback.

**Tier 3 — Silent success**

- _Definition:_ the state is normal and unsurprising.
- _Test:_ "Is this state common enough that warning would erode trust in real warnings?"
- _lore examples:_ non-git working directory; routine zero-changes delta ingest; ingest on an
  already-populated repo.

PR #41 is the worked example: the design moved from tier-1-with-opt-out to tier-2-no-flag once the
ladder was applied. The autofix in `effective_scan_state` and the warning string in
`empty_warning_message` are the concrete artefacts; there is deliberately **no**
`--allow-empty-knowledge` flag.

## Why This Matters

Three named failure modes the ladder protects against:

- **Tier creep.** Warning on routine states erodes signal-to-noise, and tier-1 hard-fails inherit
  the same skepticism. Once warnings start firing on healthy runs, real warnings get filtered out by
  habit. _(auto memory [claude])_
- **Auto-opt-out flags as foot-guns.** `--allow-X` silencers train users to add them once and never
  look back, masking different X-shaped failures later. The empty-dir case made this concrete: a
  flag introduced to silence "you have no patterns" would also silence "you accidentally pointed
  lore at the wrong directory." That recreates the silent-failure problem the warning was designed
  to fix. _(auto memory [claude])_
- **Hard-fail-with-opt-out is heavier than warn-only.** The opt-out flag becomes load-bearing
  infrastructure: renaming or removing it is a breaking change, and every release has to document
  it. Warn-only keeps the API surface clean and reversible. _(auto memory [claude])_

The cost of getting tier assignment wrong is asymmetric. Tier-1-when-tier-2 is the empty-dir trap:
hard-fail plus silencer flag plus future breakage. Tier-3-when-tier-2 is the slug-collision trap:
silent overwrite, lost work, angry user. The ladder forces the right question per case — _"what does
continuing cost?"_ — instead of defaulting to "make it loud."

## When to Apply

Run the ladder when:

- Designing a new CLI feature whose behaviour depends on an edge case (empty input, malformed input,
  missing dependency, ambiguous match).
- Reviewing a brainstorm, plan, or PR description that uses the words `error`, `abort`, `fail`, or
  `reject` for an edge case — challenge whether tier-2 would actually suffice.
- Considering an `--allow-X`, `--force`, or `--ignore-X` flag. The flag is a smell: if tier-2 would
  have handled the case, the flag is unnecessary; if tier-1 is genuinely correct, the flag
  re-enables the failure mode you just diagnosed.
- Designing any user-facing surface that can return a coherent-but-empty result (zero hits, zero
  patterns, zero changes). These almost always belong in tier 2 or tier 3, never tier 1.
- Generalising a principle from one feature to another. The slug-collision case generalised badly;
  assume yours will too until you've checked.

Resist adding silencer flags pre-emptively. Wait for a concrete user report.

## Examples

**Tier 1 — slug collisions** (`src/ingest.rs:add_pattern`)

Two source files would resolve to the same slug, meaning one would overwrite the other in the index.
`add_pattern` returns a distinct error naming the colliding slug and the offending paths; ingest
aborts with exit 1.

```rust
// pseudocode shape
if existing_slug == new_slug {
    return Err(IngestError::SlugCollision { slug, existing, incoming });
}
```

The tier test answers itself: a user who ran `lore ingest` and silently lost a pattern would be
furious. Hard fail is correct.

**Tier 2 — effective-empty knowledge directory** (PR #41)

The directory exists and is readable, but contains nothing lore can index — no markdown, or only
filtered/ignored files. The original brainstorm said: error out, with `--allow-empty-knowledge` for
users who genuinely meant it. Applying the ladder flipped this:

- `src/ingest.rs::effective_scan_state` classifies the scan result.
- `empty_warning_message` builds the human-readable warning.
- `ingest()` emits it via `on_progress` and returns `Ok(())` with an empty index.
- No flag. Exit 0.

The user gets a coherent end state (an empty index), a clear explanation on stderr, and the ability
to fix it on the next run by pointing at the right directory or adding patterns. The auto-opt-out
trap is specifically avoided: there is no flag to add to a script and forget about.

**Tier 3 — non-git working directory**

`lore` works in plain directories; git is an enrichment, not a requirement. Running in a non-git dir
is common and unsurprising. Warning on it would fire on a large fraction of legitimate runs,
training users to ignore lore's stderr. Stay silent; reserve warnings for the cases where they carry
signal.

## Related

- **PR #41** (`feat/empty-knowledge-dir-validation`) — the worked example; the pivot from fail-fast
  to warn-only is what surfaced the ladder.
- **`feat/edge-case-handling`** — upcoming slices that should be classified through the ladder
  before implementation:
  - NFC filename normalisation (likely tier 2: warn on non-NFC, continue).
  - No-`HEAD` git repos in the ingest progress line (tier 2: warn, fall back).
  - Lossy-path warning in `discover_md_files` (tier 2).
  - Missing-git regression test (codifies the existing tier-3 silent fallback).
- [`docs/solutions/best-practices/cli-suppress-stderr-in-json-mode-2026-04-03.md`](../best-practices/cli-suppress-stderr-in-json-mode-2026-04-03.md)
  — sibling CLI-output rule. The ladder governs _whether_ a diagnostic fires; this doc governs
  _whether stderr is the right channel_ under structured-output mode.
- [`docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md`](../best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md)
  — the foundational stdout/stderr split the ladder builds on top of.
- [`docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`](../best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md)
  — example of a tier-1 (hard-fail) path with a documented remedy; complementary discipline for
  cases where hard-fail is the right tier.
- Project memory `~/.claude/projects/-srv-misc-Projects-lore/memory/project_cli_behaviour_ladder.md`
  — the originating note. This doc is the authoritative codebase-side reference; the memory remains
  a personal/path-specific note.
