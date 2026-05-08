---
title: "feat: Defer SessionStart pinning for predicated universal patterns"
type: feat
status: completed
date: 2026-05-08
completed: 2026-05-08
---

# feat: Defer SessionStart pinning for predicated universal patterns

## Summary

Gate SessionStart's `## Pinned conventions` block by `applies_when_json` so only un-predicated
universal chunks pin at session start; predicated chunks defer to their existing PreToolUse
predicate path. Pure query-side change in `src/database.rs::universal_patterns()` plus tests and
documentation updates. The Track 1 v3 schema already carries the column; no migration is needed.

---

## Problem Frame

Track 1 of the `applies_when` predicate work (PR #39, merged via squash commit `794dcb6` on
2026-05-08) gated only the PreToolUse re-injection layer. SessionStart's pinned-conventions block
was unchanged: every `is_universal = 1` chunk pinned in full at every session start, regardless of
whether the chunk carried a predicate.

A pattern that carries an `applies_when` predicate has implicitly declared itself **not genuinely
universal** — only conditionally so. Pinning it at SessionStart contradicts that declaration. Track
1 dogfood (captured in `docs/solutions/workflow-issues/dogfood-reframes-workstream-2026-05-08.md`)
made the cost concrete: `workflows/git-branch-pr.md` (predicated to
`bash_command_starts_with: [git, gh]`, ~3 KB body) is pinned at every SessionStart, including
sessions with no git work, paying the full body cost for content the predicate would otherwise
filter out at every relevant tool call.

The fix makes the implication "predicate IS NOT NULL ⇒ not genuinely universal" explicit at the
SessionStart layer.

### Why not just drop the `universal` tag?

A reasonable alternative is to ask pattern authors to drop the `universal` tag from any pattern that
needs an `applies_when` predicate, and let the search pipeline route those patterns through the
normal relevance path. Track 1B does not take that route because the predicated-universal contract
is meaningfully looser than the non-universal contract. `search_with_threshold` over-fetches by
`top_k × 10` and partitions results into universal and ranked: the ranked side is truncated by
`top_k`, but the universal side is **not** truncated and prepends to the output. Removing the
`universal` tag would subject the pattern to the `top_k` cap and the `min_relevance` (not
`min_relevance_universal`) floor — strictly less reliable surfacing on tool calls where the
predicate would have matched. The intended user contract is "this pattern fires on every relevant
call", and the universal-with-predicate combination preserves that intent more faithfully than the
untagged alternative. Track 2-B can reconsider this once the broader `applies_when` design extends
to non-universal patterns.

---

## Requirements

- R1. SessionStart's pinned-conventions block excludes chunks where `applies_when_json IS NOT NULL`.
- R2. Un-predicated universal chunks (no `applies_when` block in frontmatter) continue to pin at
  SessionStart, unchanged.
- R3. PreToolUse predicate path is unchanged — predicated chunks still re-inject on matching tool
  calls via the existing `apply_predicate_filter`.
- R4. PostCompact's re-emit of pinned conventions inherits the new filter via the shared code path.
- R5. The change is documented in `CHANGELOG.md`'s `[Unreleased]` block under `### Changed`, framed
  as a behavioural refinement of Track 1's predicate.
- R6. `docs/pattern-authoring-guide.md`'s universal-tag section is amended so the claim "every
  universal pattern is pinned at SessionStart" is corrected; the `applies_when` section gains a
  short note on the SessionStart-defer behaviour and the first-tool-call delay trade-off.
- R7. No schema change. The Track 1 v3 schema already carries the `applies_when_json` column.

---

## Scope Boundaries

- Track 2 work (keyword bleed for non-universal patterns) — separate workstream.
- New predicate keys (`file_globs`, `repo_detection`, language hints) — Track 2-B candidates.
- `min_relevance_universal` config knob changes.
- Pattern-side migrations in the `lore-patterns` repository.
- MCP API surface changes (held follow-up from Track 1).
- FTS5 discoverability audit for predicated chunks — separate concern. Predicated chunks still
  surface via the existing search-overfetch + universal-no-truncate behaviour in
  `search_with_threshold` (see Key Technical Decisions for the precondition), not via dedicated FTS5
  keyword search, so the change does not alter retrieval semantics.

---

## Context & Research

### Relevant Code and Patterns

- `src/hook.rs:128-145` (`handle_session_start`) → `src/hook.rs:639-678` (`format_session_context`)
  → `src/hook.rs:699-763` (`render_pinned_conventions`) — the SessionStart code path that builds the
  pinned block.
- `src/database.rs:351-369` (`universal_patterns`) — the SQL SELECT that drives the
  pinned-conventions render. Currently:

  ```sql
  SELECT source_file, title, tags, raw_body
  FROM patterns
  WHERE is_universal = 1
  ORDER BY source_file
  ```

  Returns `Vec<UniversalPattern>` (struct at `src/database.rs:109-115`).
- `src/hook.rs:420` — PostCompact handler shares the same `format_session_context` path. The new
  filter automatically propagates without a second edit site.
- `src/hook.rs:315-379` (`apply_predicate_filter`) — Track 1's PreToolUse predicate filter. Track
  1B's SessionStart filter is a categorical subset (just "predicate present ⇒ defer") with no
  `CallContext` evaluation, so SQL placement is consistent with the simpler decision shape.
- `src/hook.rs:865` — universal dedup-bypass (`r.is_universal || !seen.contains(&r.id)`). Relevant
  because predicated chunks deferred from SessionStart still hit the PreToolUse path on matching
  calls; the bypass means dedup-seed state for these chunks is read-irrelevant, so the dedup
  contract is preserved without further plumbing.
- `tests/hook.rs:962-981` (`setup_with_universal_pattern`) and `tests/hook.rs:983-999`
  (`invoke_session_start`) — the existing test harness to extend.
- Existing baselines to mirror:
  `hook_session_start_emits_pinned_section_with_body_above_index_when_universal_present`
  (`tests/hook.rs:1015-1039`) and
  `hook_session_start_omits_pinned_section_when_no_universal_patterns` (`tests/hook.rs:1001-1013`).

### Institutional Learnings

- `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md` — dedup
  contract: `handle_session_start` truncates the dedup file via `reset_dedup` (no chunk IDs are
  seeded into dedup at SessionStart). The dedup file is populated only by `dedup_filter_and_record`
  in PreToolUse, and the universal read-side bypass at `hook.rs:865` keeps universals visible across
  every relevant PreToolUse regardless of dedup state. Track 1B does not change either side of this
  contract: SessionStart still truncates, PreToolUse still records. The change is purely to what
  SessionStart pins in the system-message payload, not to what it writes to dedup.
- `docs/solutions/integration-issues/additional-context-timing-in-pretooluse-hooks-2026-04-02.md` —
  first-tool-call delay: `additionalContext` injected at PreToolUse is visible to the agent only
  after tool composition. The first matching tool call after SessionStart sees a predicated chunk
  one tool call late. This is the correct trade-off given the predicate's "conditional relevance"
  semantics; the trade-off is worth documenting in the changelog and the pattern-authoring guide so
  future readers do not re-discover it.
- `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  — composition-cascade discipline. PostCompact shares `format_session_context`, so the filter
  propagates naturally; no extra edit site. The hazard-pin test in U2 makes the invariant auditable
  from tests alone, which is the cascade-pattern's recommended mitigation.
- `docs/solutions/workflow-issues/dogfood-reframes-workstream-2026-05-08.md` — meta-origin for the
  post-Track-1 reframe; cited in the CHANGELOG entry to anchor the rationale.

### External References

None. Internal change to existing query and test surface; no new external dependency.

---

## Key Technical Decisions

- **Filter at the SQL level, not Rust-side.** Append `AND applies_when_json IS NULL` to the
  `universal_patterns()` WHERE clause. The decision keeps the filter close to the data, mirrors how
  `is_universal = 1` already lives in SQL at the same layer, and avoids leaking `applies_when_json`
  onto the `UniversalPattern` struct (which currently has no need to carry it). The Rust-side
  alternative would require adding the column to the struct and threading it to a render-time filter
  for no behavioural gain.
- **Modify the existing `universal_patterns()` rather than introducing a new function.** Adding
  `universal_patterns_unconditional()` would imply a future companion
  `universal_patterns_predicated()` that the SessionStart path does not need; the categorical "pin
  only un-predicated universals" semantics fit a single filtered function. Verified call-site
  exclusivity: `universal_patterns()` has exactly one production caller —
  `render_pinned_conventions` in `src/hook.rs:714`. Other references in `src/server.rs`,
  `src/ingest.rs`, and `src/database.rs` are inside test code, and the existing tests use
  un-predicated fixtures so the new filter does not regress them.
- **No schema change.** The v3 schema introduced in Track 1 already carries `applies_when_json` on
  both `chunks` and `patterns`. Track 1B is purely a query-side refinement.
- **Universal dedup-bypass preserves the dedup contract.** SessionStart does not seed dedup
  (`handle_session_start` only calls `reset_dedup`); the dedup file is populated only by
  `dedup_filter_and_record` in PreToolUse. The existing universal read-side bypass at `hook.rs:865`
  (`r.is_universal || !seen.contains(&r.id)`) keeps universal chunks visible regardless of `seen`
  state. Track 1B changes neither write side nor read side: SessionStart still truncates dedup,
  PreToolUse still records, the bypass still applies. The only change is to what SessionStart pins
  in the system-message payload.
- **PostCompact inherits the filter automatically, and the plan pins both sides explicitly.**
  `format_session_context` is shared between SessionStart and PostCompact; no second-edit-site is
  required for the production change. However, U2 adds an explicit PostCompact hazard-pin test (in
  addition to the SessionStart hazard-pin test) so that a future refactor splitting the shared path
  cannot silently drop predicated-chunk filtering on the PostCompact side — the cascade-pattern
  mitigation cited under Institutional Learnings applies to both halves of the shared path.
- **Document the first-tool-call delay precondition** in the pattern-authoring guide and changelog
  entry. Predicated chunks are visible to the agent one tool call later than they were pre-Track-1B
  (because they no longer pin at SessionStart). The strict statement is that the predicated chunk
  reaches `apply_predicate_filter` only when (a) `engine::extract_query` returns a query (not on
  tool inputs that produce no query) AND (b) `search_with_threshold` returns at least one result
  above its threshold. With the default `min_relevance_universal` (which inherits from
  `min_relevance` and is `0.0` out of the box), the over-fetch (`top_k × 10`) and the
  universal-no-truncate partition in `search_with_threshold` keep predicated universals in the
  candidate set whenever search returns any results at all — the realistic case. Edge cases where a
  tool call produces no extracted query, or no search results above the configured floor, would
  defer the predicated chunk to a subsequent matching call. This is the correct trade-off for
  predicate semantics, but worth surfacing so future readers do not re-derive it.
- **CHANGELOG placement.** New `### Changed` bullet under `[Unreleased]`. Track 1's `### Added`
  bullet stays untouched as the historical record of the predicate's introduction; the SessionStart
  refinement is a discrete behavioural change worth its own line under Keep-a-Changelog convention.

---

## Open Questions

### Resolved During Planning

- _Filter at SQL or Rust-side?_ SQL — see Key Technical Decisions.
- _New named query or modify existing?_ Modify existing — see Key Technical Decisions.
- _PostCompact: separate edit site or shared?_ Shared via `format_session_context`. Verified during
  research.
- _Schema migration required?_ No. The v3 schema already carries the column.
- _Dedup contract impact?_ None. SessionStart never seeded dedup; the universal-bypass at
  `hook.rs:865` keeps universals visible on PreToolUse regardless of dedup state.

### Deferred to Implementation

- Exact wording of the doc comment in `universal_patterns()` documenting the rationale.
- Whether the `tests/hook.rs` helper `setup_with_universal_pattern` needs a predicated-variant
  constructor or whether the predicated fixture is built inline. Implementation will choose based on
  how much shared setup the new tests can leverage.
- Exact wording of the pattern-authoring-guide carve-out paragraph.
- (None — PostCompact hazard-pin coverage is now an explicit U2 test scenario rather than a deferred
  decision; the cascade-pattern mitigation requires both halves of the shared path to be pinned from
  tests.)

---

## Implementation Units

### U1. SQL filter for predicated universals

**Goal:** Add the `applies_when_json IS NULL` filter to the `universal_patterns()` SELECT so
SessionStart and PostCompact emit only un-predicated universal chunks under `## Pinned conventions`.

**Requirements:** R1, R2, R4, R7

**Dependencies:** None.

**Files:**

- Modify: `src/database.rs`

**Approach:**

- Append `AND applies_when_json IS NULL` to the WHERE clause of the SELECT in
  `universal_patterns()`.
- Add a brief doc comment on the function explaining the rationale: predicated chunks are not pinned
  at SessionStart; they re-inject on their first matching PreToolUse call via
  `apply_predicate_filter`. Cross-reference Track 1B in the changelog and the pattern-authoring
  guide.

**Patterns to follow:**

- The existing `is_universal = 1` filter at the same site — Track 1B's filter is the categorical
  companion.

**Test scenarios:** _Test expectation: covered by U2 — the SQL change has no isolated test surface;
behavioural coverage lives in the integration tests for SessionStart._

**Verification:**

- `just test` continues to pass against the existing test suite (no new failures from the filter
  applied to existing fixtures).
- The PreToolUse-only fixtures (the bulk of `tests/hook.rs`) still pass; SessionStart-specific tests
  are added in U2.

---

### U2. Hazard-pin tests for SessionStart filter behaviour

**Goal:** Pin the new SessionStart invariant from tests alone, including the predicated-defers-to-
PreToolUse cross-check so the dedup contract and the predicate-path interaction are auditable.

**Requirements:** R1, R2, R3, R4

**Dependencies:** U1.

**Files:**

- Modify: `tests/hook.rs`

**Approach:**

- Build on the existing `setup_with_universal_pattern` and `invoke_session_start` harness. Add a
  fixture variant that writes a universal pattern whose frontmatter includes an `applies_when` block
  (mirror the format Track 1 introduced).
- Add the hazard-pin test asserting that a predicated universal pattern is excluded from the
  SessionStart pinned body, while an un-predicated sibling universal still pins.
- Add a cross-check test asserting that the predicated universal still re-injects via PreToolUse on
  a matching tool call (mirror the existing PreToolUse universal tests in the same file).

**Patterns to follow:**

- `hook_session_start_emits_pinned_section_with_body_above_index_when_universal_present`
  (`tests/hook.rs:1015-1039`) — positive baseline for SessionStart pinning.
- `hook_session_start_omits_pinned_section_when_no_universal_patterns` (`tests/hook.rs:1001-1013`) —
  header-omitted shape.
- Existing PreToolUse universal tests in `tests/hook.rs` for the cross-check coverage style.

**Test scenarios:**

- _Happy path._ A universal pattern with `applies_when: bash_command_starts_with: [git, gh]` is
  **excluded** from the `## Pinned conventions` body emitted by `invoke_session_start`. Covers R1.
- _Happy path._ An un-predicated universal pattern alongside the predicated one **still pins**. The
  SessionStart body contains the un-predicated pattern's title and body, and does not contain the
  predicated pattern's. Covers R2.
- _Edge case._ When the only universal pattern is predicated, the SessionStart output omits the
  `## Pinned conventions` section entirely (header-omitted shape, mirroring the no-universal case).
  Covers the R1 + R2 boundary.
- _Integration._ Within a single test, against a shared fixture and a single populated DB: invoke
  SessionStart, capture the pinned body, and assert the predicated chunk is **not** in it. Then
  invoke a matching PreToolUse Bash call (e.g., `git status`) and assert the **same** chunk id
  appears in the `additionalContext` payload via the existing `apply_predicate_filter` path. The
  SessionStart-skip and PreToolUse-fire halves are coupled inside one test so the transition is the
  assertion target, not two independent properties. Covers R3.
- _Hazard pin: PostCompact._ Invoke `handle_post_compact` against the same fixture and assert the
  re-emitted pinned body excludes the predicated chunk. The production code path is shared with
  SessionStart today, but this test pins the invariant on the PostCompact side directly so a future
  refactor splitting the shared path cannot silently regress it (composition-cascade mitigation
  cited under Institutional Learnings). Covers R4.

**Verification:**

- All five new tests pass.
- The existing pre-Track-1B SessionStart tests continue to pass (un-predicated universals behave
  identically to before).

---

### U3. Documentation and changelog updates

**Goal:** Amend the pattern-authoring guide so the universal-tag section reflects the new
SessionStart-defer behaviour, and add a `[Unreleased]` `### Changed` entry to the changelog framing
the refinement.

**Requirements:** R5, R6

**Dependencies:** U1, U2 (so doc claims match shipped behaviour).

**Files:**

- Modify: `docs/pattern-authoring-guide.md`
- Modify: `CHANGELOG.md`

**Approach:**

- In `docs/pattern-authoring-guide.md`, amend the "When to use the `universal` tag" section's claim
  that every universal pattern is pinned at SessionStart. Add a short carve-out: a universal pattern
  with an `applies_when` predicate is **not** pinned at SessionStart; it re-injects on its first
  matching PreToolUse call.
- In the existing `applies_when` section of the same document, add a sentence calling out the
  SessionStart-defer behaviour and the first-tool-call delay trade-off.
- In `CHANGELOG.md`, add a `### Changed` bullet under `[Unreleased]` summarising the refinement and
  citing the meta-origin doc at
  `docs/solutions/workflow-issues/dogfood-reframes-workstream-2026-05-08.md`.

**Patterns to follow:**

- Existing `[Unreleased]` entries (CHANGELOG.md lines 7-43) for tone and granularity.
- The existing pattern-authoring-guide section structure for the carve-out paragraph.

**Test scenarios:** _Test expectation: none — documentation-only changes. The behaviour is covered
in U2; the documentation describes that behaviour._

**Verification:**

- `dprint fmt` passes (the project's pre-commit hook will reject any line-length drift).
- The doc claims match the U1 + U2 behaviour: a manual read of the doc against the new code path
  reads consistently.
- `just ci` passes.

---

## System-Wide Impact

- **Interaction graph:** SessionStart and PostCompact share `format_session_context`. The filter
  propagates to both via U1's single-site change.
- **Error propagation:** None. The filter is categorical (column NULL check); there is no new error
  path.
- **State lifecycle risks:** Dedup file: predicated chunks are no longer pre-seeded. The
  universal-bypass at `hook.rs:865` makes this irrelevant for read-side dedup; write-side recording
  on first PreToolUse injection is unchanged.
- **API surface parity:** No change to public CLI flags, MCP tool params, or config keys. The
  behavioural change is observable only through SessionStart and PostCompact output.
- **Integration coverage:** U2's cross-check test exercises the SessionStart-skip → PreToolUse-fire
  transition, which is the new interaction the change introduces.
- **Unchanged invariants:** Track 1's `apply_predicate_filter`, `evaluate_applies_when`, the
  smart-prefix matcher, the v3 schema, and the config knobs are all unchanged. Track 1B is purely a
  query refinement at the SessionStart layer.

---

## Risks & Dependencies

| Risk                                                                                                                                              | Mitigation                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| ------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| First-tool-call delay surprises a future reader who expected universal patterns to be visible at session start.                                   | Document the trade-off in `docs/pattern-authoring-guide.md` and the CHANGELOG `### Changed` entry (U3).                                                                                                                                                                                                                                                                                                                                       |
| A future `format_session_context` consumer (PostCompact, a hypothetical reset path) silently relies on the old "every universal pins" assumption. | The hazard-pin tests in U2 make the new invariant auditable from tests alone; any code path that violates it surfaces a test failure.                                                                                                                                                                                                                                                                                                         |
| FTS5 discoverability of predicated chunks degrades over time as patterns evolve.                                                                  | Out of scope for Track 1B; tracked in the Track 2 context document at `tmp/track-2-keyword-bleed-context.md`. The over-fetch (`top_k × 10`) plus the universal-no-truncate path in `search_with_threshold` keeps predicated universals in the candidate set across the realistic query range; the regression class Track 1B introduces is bounded to tool calls with no extracted query or zero search results — see Key Technical Decisions. |

---

## Documentation / Operational Notes

- No rollout flag, monitoring change, or migration. The change is fully self-contained: ship the
  binary, restart Claude Code sessions, and the next SessionStart payload reflects the new filter.
- The v3 database is unchanged; pre-Track-1B binaries reading a v3 DB still work and continue to pin
  all universals (the column they ignore is the predicate column, which is what they did before
  Track 1 too).

---

## Sources & References

- Origin (post-merge follow-up): https://github.com/attila/lore/pull/39 (squash commit `794dcb6`).
- Meta-origin: `docs/solutions/workflow-issues/dogfood-reframes-workstream-2026-05-08.md`.
- Track 1 plan (completed): `docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`.
- Track 2 context (gitignored): `tmp/track-2-keyword-bleed-context.md`.
- Related code: `src/hook.rs:699-763` (`render_pinned_conventions`), `src/database.rs:351-369`
  (`universal_patterns`).
- Test surface: `tests/hook.rs:962-1039` (existing SessionStart harness).
