---
title: "feat: No-HEAD progress line on fresh git init"
type: feat
status: completed
date: 2026-05-11
origin: docs/brainstorms/2026-04-08-edge-case-handling-requirements.md
---

# No-HEAD Progress Line on Fresh `git init`

## Summary

Replace the misleading `No previous ingest recorded — running full ingest` progress line with a
no-HEAD-specific message (`No commits yet — HEAD will be recorded after your first commit.`) when
the knowledge directory is a fresh `git init` with zero commits. Discrimination uses
`git
symbolic-ref --quiet HEAD` plus a `rev-parse --verify` on its target so other `head_commit`
failure modes (corrupted packed-refs, permission errors, detached HEAD with a broken ref) keep the
existing wording. Slice C of the edge-case-handling brainstorm. Tier-2 per the CLI behaviour ladder.

---

## Problem Frame

On a fresh `git init` with no commits, `lore ingest` enters the `db.get_metadata(META_LAST_COMMIT)`
None branch at `src/ingest.rs:223-226` and emits
`"No previous ingest recorded — running full
ingest"`. The user has no previous ingest because HEAD
doesn't exist yet, not because the database was cleared — the message misroutes their intuition. The
post-commit transition (R10) already works: once HEAD lands, the same branch fires, `full_ingest`
records HEAD at end, the next ingest runs in delta mode. But the message between init and first
commit nudges users to look for a state-management problem that does not exist.

The fix is a copy change with a discriminator: detect the unborn-branch state specifically (HEAD
exists as a symbolic ref pointing at an unborn branch like `refs/heads/main`) and emit a
no-HEAD-specific message. Other reasons the META_LAST_COMMIT branch fires (a real "first ingest on
an established repo" case, or a database that was cleared) keep the existing wording.

---

## Requirements

Adapted from origin (`docs/brainstorms/2026-04-08-edge-case-handling-requirements.md`), scoped to
Slice C. R9's entry-point phrasing is narrowed below — the origin says "`full_ingest` and the
delta-ingest entry point both emit" but in current code `ingest()` is the single entry point that
selects between delta and full and houses the fallback ladder, so the discriminator lives there
exactly once. No scope reduction — the message still fires for every code path that today emits the
misleading `No previous ingest recorded — running full ingest` wording.

- **R9.** On a git repository with no HEAD (fresh `git init`, zero commits), `ingest()` emits a
  clear, one-shot progress line —
  `No commits yet — HEAD will be recorded after your first
  commit.` — explaining the state. The
  message replaces the misleading `No previous ingest
  recorded — running full ingest` line **only
  in the no-HEAD case**; any other reason for falling through to full mode keeps its existing
  wording. Discrimination uses `git
  symbolic-ref --quiet HEAD` to detect the unborn-branch state
  specifically (HEAD exists as a symbolic ref pointing to an unborn branch), distinguishing it from
  other `head_commit` failure modes like corrupted packed-refs or permission errors, which must
  still surface as warnings.
- **R10.** Once the first real commit lands in the knowledge dir, the next `ingest` call falls
  through to `full_ingest` via the existing "No previous ingest recorded" path, which now records
  HEAD and enables delta mode on the run after. **No code change is required** — this already works.
  R10 is a regression-test contract only (covered by R11.3).

**Regression tests (R11 subset)**

- **R11.2.** Fresh `git init` with no commits (R9): ingest succeeds, the no-HEAD progress line fires
  exactly once, and `META_LAST_COMMIT` is **not** yet written.
- **R11.3.** No-HEAD → commit → re-ingest transition (R10): sequence is `(a)` `git init`, `(b)`
  write markdown, `(c)` `ingest()`, `(d)` `git add` + `git commit`, `(e)` `ingest()` again, `(f)`
  assert the second call recorded HEAD and the third call enters delta mode. **Must exercise the
  top-level `ingest()` entry point**, not `full_ingest()` directly, because the transition is the
  load-bearing behaviour.

---

## Scope Boundaries

- **No new flag.** No `--allow-no-head` or `--quiet-no-commits` silencer. Per the CLI behaviour
  ladder, silencer flags train users to mask the signal the warning was designed to provide.
- **No change to other full-mode fallback wording.**
  `"Not a git repository — running full
  ingest"`,
  `"Previous commit not found in history — running full ingest"`,
  `"Failed to resolve
  HEAD (…) — running full ingest"`, and
  `"git diff failed (…) — running full ingest"` remain unchanged. The no-HEAD message is a fifth,
  narrowly-scoped wording.
- **No detached-HEAD specialisation.** A detached HEAD pointing at an existing commit takes the
  normal `head_commit` happy path. A detached HEAD pointing at a corrupted ref takes the existing
  `"Failed to resolve HEAD"` path. Neither is the unborn-branch case the discriminator targets.
- **No `lore_status` MCP tool change.** `lore_status` already reports `last_ingested_commit`;
  surfacing "no HEAD yet" via that channel is a sibling slice for a future PR if a real user report
  justifies it.
- **No CLI subcommand surface change.** `lore ingest --help` is unchanged.

### Deferred to Follow-Up Work

_(none — Slice D (lossy-path warning) remains on the brainstorm but is independent of C.)_

---

## Context & Research

### Relevant Code and Patterns

- `src/ingest.rs:200` — `pub fn ingest(...)`. Top-level entry point. The fallback ladder lives here
  between lines ~217-252:
  - Line 217-220: `is_git_repo` false → `"Not a git repository — running full ingest"`.
  - **Line 223-226: `META_LAST_COMMIT` None →
    `"No previous ingest recorded — running full
    ingest"`.** Target of the R9 discriminator.
  - Line 229-232: `commit_exists` false →
    `"Previous commit not found in history — running full
    ingest"`.
  - Line 235-243: `head_commit` Err → `"Failed to resolve HEAD ({e}) — running full ingest"`.
  - Line 246-252: `diff_name_status` Err → `"git diff failed ({e}) — running full ingest"`.
- `src/git.rs:32` — `pub fn is_git_repo(dir: &Path) -> bool`. Returns false for non-repo and
  missing-binary cases. The unborn-branch case returns **true** (it's still a git repo), so the R9
  discriminator must run after `is_git_repo` but at or before the `META_LAST_COMMIT` check.
- `src/git.rs:245` — `pub fn head_commit(repo_dir: &Path) -> anyhow::Result<String>`. Returns `Err`
  on unborn branch, corrupted refs, and missing binary. R9's discriminator must distinguish the
  unborn-branch case from the other two — that's the rationale for `symbolic-ref` plus
  `rev-parse --verify` rather than reusing `head_commit`'s error.
- `src/ingest.rs:1765` — existing `git_init` test helper (creates repo + commits initial files by
  default — see whether a no-commit variant is needed).
- `src/ingest.rs:3170` — `full_ingest_records_head_commit` test. Confirms `full_ingest` records HEAD
  when one exists. R11.3's transition assertion builds on this.

### Institutional Learnings

- `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` — Slice C classifies as **tier-2
  (warn)**: continuing produces a coherent recoverable result (empty index, no commits yet), the
  user can fix it on their next run by committing. No silencer flag. The current wording already
  fires (tier-2 in spirit); this slice corrects the wording, not the tier.
- `docs/solutions/design-patterns/round-trip-discriminator-canonicalise-both-sides-2026-05-10.md` —
  adjacent pattern but not directly applicable here (no round-trip serialisation involved); cited as
  related context only.

### External References

_(none — git plumbing commands `symbolic-ref` and `rev-parse --verify` are stable and
well-documented; no external research needed.)_

---

## Key Technical Decisions

- **Discriminator placement: after `is_git_repo` true, in the `META_LAST_COMMIT` None branch.** The
  current `META_LAST_COMMIT` None branch fires for two structurally-distinct reasons: (a) we are in
  an unborn-branch state, never committed; (b) we are in a real repo with commits but the database
  has no `META_LAST_COMMIT` (first ingest or cleared metadata). The discriminator differentiates
  these two **inside** the existing None branch, keeping the rest of the fallback ladder untouched.
- **Discriminator via two git plumbing commands, not `head_commit`.** Reusing `head_commit`'s `Err`
  to mean "unborn branch" would conflate unborn-branch (silent, R9 wording) with corrupted-refs /
  permission-denied (existing wording, surfaces as warning). The brainstorm's explicit recipe —
  `git symbolic-ref --quiet HEAD` + `git rev-parse --verify <target>` — is the cleanest way to pin
  "HEAD is a symbolic ref to a ref that does not yet exist". A small helper in `src/git.rs` keeps
  the discriminator readable at the call site.
- **Wording matches the brainstorm exemplar.** The progress line is
  `No commits yet — HEAD will
  be recorded after your first commit.` — second-person,
  action-oriented, tells the user what to do next without being prescriptive. Periods and em dash
  match the existing fallback line style.
- **Single-fire contract.** The message fires exactly once per `ingest()` call (from the
  `META_LAST_COMMIT` None branch, before `full_ingest` is invoked). `full_ingest` itself does not
  re-emit any no-HEAD discrimination — it runs to completion, attempts to record HEAD at end, and
  the next `ingest()` call re-evaluates.
- **R10 needs no code change.** The brainstorm states this explicitly and the existing
  `full_ingest_records_head_commit` test confirms `full_ingest` writes `META_LAST_COMMIT` when
  `head_commit` succeeds. After the first user commit, the next `ingest()` enters the
  `META_LAST_COMMIT` None branch again (because the prior fresh-init ingest could not record HEAD —
  `full_ingest`'s `let Ok(head) = git::head_commit(...)` short-circuited the let-chain); it fires
  `full_ingest`, which now records HEAD; the _third_ `ingest()` enters delta mode.
- **CHANGELOG entry IS warranted.** Per the recently-codified
  `feedback_changelog_user_facing_only.md`, this slice's R9 wording change is user-visible — the
  stderr progress line text changes for the no-HEAD case. Add an `[Unreleased]/Changed` bullet (not
  `Added`) describing the wording shift.

---

## Open Questions

### Resolved During Planning

- **Should the discriminator use `head_commit`'s `Err` instead of `symbolic-ref` + `rev-parse`?**
  No. Conflates failure modes. The brainstorm calls out this trap explicitly.
- **Should we add `lore_status` reporting for "no HEAD yet"?** No. Out of scope per Scope
  Boundaries; surfaces as `last_ingested_commit: null` already.
- **CHANGELOG section: Added vs Changed?** Changed. The progress line wording shifts for a
  pre-existing scenario; no new feature is introduced.

### Deferred to Implementation

- **Exact name of the new git helper.** `is_unborn_head`, `has_unborn_head`, `head_is_unborn` —
  implementer's call. Recommend `is_unborn_head` to mirror `is_git_repo`.
- **Whether to add the helper next to `is_git_repo` or next to `head_commit`.** Placement is
  cosmetic; both group with the helper closest in semantic intent.

---

## Implementation Units

### U1. Add `is_unborn_head` helper in `src/git.rs`

**Goal:** Provide a single-purpose discriminator that returns `true` iff the directory is a git repo
whose `HEAD` is a symbolic ref to a branch that does not yet have any commits (unborn branch).

**Requirements:** R9 (helper).

**Dependencies:** None.

**Files:**

- Modify: `src/git.rs` — add the helper plus its inline `#[cfg(test)]` unit tests.

**Approach:**

- The helper invokes two git plumbing commands:
  1. `git symbolic-ref --quiet HEAD` to ask "is HEAD a symbolic ref, and if so, what's the target?"
     Returns the target (e.g. `refs/heads/main`) on success, exits non-zero on detached HEAD or
     missing/corrupted HEAD.
  2. `git rev-parse --verify <target>` to ask "does the target ref point to a real object?" Returns
     the SHA on success, exits non-zero on unborn branch (target doesn't exist yet).
- Unborn-branch state: command 1 succeeds, command 2 fails. Helper returns `true`.
- Any other combination — command 1 fails (detached HEAD or missing HEAD), or both succeed (normal
  repo) — returns `false`.
- Use the existing `Command::new("git")` pattern from `is_git_repo` and `head_commit`; no new
  dependencies.

**Patterns to follow:**

- `src/git.rs::is_git_repo` (line 32) — same shape: `Command::new("git")`, `.current_dir(...)`,
  `.output()`, map to bool.
- `src/git.rs::head_commit` (line 245) — for invocation-arg construction and stdout parsing.

**Test scenarios:**

- Happy path: `tempdir` + `git_init(dir)` helper (already runs init + config only, no commit) →
  helper returns `true`.
- Happy path: `tempdir` + `git_init(dir)` + commit a file via `git::add_and_commit(...)` → helper
  returns `false`.
- Edge case: plain `tempdir` (not a git repo) → helper returns `false`. (Both git commands fail;
  helper does not panic.)
- Edge case: `tempdir` with detached HEAD pointing at a real commit → helper returns `false`.
  (`symbolic-ref` fails on detached HEAD; helper short-circuits at command 1.)
- Edge case: missing `git` binary on PATH — verify the helper does not panic and returns `false` (so
  the existing fallback ladder catches the case via `is_git_repo`). Cover by spawning the test via
  `assert_cmd` with `.env_clear().env("PATH", "")` if integration coverage is needed, or rely on the
  helper's internal `.output().is_ok()` guard for unit-level coverage.

**Verification:**

- `cargo test --lib --features test-support git::tests::is_unborn_head_*` passes all four scenarios.
- Helper has no public dependencies beyond `std::process::Command` and existing imports.

---

### U2. Wire the discriminator into `ingest()` at the `META_LAST_COMMIT` None branch

**Goal:** When the `META_LAST_COMMIT` None branch fires, check `is_unborn_head` first; if true, emit
the R9 no-HEAD-specific progress line; otherwise keep the existing wording. Add the R11.2 regression
test.

**Requirements:** R9 (wiring), R11.2.

**Dependencies:** U1.

**Files:**

- Modify: `src/ingest.rs` — the `META_LAST_COMMIT` None branch around line 223-226.
- Test: `src/ingest.rs::tests` — add R11.2 inline alongside existing `full_ingest_*` tests.

**Approach:**

- At the existing `let Ok(Some(last_commit)) = db.get_metadata(META_LAST_COMMIT) else { ... }`
  block, branch the progress wording on `git::is_unborn_head(knowledge_dir)`:
  - `true` → `on_progress("No commits yet — HEAD will be recorded after your first commit.")`
  - `false` → keep the existing
    `on_progress("No previous ingest recorded — running full
    ingest")` (literal-equal to today).
- Fall through to `full_ingest(...)` in both cases — same exit, different progress copy.
- The single-fire contract is automatic: the branch fires once per `ingest()` invocation and
  `full_ingest` does not re-emit the discrimination.

**Patterns to follow:**

- The existing fallback-line emission shape at `src/ingest.rs:217-220` and `:223-226` — same call
  shape (`on_progress(...)` then return `full_ingest(...)`).

**Test scenarios:**

- Use a `RefCell<Vec<String>>` progress collector (the codebase has no existing precedent for
  asserting on captured progress — all existing tests pass `&|_| {}`). Shape:
  `let progress =
  RefCell::new(Vec::<String>::new()); let collect = |s: &str|
  progress.borrow_mut().push(s.to_string()); ingest(..., &collect);`.
  `on_progress` is `Fn`, not `FnMut`, so the `RefCell` is mandatory.
- **R11.2** (happy path): `tempdir`, existing `git_init(dir)` helper (already leaves repo with no
  commits — no new helper needed), write one markdown file, run `ingest()` with the collector.
  Assert: (a) result is success, (b) the captured progress list contains the literal
  `"No commits yet — HEAD will be recorded after your first commit."` exactly once, (c) the list
  does **not** contain `"No previous ingest recorded — running full ingest"`, (d)
  `db.get_metadata(META_LAST_COMMIT)` returns `None` after the run.
- Negative control: `tempdir`, `git_init(dir)`, `git::add_and_commit(...)` to land one real commit,
  fresh `memory_db()` (no prior metadata). The first `ingest()` against this state hits the
  `META_LAST_COMMIT` None branch with `is_unborn_head=false` (HEAD exists, just not recorded in DB).
  Assert the **old** wording fires (`"No previous ingest recorded — running
  full ingest"`), not
  the new one. This pins the discriminator's specificity — a regression that broadens the new
  wording to all None cases would be caught here.

**Verification:**

- `cargo test --lib --features test-support` covers both new tests plus all existing `full_ingest_*`
  and `ingest_*` tests still pass.
- Manual trace: `git init` a tempdir, write a `*.md` file, run the binary
  `lore ingest --config
  <path>` and visually confirm the new wording on stderr.

---

### U3. Add the R11.3 transition test for `ingest()` top-level entry point

**Goal:** Codify the no-HEAD → first-commit → delta-mode transition contract. Pure test addition; no
production code change. R10 is the "no code change required" requirement and this unit is its
regression contract.

**Requirements:** R10, R11.3.

**Dependencies:** U2 (uses the new wording via the top-level entry point).

**Files:**

- Test: `src/ingest.rs::tests` — new test alongside U2's R11.2 test.

**Approach:**

- Set up: `tempdir`, existing `git_init(dir)` helper at `src/ingest.rs:1765` (it already runs only
  `git init` + config — no commit, no `git_init_no_commit` variant needed), write `rust.md` with a
  heading + body, fresh in-memory DB via `memory_db()`.
- Use a `RefCell<Vec<String>>` progress collector — the codebase has no existing precedent (every
  other ingest test passes `&|_| {}`), so introduce the closure-with-RefCell pattern here for the
  first time. Shape:
  `let progress = RefCell::new(Vec::<String>::new()); let
  collect = |s: &str| progress.borrow_mut().push(s.to_string()); ingest(..., &collect);`
  — reset `progress` between calls so assertions target each call's emissions.
- Call `ingest()` (top-level — NOT `full_ingest()` directly): assert the no-HEAD progress line fired
  and `META_LAST_COMMIT` is None.
- Run `git_commit_all(dir, "initial")` (existing helper at line 3560 — adds all files, commits).
- Call `ingest()` a second time: this hits the `META_LAST_COMMIT` None branch again
  (`is_unborn_head` is now false because HEAD exists), fires the existing
  `"No previous ingest
  recorded — running full ingest"` wording, runs `full_ingest`, **records
  HEAD** (via the let-chain at `src/ingest.rs:862-868` that previously short-circuited on
  `head_commit`'s `Err`). Assert `META_LAST_COMMIT` is now `Some(<sha>)`.
- Call `ingest()` a third time: this exercises the delta path (META_LAST_COMMIT is set, points at a
  real commit). Assert the captured progress does **not** contain any of the full-fallback wordings
  — the run took the happy delta path.

**Dependency on U2:** U2 introduces the discriminator. This test exercises the multi-call lifecycle
that R11.3 requires (no-HEAD → first commit → delta-mode), so U2's wiring must be in place. U2's
R11.2 test pins the single-call no-HEAD contract; U3's test pins the transition.

**Patterns to follow:**

- `src/ingest.rs::tests::full_ingest_records_head_commit` (around line 3170) for the HEAD-recording
  assertion shape.
- `src/ingest.rs::tests::git_commit_all` helper (line 3560) for the commit step.
- The existing FakeEmbedder usage in delta tests (`delta_ingest_*` tests, search around line 3170+)
  for embedder construction.

**Execution note:** This is a multi-call test simulating a real user lifecycle. Use a single mutable
progress collector per call, reset between calls, so assertions can target the specific call's
emissions.

**Test scenarios:**

- The single multi-step scenario above is the test. Three sequential `ingest()` calls with distinct
  expected behaviours at each step (no-HEAD wording → existing "No previous ingest" wording →
  delta-path no-wording).

**Verification:**

- `cargo test --lib --features test-support ingest::tests::ingest_no_head_to_commit_transition` (or
  whatever final name) passes on a clean checkout.
- Failure mode that this test would catch: a regression where `full_ingest` no longer records HEAD
  when HEAD becomes available between two ingests would surface as the third call re-entering the
  full-fallback path instead of delta.

---

### U4. Update CHANGELOG, ROADMAP, flip plan to completed

**Goal:** Surface the user-visible wording change in release notes; mark Slice C completed in
ROADMAP; flip plan frontmatter.

**Requirements:** None (documentation hygiene).

**Dependencies:** U2, U3.

**Files:**

- Modify: `CHANGELOG.md` — add an `[Unreleased]/Changed` bullet describing the new wording.
- Modify: `ROADMAP.md` — move Slice C from the active edge-case bullet to Completed.
- Modify: `docs/plans/2026-05-11-002-feat-no-head-progress-line-plan.md` — flip frontmatter
  `status: active` → `status: completed` before the final pre-merge commit.

**Approach:**

- CHANGELOG entry shape (under `Changed`, not `Added`, since this is a wording shift on a
  pre-existing scenario, not a new feature):

  > **No-HEAD progress line on fresh `git init`.** When `lore ingest` runs against a knowledge
  > directory that's a freshly initialised git repository with zero commits, the progress line on
  > stderr now reads `No commits yet — HEAD will be recorded after your first
  > commit.` instead
  > of the misleading `No previous ingest recorded — running full ingest`. The latter wording still
  > fires for the other case it always covered: a real repo with commits but no recorded ingest
  > metadata (first ingest after `lore ingest --force`, cleared database, etc.). Tier-2 per the CLI
  > behaviour ladder; no exit-status change. Slice C of the edge-case-handling brainstorm.

- ROADMAP: remove Slice C from the Up-Next "remaining slices" bullet (leaving Slice D), add a
  Completed entry pointing at this plan.

**Patterns to follow:**

- `CHANGELOG.md` `[Unreleased]/Changed` precedent — see the existing "Predicated universal patterns
  are no longer pinned at SessionStart" entry for tone and structure.
- ROADMAP precedent — see PR #43 (slice A+B completion) and the slice-E completion entry from PR #44
  for the Up-Next-bullet-shrink-plus-Completed-entry pattern.

**Test scenarios:**

Test expectation: none — documentation-only unit, no behaviour change.

**Verification:**

- `dprint check` passes on edited files (per `feedback_changelog_user_facing_only.md` follow-up,
  always run `dprint fmt` after manual conflict resolves or before committing docs).
- ROADMAP diff shows Slice C flipped to Completed.
- Plan frontmatter `status: completed`.

---

## System-Wide Impact

- **Interaction graph:** Single touch-point in `ingest()`'s fallback ladder; no callbacks,
  middleware, or observers. `full_ingest`'s downstream calls are unchanged.
- **Error propagation:** The discriminator is an inert query — both git plumbing commands run
  read-only; their failure modes are silent (return `false`). Existing `Failed to resolve HEAD` and
  `git diff failed` fallback paths are unchanged.
- **State lifecycle risks:** None. The no-HEAD case writes nothing to the database (no HEAD to
  record); subsequent transition is the existing flow.
- **API surface parity:** `ingest()` is the only entry point exercising this fallback; the MCP
  `lore_status` tool surfaces the indirect signal via `last_ingested_commit: null` and is unchanged.
  Neither `add_pattern` nor `update_pattern` is affected.
- **Integration coverage:** R11.3 is the integration scenario — three sequential `ingest()` calls
  bracketing a real `git commit`. Exercises the unborn → committed → delta-mode lifecycle end-to-end
  against a real on-disk git repo with `FakeEmbedder`.
- **Unchanged invariants:** Four sibling fallback wordings (non-git, prev-commit-missing,
  head-resolve-failed, git-diff-failed) all keep their existing copy. The empty-knowledge-dir
  warning at the top of `ingest()` (`empty_warning_message`) is unchanged and still fires before any
  of these branches.

---

## Risks & Dependencies

| Risk                                                                                                                                                                                                                        | Mitigation                                                                                                                                                                                                                                                   |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `git symbolic-ref --quiet HEAD` returns a non-`refs/heads/` value (e.g. `refs/notes/...` for some plumbing edge), `rev-parse --verify` then verifies a real ref → helper returns `false` on a state that "should" be unborn | Acceptable. The fallback line in this case becomes the existing `"No previous ingest recorded"` — strictly no worse than today, and the case is exotic enough that the CLI message is not the user's primary diagnostic.                                     |
| `git symbolic-ref` exit semantics differ between git versions                                                                                                                                                               | Stable since git 1.x. The `--quiet` flag suppresses error output but exit codes are stable. Use exit-code-based branching (`.status.success()`), not stdout parsing of error text.                                                                           |
| Test flakiness from shelling out to git in unit tests                                                                                                                                                                       | Existing `git_init` and `git_commit_all` helpers in `src/ingest.rs::tests` already shell out to git in dozens of tests without flakiness. Reuse them. R11.2 needs a new `git_init_no_commit` helper that simply skips the post-init commit.                  |
| User missed the new wording — silent CLI difference                                                                                                                                                                         | The CHANGELOG/release-notes entry surfaces it; the wording is informative enough that on reading they recognise the state. Worst case: the message change is a non-event — they were already running fine on the old wording's misleading line.              |
| `is_unborn_head` becomes part of the public `git` module API and constrains future refactors                                                                                                                                | Helper is single-purpose, ~10 lines, no public consumers beyond `ingest()`. If the git module is reshaped later, this helper inlines cleanly. Mark `pub(crate)` rather than `pub` if no callers outside the crate are expected (verify in U1 — likely fine). |

---

## Documentation / Operational Notes

- The new wording is the only user-facing surface change. No README, `docs/configuration.md`, or
  `docs/pattern-authoring-guide.md` edits required — none of them quote the previous progress lines.
- The CHANGELOG entry covers release-notes discoverability.
- Plan should flip `status: completed` before the final pre-merge commit; no `deepened:` date needed
  unless an implementation-time refinement substantively shifts the plan body.

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-04-08-edge-case-handling-requirements.md` (Slice C in
  the Implementation Slices table; R9, R10, R11.2, R11.3).
- **Companion plans on the same brainstorm:**
  - `docs/plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md` (empty-dir, shipped)
  - `docs/plans/2026-05-10-001-feat-unicode-nfc-slug-collisions-plan.md` (A+B, shipped)
  - `docs/plans/2026-05-11-001-feat-missing-git-regression-test-plan.md` (E, shipped)
- **CLI behaviour ladder:** `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` —
  classifies this as tier-2 (warn but proceed).
- **Related code:** `src/ingest.rs:200-252` (fallback ladder), `src/git.rs:32` (`is_git_repo`),
  `src/git.rs:245` (`head_commit`).
