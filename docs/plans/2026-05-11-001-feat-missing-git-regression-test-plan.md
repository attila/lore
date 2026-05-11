---
title: "feat: Missing-git binary regression test"
type: feat
status: active
date: 2026-05-11
origin: docs/brainstorms/2026-04-08-edge-case-handling-requirements.md
---

# Missing-git Binary Regression Test

## Summary

Add an integration test that spawns the `lore` binary with `PATH` cleared so the `git` binary is
unreachable, and assert ingest completes successfully via the `full_ingest` fallback. R11.1 (non-git
directory → `NotCommitted` write status) is confirmed by inspection against existing in-process
tests; R11.4 (missing-binary fallback) adds a new test in `tests/edge_cases.rs`. Slice E of the
edge-case-handling brainstorm: test-only, no production code changes expected.

---

## Problem Frame

`is_git_repo` calls `Command::new("git").output()` and returns `false` on `Err` — covering both
"directory is not a git repo" and "git binary is missing from PATH". The non-git-repo path is
exercised by `add_pattern_creates_file_with_frontmatter` and siblings in `src/ingest.rs::tests`
(they call `add_pattern` on a `tempdir` with no `git init` and assert `CommitStatus::NotCommitted`).
The missing-binary path is structurally equivalent in production code but is not exercised by any
regression test today. Without coverage, a future refactor that replaces `Command::new("git")` with
a different mechanism could silently regress binary-less environments without CI catching it.

The brainstorm specifies the test recipe: `assert_cmd` spawning the `lore` binary with
`.env_clear().env("PATH", "")` on the child process only — explicitly **not** mutating the parent
test process's `PATH` via `std::env::set_var`, which would race with every other test that invokes
`git` (and there are many) in a parallel test runner.

---

## Requirements

Carried verbatim from origin (`docs/brainstorms/2026-04-08-edge-case-handling-requirements.md`),
scoped to Slice E.

- **R11.1.** Non-git knowledge directory ingests successfully and `try_commit` returns
  `CommitStatus::NotCommitted`. Already covered by existing `add_pattern` tests in
  `src/ingest.rs::tests` (notably `add_pattern_creates_file_with_frontmatter`, which writes to a
  `tempdir` with no `git init` and asserts `NotCommitted`). This slice confirms by inspection; no
  new test required.
- **R11.4.** Missing `git` binary on PATH: spawn the `lore` binary as a subprocess via `assert_cmd`
  with `.env_clear().env("PATH", "")`. `is_git_repo` must return `false`, ingest must fall through
  to full mode, and write operations must return `NotCommitted`. The parent process's `PATH` must
  not be mutated.

---

## Scope Boundaries

- **No production code changes.** Slice E is test-only. `is_git_repo`, `try_commit`, and the
  delta-vs-full fallback in `ingest` are unchanged.
- **No `lore serve` JSON-RPC NotCommitted test.** The write-ops-return-`NotCommitted` invariant is
  exercised in-process by existing `src/ingest.rs::tests` and `src/server.rs::tests`. Adding a
  spawn-the-binary + JSON-RPC framing test would more than double the unit's complexity for marginal
  additional coverage of an already-tested invariant. Documented here rather than silently dropped —
  re-add if a real regression slips past in-process coverage.
- **No `lore status` test on the no-git path.** Status output is unrelated to the missing-binary
  fallback.

### Deferred to Follow-Up Work

_(none — slices C and D remain on the brainstorm but are independent of E.)_

---

## Context & Research

### Relevant Code and Patterns

- `src/git.rs::is_git_repo` — calls `Command::new("git").args(["rev-parse", "--git-dir"]).output()`;
  returns `false` on `Err` or non-zero exit. The missing-binary case is the `Err` branch.
- `src/ingest.rs::ingest` — top-level entry point. Falls through to `full_ingest` when `is_git_repo`
  returns `false` or no previous-commit metadata exists.
- `src/main.rs::cmd_ingest` — CLI entry point invoked when the binary runs
  `lore ingest --config
  <path>`. The natural surface to spawn for an end-to-end PATH-less test.
- `tests/edge_cases.rs` — existing integration-test home for CLI edge cases. Already contains
  `ingest_empty_directory_warns_via_cli`, `serve_startup_warns_on_empty_dir_via_cli`, and the
  `setup_empty_knowledge` helper. The new test reuses the helper shape but seeds one markdown file
  so ingest has something to find.
- `src/ingest.rs::tests::add_pattern_creates_file_with_frontmatter` — confirms R11.1 (non-git dir +
  write → `NotCommitted`).

### Institutional Learnings

- `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` — the missing-git fallback
  classifies as tier-3 (silent success): `lore` works fine in plain directories, git is an
  enrichment. Warning on every non-git invocation would erode trust in real warnings. This slice
  codifies the existing tier-3 silent behaviour rather than changing it.

### External References

_(none — pure test work on familiar infrastructure.)_

---

## Key Technical Decisions

- **Spawn the binary, not in-process.** Per the brainstorm's R11.4 explicit prohibition on
  `std::env::set_var`: the 2024 Rust edition marks `set_var` `unsafe` precisely because process env
  is shared mutable state, and unit tests run in parallel. Mutating `PATH` from one test would
  corrupt every concurrent test that shells out to `git` (and many do, via `git_init` and friends).
  `assert_cmd` spawns a child with isolated env, eliminating the race.
- **Test home is `tests/edge_cases.rs`.** Matches the existing pattern for cross-cutting CLI-spawned
  tests in this codebase. The `add_pattern_creates_file_with_frontmatter` precedent for R11.1 lives
  inline in `src/ingest.rs::tests` and remains the canonical confirmation of that requirement.
- **Seed one markdown file, not zero.** A populated knowledge dir exercises the `full_ingest` path
  more meaningfully than an empty one (which would also overlap with the empty-knowledge-dir warning
  surface). The test asserts the fallback marker on stderr, not chunk count — embedding via Ollama
  may or may not be reachable in CI; if it is not, `index_single_file` records a per-file embedding
  failure in `result.errors` but the CLI still exits 0 because `cmd_ingest` only bails on
  `SingleFile` mode. The missing-git fallback contract is therefore (a) the fallback marker fires on
  stderr and (b) the binary exits 0, both independent of whether the embedder is reachable.
- **Pin the assertion to the unique fallback marker.** The string
  `"Not a git repository —
  running full ingest"` (`src/ingest.rs:218`) is the one marker that
  uniquely identifies the missing-git fallback path — other full-mode fallbacks emit different copy
  (`"No previous ingest recorded — running full ingest"`, `"Previous commit not found …"`, etc.).
  Asserting only on `"Found N markdown files"` would not distinguish missing-git from any other
  full-mode trigger.
- **PATH-cleared environment may strip more than git.** `.env_clear()` removes everything including
  `HOME`, `TMPDIR`, locale vars. SQLite tempfile creation and rusqlite's bundled build do not need
  PATH, but the test should preserve `HOME` (for config/path expansion) and `TMPDIR`/`TMP` (for
  SQLite spill files) explicitly if `env_clear` proves to break the binary on first run. The
  brainstorm's exact recipe is `.env_clear().env("PATH", "")`; honour it first, restore other vars
  only if implementation-time discovery shows the binary needs them.

---

## Open Questions

### Resolved During Planning

- **Whether to add a binary-spawn JSON-RPC test for `add_pattern` returning `NotCommitted` under
  PATH-cleared env.** No. The write-ops invariant is already exercised by in-process tests; the
  added test would be expensive (initialize + tool-call JSON-RPC frames, response parsing) and
  redundant. See Scope Boundaries.
- **Test file location.** `tests/edge_cases.rs` (precedent, brainstorm calls it out by name).

### Deferred to Implementation

- **Whether `env_clear` strips a variable the binary requires.** Implementation-time discovery: if
  the spawned `lore` fails for env-related reasons (not PATH-related), preserve `HOME` and the
  relevant temp-dir vars on the child process and document the addition in the test.

---

## Implementation Units

### U1. Add R11.4 regression test in `tests/edge_cases.rs`

**Goal:** Confirm `lore ingest` runs successfully against a populated knowledge directory when the
`git` binary is unreachable via PATH, exercising the missing-binary fallback into `full_ingest`.

**Requirements:** R11.4 (R11.1 confirmed by inspection — no new code).

**Dependencies:** None.

**Files:**

- Modify: `tests/edge_cases.rs`

**Approach:**

- Add an `assert_cmd` integration test that:
  1. Creates a `tempdir`, populates `knowledge/` with one markdown file (heading + body long enough
     to chunk), and writes a `lore.toml` pointing at the directory.
  2. Spawns `lore ingest --config <path>` with `.env_clear().env("PATH", "")` on the **child**.
     `cmd_ingest` does not consult `HOME` when `--config` is supplied (the HOME/XDG branch in
     `src/main.rs::default_config_path` is skipped); SQLite is bundled and does not shell out.
     `.env_clear().env("PATH", "")` is therefore sufficient and matches the brainstorm's recipe
     exactly. If implementation-time discovery surfaces a needed env var, document the addition and
     preserve it on the child.
  3. Asserts exit 0.
  4. Asserts stderr contains the unique fallback marker
     `"Not a git repository — running full
     ingest"` (`src/ingest.rs:218`). This is the
     load-bearing assertion — it is the one progress line that uniquely identifies the missing-git
     path versus other full-mode fallbacks.
  5. Optionally also asserts `"Found 1 markdown files"` (note plural — the format string always
     emits `markdown files` regardless of count) appears on stderr as a secondary signal that
     discovery ran.
- Does **not** mutate the parent test process's `PATH`. The child env is set via `assert_cmd`'s
  `.env_clear()`/`.env()` chain only. `assert_cmd`'s `cargo_bin("lore")` resolves the binary path
  from `CARGO_BIN_EXE_lore` before `.env_clear()` runs, so the child still finds the executable.

**Patterns to follow:**

- `tests/edge_cases.rs::serve_startup_warns_on_empty_dir_via_cli` — example of an `assert_cmd` test
  that spawns a `lore` subcommand and asserts on exit + stderr.
- `tests/edge_cases.rs::setup_empty_knowledge` — helper shape for tempdir + config setup. The new
  test needs a populated variant; either factor out a `setup_populated_knowledge` helper or inline
  the setup (single use; inlining is acceptable for one test).

**Test scenarios:**

- Happy path (R11.4): tempdir with one markdown file, `lore ingest --config <path>` spawned with
  PATH cleared → exit 0, stderr contains `"Not a git repository — running full ingest"`, no panic on
  the missing git binary. Embedding may fail with Ollama unreachable in CI; that does not change
  exit code or the fallback marker.

(No parent-side `git --version` "negative control" — `assert_cmd::Command::env_clear()` is
structurally child-only and cannot leak into the parent's env. The real proof the child saw a
cleared PATH is the fallback marker firing on stderr; if PATH leaked, `is_git_repo` would return
true on a real git checkout and the marker would not fire.)

**Verification:**

- `cargo test --test edge_cases <test-name>` passes on a clean checkout.
- A refactor of `is_git_repo` that panics or propagates errors on missing git (e.g. swapping
  `.unwrap_or(false)` for `.unwrap()` or `?`) would be caught here — the test pins the observable
  contract that missing git → silent full-mode fallback, not a specific error-handling shape inside
  `is_git_repo`.

---

### U2. Update CHANGELOG and ROADMAP

**Goal:** Reflect the slice landing in the release-tracking docs.

**Requirements:** None (documentation hygiene).

**Dependencies:** U1.

**Files:**

- Modify: `CHANGELOG.md` — extend the existing `[Unreleased]/Added` "Edge case handling" entry added
  by PR #42, or add a short follow-up bullet noting the missing-git regression test now exists.
  Either is acceptable; the implementer picks based on rendered length.
- Modify: `ROADMAP.md` — move Slice E from the "Up Next > Edge case handling" line into the
  Completed section, mirroring how slices A+B landed in PR #43.

**Approach:**

- The Slice E test does not change user-visible behaviour. The CHANGELOG entry should be brief (one
  sub-bullet under the existing edge-case Added entry, or a one-line "Tests" mention) and honest
  about scope — "regression test for missing git binary on PATH; codifies existing tier-3 silent
  fallback" — rather than implying new functionality.
- The ROADMAP edit removes Slice E from the active bullet (leaving C and D) and adds a Completed
  entry pointing at this plan.

**Test scenarios:**

Test expectation: none — documentation-only unit, no behaviour change.

**Verification:**

- `dprint check` passes on the edited files.
- `git diff origin/main..HEAD ROADMAP.md` shows Slice E flipped to Completed.
- `git diff origin/main..HEAD CHANGELOG.md` shows the appended entry under `[Unreleased]/Added`.

---

## System-Wide Impact

- **Interaction graph:** None. The new test is fully isolated (`tempdir`, no DB shared state, no
  fixtures touched).
- **Error propagation:** None — no error paths added or modified.
- **State lifecycle risks:** None.
- **API surface parity:** None. `lore ingest` is the existing CLI surface; this test exercises it
  as-is.
- **Integration coverage:** The test itself is an integration scenario — it spawns the compiled
  binary against a real (empty-PATH) child env, exercising the full code path from `cmd_ingest`
  through `is_git_repo` to `full_ingest`.
- **Unchanged invariants:** `is_git_repo`'s contract (`false` on non-repo, missing-dir, or
  missing-binary) is preserved by construction — no production code changes.

---

## Risks & Dependencies

| Risk                                                                                                                                                                                                                         | Mitigation                                                                                                                                                                                                                                                       |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `env_clear()` strips a variable the binary needs (HOME, TMPDIR), making the test fail for the wrong reason                                                                                                                   | Preserve `HOME` and `TMPDIR`/`TMP` on the child if first-run fails for env-related reasons. Documented in Open Questions > Deferred to Implementation. The brainstorm's recipe (`.env_clear().env("PATH", "")`) is the starting point; restore only if needed.   |
| Sandbox already has 4 pre-existing test failures (`hook::tests::*` need `$HOME` write; `git::tests::push_branch_to_bare_remote` and `server::tests::add_pattern_with_inbox_returns_pending_review` need `github.com:22` SSH) | None of these block U1 — they fail under existing test infrastructure for sandbox reasons unrelated to slice E. The new test does not push to a remote and does not require `$HOME` write; it should pass in the sandbox.                                        |
| Future change to `is_git_repo` replaces `Command::new("git")` with a library call (`git2`, `gix`, etc.) — the test then asserts the wrong contract                                                                           | Acceptable. The test pins the **observable behaviour** (missing-binary → full-mode fallback), not the implementation. A library-backed `is_git_repo` should also return `false` when the runtime cannot locate a git installation, satisfying the same contract. |

---

## Documentation / Operational Notes

- CHANGELOG entry should NOT promise new user-visible behaviour. Slice E codifies existing
  behaviour; phrasing as "regression test added" is honest.
- No README or `docs/configuration.md` changes.
- Plan should flip to `status: completed` (with a matching `deepened:` date if any
  implementation-time discovery surfaces a meaningful refinement) before the final pre-merge commit,
  per the project's pre-merge plan-state convention.

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-04-08-edge-case-handling-requirements.md` (Slice E,
  R11.1 + R11.4 in the Implementation Slices table).
- **Companion plans on the same brainstorm:**
  `docs/plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md` (empty-knowledge-dir slice
  — shipped) and `docs/plans/2026-05-10-001-feat-unicode-nfc-slug-collisions-plan.md` (slices A+B —
  shipped). Sibling integration tests in `tests/edge_cases.rs` from PR #41 are the closest pattern
  to follow.
- **CLI behaviour ladder:** `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` —
  classifies the missing-git fallback as tier-3 (silent success).
- **Related code:** `src/git.rs::is_git_repo`, `src/ingest.rs::ingest`, `src/main.rs::cmd_ingest`.
