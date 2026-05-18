---
title: "feat: shared trace-walk predicate; tighten `lore trace prune` to trace data files"
type: feat
status: active
date: 2026-05-16
deepened: 2026-05-17
---

# feat: shared trace-walk predicate; tighten `lore trace prune` to trace data files

## Summary

Codify the trace-walk discipline as a structural invariant in a new `src/trace/walk.rs` submodule
(matching the engine submodule pattern). `TraceStats::compute` and `enumerate_trace_files` delegate
to a single `is_real_trace_file` predicate — regular `.jsonl`/`.jsonl.gz` file, not symlink, not
maintenance state file. `src/trace/query.rs` deliberately retains its current symlink-following
behaviour with a documented rationale comment, since its read-only surface doesn't carry the
symlink-safety argument that drove the filter in the two writers. As a deliberate consequence of
moving the extension filter into the predicate, trace maintenance (both `lore trace prune` and the
silent SessionStart-triggered lazy pass) now skips foreign files (`.tmp`, `.bak`, editor `.swp`,
etc.) older than the retention horizon — they accumulate instead of being swept. Plus a regression
test pinning `.last_pruned_at` exclusion from `total_bytes` and a static-grep CI guard on
`src/trace/stats.rs` with two halves — three negative substring checks (`std::fs::metadata`,
`std::fs::symlink_metadata`, `.metadata(`) plus a positive assertion that `walk::is_real_trace_file`
is invoked.

## Requirements

- R1. A new `src/trace/walk.rs` submodule hosts the shared `is_real_trace_file` predicate that
  decides whether a directory entry is a real trace file (regular `.jsonl`/`.jsonl.gz` file, not a
  symlink, not the maintenance state file). `TraceStats::compute` and `enumerate_trace_files`
  delegate to it; `src/trace/query.rs` deliberately does not delegate (retains `e.metadata()`,
  follows symlinks) and carries an explanatory comment. Existing tests for all three modules
  continue to pass without modification.
- R2. A regression test in `src/trace/stats.rs` pins the invariant that `TraceStats::total_bytes`
  excludes `.last_pruned_at` and that `TraceStats::last_pruned_at` is captured from it.
- R3. `tests/invariants.rs` gains a `src/trace/stats.rs` block with two halves: zero occurrences of
  `std::fs::metadata`, `std::fs::symlink_metadata`, and `.metadata(` in the test-stripped source
  (negative — stats does not stat the filesystem directly); AND at least one occurrence of
  `walk::is_real_trace_file` (positive — stats does delegate to the predicate). Together they codify
  both halves of "stats delegates filesystem classification to `walk::is_real_trace_file`." The
  DirEntry `.metadata()` shape is the most likely negative-side regression form; a future
  contributor removing the delegate-call entirely is the most likely positive-side regression form.
- R4. Trace maintenance's extension-tightening behaviour change (affecting both `lore trace prune`
  and the SessionStart-triggered lazy maintenance pass) is documented in `CHANGELOG.md` under
  `[Unreleased]` `### Changed`. One assertive-voice sentence ending in `(#N)`, with wording that
  names both surfaces.

## Scope Boundaries

- The predicate stays scoped to `src/trace/` via `pub(super)` (or equivalent). No re-export at the
  crate root.
- `src/trace/query.rs` is deliberately excluded from the predicate (rationale: read-only surface, no
  data-safety argument). The exception is documented in code; the U3 invariant pin is per-file
  (stats.rs only) rather than a broader "all trace submodules delegate" assertion.
- Does not address the three other macOS-smoke observations (1, 3, 4) — runbook prose fixes,
  separate.
- Does not extend the smoke runbook to a checked-in CI workflow — parked until a holistic
  test-strategy push.
- Does not introduce a second helper for "non-extension-gated trace entry" (e.g. preserving prune's
  prior catch-all behaviour). Single-predicate design is chosen; the prune behaviour change is
  accepted explicitly.

## Context & Research

### Relevant Code and Patterns

- `src/trace/stats.rs:53-115` — current `TraceStats::compute`. Lines 78-87 are the `.last_pruned_at`
  capture (special behaviour, kept). Lines 88-105 are the filter body that moves into the predicate.
- `src/trace/maintenance.rs:258-292` — current `enumerate_trace_files`. The symlink-filter body to
  extract lives at lines 282-286 (the `symlink_metadata` call through `is_symlink() || !is_file()`
  check). The state-file name check at line 273 moves inside the predicate per R1's "not the
  maintenance state file" clause.
- `src/trace/maintenance.rs:203-205` — compress phase's existing extension check. Removed in U1
  (redundant once the predicate gates extension at enumeration).
- `src/trace/maintenance.rs:230-251` — prune phase. Currently has no extension filter; iterates
  `all` (re-enumerated post-compress) and deletes anything older than the retention horizon. After
  U1 the predicate restricts enumeration to `.jsonl`/`.jsonl.gz` files — this is the user-visible
  behaviour change covered by R4.
- `src/trace/query.rs:103-127` — `list_trace_files_newest_first`. The third trace walker,
  deliberately exempt from the predicate. Uses `e.metadata()` (follows symlinks) and propagates
  `read_dir` errors via `?` — both stay.
- `src/engine/` — closest in-repo precedent for the shared-concern submodule pattern. Engine
  declares `call_context.rs`, `languages.rs`, `predicate.rs`, `query.rs`, `text.rs` as named
  concerns with their own invariants; `src/trace/walk.rs` parallels that shape.
- `tests/invariants.rs:185-204` — pattern for the new stats.rs block. The maintenance pin uses
  `count_substring` after `strip_test_modules`. The new block follows the same shape with three
  substrings instead of two.
- `src/trace/stats.rs:196-214` — existing `populated_trace_dir_reports_counts_and_bytes` test.
  Fixture shape (tempdir + seeded files) reused for U2.
- `src/trace/stats.rs:243-267` — existing
  `session_count_skips_symlinks_for_symmetry_with_maintenance` test. Continues to pass against the
  refactored callers.
- `src/trace/maintenance.rs:533-562` — existing `maintenance_skips_symlinked_entries` test.
  Continues to pass.

### Institutional Learnings

- `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` — the prune hardening is a
  deliberate behaviour change, not an accidental surprise. CHANGELOG entry under `### Changed` is
  the appropriate operator-facing notification.
- `docs/solutions/design-patterns/round-trip-discriminator-canonicalise-both-sides-2026-05-10.md` —
  the pattern the refactor instantiates: when two functions filter the same source set, the filter
  predicate should live in one shared helper, not duplicated.
- `docs/solutions/database-issues/stale-chunks-symlinked-knowledge-dir-2026-05-14.md` — closest
  precedent for "make the function self-symmetric on the invariant by canonicalising both sides
  inside one helper" and for "macOS smoke caught what Linux CI missed."
- `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  — pattern of pinning hazards with named regression tests (informs U2's test name).

## Key Technical Decisions

- **Predicate home: new `src/trace/walk.rs` submodule.** A named sibling helper holding the shared
  filter discipline — `pub(super)`-scoped, single predicate, no growth path implied. The pattern is
  more conservative than the `src/engine/` submodule shape (which holds co-equal substantial domain
  concepts like `predicate.rs` and `query.rs`); `walk.rs` is an internal sibling utility consumed by
  stats and maintenance only. The "free function in `mod.rs`" alternative was considered but is
  reserved in this codebase for tiny generic helpers (e.g. `format_rfc3339_millis`); a walk
  predicate with multiple callers, a static-grep invariant, and a deliberate cross-module exception
  is a different shape — the dedicated submodule gives the rationale comment in `query.rs` and the
  U3 invariant pin a stable target.
- **Predicate semantics: real trace data file.** Regular `.jsonl`/`.jsonl.gz` file, not a symlink,
  not `LAST_PRUNED_AT_FILE`. The state-file rejection lives inside the predicate via explicit name
  check — not as a side effect of the extension filter — so a future rename of the state file
  constant doesn't silently break the invariant.
- **Predicate return: file metadata only; callers retain their own mtime handling.** Stats currently
  skips the oldest/newest aggregate on `meta.modified()` failure; maintenance falls back to
  `SystemTime::UNIX_EPOCH`. The divergence is a deliberate per-caller policy decision and is
  preserved by returning the metadata only — callers compute `modified()` themselves. Maintenance's
  `unwrap_or(SystemTime::UNIX_EPOCH)` is the load-bearing case: it encodes the prune-eligibility
  policy "we can't stat the mtime → treat as ancient → eligible for compress/prune on this pass." A
  future predicate refactor that absorbed `modified()` and surfaced its failure as "skip the entry"
  would silently change this behaviour — files with broken metadata would be retained past the
  horizon instead of pruned. Metadata-only return preserves the policy where it currently lives in
  each caller.
- **`src/trace/query.rs` deliberately does not delegate.** Read-only consumer surface; the
  symlink-safety rationale that drove the filter in maintenance and stats doesn't apply
  (`lore trace why` can't accidentally delete or eat a symlink target). A comment above
  `list_trace_files_newest_first` documents the deliberate non-delegation so a future contributor
  doesn't "unify" it without rereading the rationale. The cost is acknowledged: `lore trace why` may
  report sessions that `lore status` does not count. An intermediate option — a thinner shared
  helper that applies the extension + state-file checks without the symlink filter, so query.rs
  could share the extension/state-file discipline while keeping its symlink-follow behaviour — was
  considered and rejected as YAGNI. The asymmetry on those two checks (an operator would have to
  drop a non-jsonl file AND be looking for it through `lore trace why`) is small; a second helper
  costs more in cognitive load than it returns in symmetry. Revisit if a third operator-visible
  asymmetry surfaces.
- **Trace maintenance extension hardening accepted as a deliberate behaviour change.** Both
  `run_lazy` (SessionStart-triggered, runs under `Verbosity::Silent`) and `run_manual` (the
  `lore trace prune` CLI surface) share `run_pass`, which is the only consumer of
  `enumerate_trace_files` — so the hardening lands on every Claude Code SessionStart, not only on
  explicit prune invocations. Today the prune phase deletes anything past the retention horizon that
  isn't a symlink or state file — including foreign files (`.tmp`, `.bak`, editor `.swp`). After the
  predicate gains the extension gate, the prune phase only deletes `.jsonl`/`.jsonl.gz`. Foreign
  files accumulate. Accepted because (1) foreign files in the trace dir are unusual operator
  artefacts, (2) lore's responsibility is its own data, (3) one predicate is a simpler mental model
  than two helpers. Surfaces in CHANGELOG under `### Changed`, with wording that names both surfaces
  so operators reading the changelog aren't misled into thinking the change is opt-in via the CLI.
- **Compress phase's extension check (`maintenance.rs:203-205`) is removed in U1.** Redundant once
  the predicate gates extension at enumeration. The load-bearing safety net for this removal is
  `gzip_file`'s own `.gz` short-circuit at `src/trace/maintenance.rs:312-314`, which prevents the
  compress loop from re-gzipping already-gzipped files even if a future predicate change widens
  enumeration to include `.gz` entries. Keep that short-circuit on any future maintenance.rs
  refactor — removing it would silently re-couple correctness to the upstream filter the predicate
  now owns. The inverse coupling is also load-bearing: `walk::is_real_trace_file` must include
  `.jsonl.gz` in its accept-set. Narrowing the predicate to `.jsonl`-only would silently make the
  compress phase correct again (no `.gz` entries to skip) while breaking `lore status` (`.gz` files
  no longer counted toward `total_bytes`) and prune (`.gz` files no longer deleted after the
  retention horizon). The walk predicate's extension scope and `gzip_file`'s `.gz` short-circuit are
  two ends of one contract — change either, audit both.
- **U3 invariant pin scoped to `src/trace/stats.rs` only.** Query.rs deliberately deviates, so a
  broader "all trace submodules delegate" pin would need an explicit query.rs exception. Per-file
  scope is the path of least exception.
- **U3 pin has two halves — negative AND positive.** Negative: zero occurrences of
  `std::fs::metadata`, `std::fs::symlink_metadata`, and `.metadata(` in test-stripped stats.rs.
  Positive: at least one `walk::is_real_trace_file` call. The negative half catches three regression
  shapes — the DirEntry `.metadata()` open-paren match is the most likely and is what
  `src/trace/query.rs:120` uses today (the open-paren keeps the substring from tripping on
  identifier-shaped accesses). The positive half catches the inverse failure: a contributor who
  removed the delegate-call entirely (e.g. rewriting stats to trust `DirEntry::file_type()` without
  any metadata call) would slip past three negative substring checks while violating the stated
  invariant. Together they pin both "stats does not stat directly" and "stats delegates to walk."
  `strip_test_modules` excludes the inline `#[cfg(test)] mod tests` so test fixtures don't trip
  either half.

## Implementation Units

### U1. New `walk` submodule, three-caller refactor, prune hardening, CHANGELOG entry

**Goal:** Create `src/trace/walk.rs` with the shared `is_real_trace_file` predicate. Refactor
`TraceStats::compute` and `enumerate_trace_files` to delegate. Remove the now-redundant compress
phase extension check. Add the rationale comment to `src/trace/query.rs`. Land the CHANGELOG entry
for the prune behaviour change.

**Requirements:** R1, R4

**Dependencies:** None

**Files:**

- Create: `src/trace/walk.rs` (new submodule; hosts `is_real_trace_file` and inline tests)
- Modify: `src/trace/mod.rs` (declare `pub mod walk;`)
- Modify: `src/trace/maintenance.rs` (delegate filter in `enumerate_trace_files`; remove compress
  phase extension check at lines 203-205)
- Modify: `src/trace/stats.rs` (delegate filter; state-file capture block at lines 78-87 stays)
- Modify: `src/trace/query.rs` (add rationale comment above `list_trace_files_newest_first`
  explaining deliberate non-delegation; no behaviour change)
- Modify: `CHANGELOG.md` (one assertive-voice sentence under `[Unreleased]` `### Changed` ending in
  `(#N)`)
- Test: `src/trace/walk.rs` (inline `#[cfg(test)] mod tests` with predicate unit tests)

**Approach:**

- Predicate input: a directory entry path. Output: file metadata when the entry is a real trace data
  file (regular file, `.jsonl`/`.jsonl.gz` extension, not a symlink, not `LAST_PRUNED_AT_FILE`);
  otherwise "skip." The predicate owns the single `symlink_metadata` syscall. Returns metadata only
  — no derived mtime — so callers retain their existing mtime fallback policies.
- Filter order inside the predicate: name check (reject `LAST_PRUNED_AT_FILE`) → extension check
  (reject anything not `.jsonl`/`.jsonl.gz`) → `symlink_metadata` → reject symlinks and non-regular
  files. Name check before extension check so the state-file rejection is explicit rather than
  implicit.
- `enumerate_trace_files` becomes a thin wrapper: iterate `read_dir`, call the predicate, push
  `(path, modified)` tuples for "keep" returns. The state-file name check at the head of the current
  function disappears (now inside the predicate).
- `TraceStats::compute` keeps the `.last_pruned_at` capture block (lines 78-87) — it has the special
  timestamp-extract behaviour. After the capture block, call the predicate for the remaining
  entries.
- `src/trace/query.rs::list_trace_files_newest_first` is unchanged in behaviour. A new doc comment
  above it cites this plan and explains: read-only surface, no data-safety argument for skipping
  symlinks, intentional asymmetry with stats/maintenance, future contributors should rereview before
  unifying.
- Compress phase's `if path.extension().is_none_or(|e| e != "jsonl") { continue; }` at
  `maintenance.rs:203-205` is removed; the predicate now guarantees the extension constraint at
  enumeration.
- CHANGELOG entry follows the project shape (user-facing, assertive voice, ends in `(#NN)`). Sample:
  `Trace maintenance now only deletes trace data files (\`.jsonl\` / \`.jsonl.gz\`) after the
  retention horizon; other files in the trace directory are left alone instead of being swept —
  applies to both \`lore trace prune\` and the SessionStart-triggered lazy maintenance pass. (#NN)`.

**Patterns to follow:**

- `src/engine/`'s submodule-directory layout — precedent for grouping concern-named files under a
  parent module. The reference is structural (where the new file lives) rather than conceptual
  (`walk.rs` is a lighter-weight sibling-utility, not a co-equal domain concept).
- Existing filter logic at `src/trace/maintenance.rs:282-286` (the `symlink_metadata` +
  `is_symlink() || !is_file()` body). Extracted into the predicate, with the state-file name check
  and extension check added explicitly.
- Sibling-module access pattern `super::<submodule>::<name>` already used in `src/trace/` (e.g.
  `super::maintenance::LAST_PRUNED_AT_FILE` at `src/trace/query.rs:113`).

**Test scenarios (inline `#[cfg(test)] mod tests` in `walk.rs`):**

- Happy path: a regular `.jsonl` file in the trace dir → predicate returns the metadata; `len()`
  matches.
- Happy path: a regular `.jsonl.gz` file → same.
- Edge case: the `.last_pruned_at` state file → predicate skips (explicit name check, regardless of
  any extension).
- Edge case: a non-jsonl file (e.g. `README.md`) → predicate skips.
- Edge case (`#[cfg(unix)]`-gated): a symlink named `*.jsonl` pointing outside the trace dir →
  predicate skips.
- Edge case: a subdirectory named `*.jsonl` → predicate skips (not a regular file).
- Error path: a path that fails `symlink_metadata` (e.g. broken symlink, race-deleted entry) →
  predicate skips without panicking.
- Integration: the existing `session_count_skips_symlinks_for_symmetry_with_maintenance`
  (`src/trace/stats.rs:243`) and `maintenance_skips_symlinked_entries`
  (`src/trace/maintenance.rs:533`) pass against the refactored callers without modification.
- Behaviour: a maintenance unit test that seeds a trace dir with one expired `.jsonl`, one expired
  `.jsonl.gz`, and one expired `.tmp` file and asserts `run_pass` deletes the `.jsonl` and
  `.jsonl.gz` but leaves the `.tmp` — pins R4's hardening behaviourally and pins the bidirectional
  `walk.rs` ↔ `gzip_file` contract from Decision 6 (the `.jsonl.gz` accept-set in walk.rs must
  survive the compress-phase iteration and reach the prune-phase delete). Exercising `run_pass`
  directly covers both `run_lazy` and `run_manual` via the shared call path.

**Verification:**

- All trace-module unit tests pass.
- The maintenance.rs syscall pins in `tests/invariants.rs:185-204` remain green — `symlink_metadata`
  now lives in `walk.rs`, but no current invariant constrains its count outside the maintenance.rs
  block.
- The new behavioural test for prune hardening passes.

### U2. Regression test: `total_bytes_excludes_last_pruned_at`

**Goal:** Pin the invariant that `TraceStats::total_bytes` excludes the maintenance state file and
`TraceStats::last_pruned_at` is captured from it.

**Requirements:** R2

**Dependencies:** U1 (predicate refactor lands first so the test exercises the refactored shape)

**Files:**

- Modify: `src/trace/stats.rs` (add unit test in `#[cfg(test)] mod tests`)

**Approach:**

- Fixture: tempdir + create trace dir; write a known-size `.jsonl` (e.g. 42 bytes); write
  `.last_pruned_at` containing a parseable Unix timestamp.
- Compute `TraceStats`. Assert `total_bytes` matches the real file's length exactly; assert
  `last_pruned_at` is `Some(<expected>)`; assert `session_count == 1`.
- Mirror the fixture shape of `populated_trace_dir_reports_counts_and_bytes` at
  `src/trace/stats.rs:196-214`.
- Test name: `total_bytes_excludes_last_pruned_at`.

**Patterns to follow:**

- `src/trace/stats.rs:196-214` (fixture shape).
- Sentence-shape test naming convention in the existing module.

**Test scenarios:**

- Happy path: trace dir contains one known-size `.jsonl` + a `.last_pruned_at` with a valid Unix
  timestamp. Assert `session_count == 1`, `total_bytes` equals the real file's length exactly,
  `last_pruned_at == Some(<derived>)`.
- Edge case (separate test or second scenario): `.last_pruned_at` with malformed contents
  (non-numeric). Assert `total_bytes` still excludes it; `last_pruned_at == None`.

**Verification:**

- The test passes against U1's refactored walker.
- Temporarily dropping the state-file name check or extension check in the predicate makes this test
  fail loudly — the invariant is durably pinned.

### U3. `src/trace/stats.rs` block in `tests/invariants.rs`

**Goal:** Static-grep CI guard that `src/trace/stats.rs` does not directly call any of
`std::fs::metadata`, `std::fs::symlink_metadata`, or the DirEntry `.metadata()` method. Locks in
"stats delegates filesystem classification to `walk::is_real_trace_file`."

**Requirements:** R3

**Dependencies:** U1 (refactor lands first so the assertion reflects the new shape)

**Files:**

- Modify: `tests/invariants.rs` (add stats.rs block between the existing maintenance.rs block at
  lines 185-204 and the trace/query.rs block at line 206)

**Approach:**

- Mirror the existing maintenance block's shape: read the source via the existing `read_source`
  helper, strip test modules via `strip_test_modules`, then three
  `assert_eq!(count_substring(...), 0, "...")` calls — for `std::fs::metadata`,
  `std::fs::symlink_metadata`, and `.metadata(` (negative half: stats does not stat the filesystem
  directly). Add one `assert!(count_substring(...) >= 1, "...")` call for `walk::is_real_trace_file`
  (positive half: stats does delegate to the predicate). The pair-of-assertions shape matches the
  maintenance.rs block at `tests/invariants.rs:185-204`, which pins both presence
  (`OpenOptions == 3`) and absence — together both halves encode "stats stops stat'ing AND starts
  delegating."
- The `.metadata(` substring (with open paren) catches `e.metadata()` (the DirEntry method) — the
  form `src/trace/query.rs:120` uses today and the more ergonomic shortcut a contributor would reach
  for. The open paren keeps the match from tripping on identifier-shaped accesses (e.g.
  `meta.metadata_field`).
- Rationale comment in the block: states the property pinned (stats delegates filesystem
  classification to `walk::is_real_trace_file`) and the failure message points the next contributor
  at the predicate.

**Patterns to follow:**

- `tests/invariants.rs:185-204` — maintenance.rs invariant block shape and message style.

**Test scenarios:**

- Test expectation: this unit IS itself the invariant test. Passing means `src/trace/stats.rs`
  outside its inline test module contains zero direct filesystem-stat calls AND at least one
  delegation to `walk::is_real_trace_file`.
- Verification of signal: after U1 lands, the invariants test passes. Reintroducing any of the three
  forbidden substrings into `src/trace/stats.rs` (outside the test module) fires the negative half.
  Removing the `walk::is_real_trace_file` call from stats's compute path fires the positive half.
  Both message strings should name the specific failure mode the contributor needs to fix.

**Verification:**

- The invariants test passes against the post-U1 state.
- Manually adding `e.metadata()` to `src/trace/stats.rs` outside the test module (then reverting)
  demonstrates the negative-half assertion fires with a useful message.
- Manually deleting the `walk::is_real_trace_file` call from stats's compute path (then reverting)
  demonstrates the positive-half assertion fires with a useful message.

## System-Wide Impact

- **Interaction graph:** Three trace walkers. `TraceStats::compute` and `enumerate_trace_files`
  delegate to `walk::is_real_trace_file`; `query.rs::list_trace_files_newest_first` deliberately
  does not (documented exception).
- **API surface parity:** `lore status` Trace block and MCP `lore_status.trace` JSON shapes are
  unchanged. `lore trace why` output is unchanged. Trace maintenance behaviour changes for foreign
  files: files that are not `.jsonl`/`.jsonl.gz` are no longer deleted after the retention horizon.
  This affects both `lore trace prune` (explicit CLI invocation) and the silent
  SessionStart-triggered lazy maintenance pass — both go through `run_pass` and consume the same
  predicate. Documented in CHANGELOG under `### Changed`.
- **Unchanged invariants:** `tests/invariants.rs:185-204` (maintenance.rs syscall pins) —
  `OpenOptions` and `read_to_string` counts are unaffected. `symlink_metadata` now lives in
  `walk.rs`; no current invariant constrains its count outside maintenance.rs's block.

## Risks & Dependencies

| Risk                                                                                                                                                                                                                                                                                | Mitigation                                                                                                                                                                                                                                                                                                                                                                                                 |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Operators with foreign files in `$XDG_STATE_HOME/lore/traces/` notice the files accumulate post-update where they used to be swept after the retention horizon. The change applies on every Claude Code SessionStart (silent lazy pass) and on every `lore trace prune` invocation. | CHANGELOG entry under `[Unreleased]` `### Changed` calls out the behaviour change explicitly and names both surfaces so operators aren't misled into thinking the change is opt-in. Foreign files in the trace dir are operator artefacts, not lore data — the new behaviour is more truthful to the maintenance contract. The behavioural test in U1 pins the new contract on the shared `run_pass` path. |
| A future contributor reads the engine/walk parallel and "helpfully" makes `query.rs` delegate too, breaking the documented exception.                                                                                                                                               | The comment above `list_trace_files_newest_first` cites this plan and explains the read-side rationale. A unification PR has to overwrite the comment, which is the friction point an unprompted contributor will trip on.                                                                                                                                                                                 |
| Predicate's `symlink_metadata` syscall now lives in `walk.rs`; a future change relocating it could shift accounting.                                                                                                                                                                | U3 keeps the structural guard for stats.rs. The maintenance.rs invariant pin (`tests/invariants.rs:185-204`) constrains `OpenOptions` and `read_to_string` counts only, not `symlink_metadata` — no other pin breaks.                                                                                                                                                                                      |

## Sources & References

- **macOS smoke report:** `tmp/smoke-test-track-2-observability-macos-report.md` (Observations 2 and
  5 — resolved by closer reading of the smoke run plus the post-`48b28c0` merge diff; the planned
  work pivots from "fixing" observations to codifying the symmetry the smoke pass surfaced).
- **Smoke runbook:** `tmp/smoke-test-track-2-observability.md` (Scenario 17 exercises the symlink
  case).
- Related PR: #59 (Track 2 Observability — merge commit `5bab9e2`).
- Related code:
  - `src/trace/stats.rs:53-115` (`TraceStats::compute`)
  - `src/trace/maintenance.rs:258-292` (`enumerate_trace_files`); `:230-251` (prune phase);
    `:203-205` (compress extension check, to be removed)
  - `src/trace/query.rs:103-127` (`list_trace_files_newest_first`, deliberate exception)
  - `src/engine/` (precedent for shared-concern submodule pattern)
  - `tests/invariants.rs:185-204` (existing maintenance.rs invariant block)
- Related solutions:
  - `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md`
  - `docs/solutions/design-patterns/round-trip-discriminator-canonicalise-both-sides-2026-05-10.md`
  - `docs/solutions/database-issues/stale-chunks-symlinked-knowledge-dir-2026-05-14.md`
  - `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
