---
title: "feat: Empty Knowledge-Directory Detection"
type: feat
status: completed
date: 2026-05-04
deepened: 2026-05-10
origin: docs/brainstorms/2026-05-04-empty-knowledge-dir-validation-requirements.md
---

# Empty Knowledge-Directory Detection

## Overview

Surface a tier-2 warning when the effective scan set in the knowledge directory is empty — either
because no `.md` files exist or because `.loreignore` excludes every candidate. The warning fires at
the top-level `ingest()` entry point (`src/ingest.rs:199`), so both the `delta_ingest`
(`src/ingest.rs:471`) and `full_ingest` (`src/ingest.rs:678`) paths are covered in one place; the
existing partial warning at `src/ingest.rs:691-693` is removed and folded into the unified path.
Mirror the warning at MCP server startup (`cmd_serve`) and surface the disk state on the
`lore_status` MCP tool plus the `lore status` CLI command. Two distinct messages, one for each cause
(filesystem-empty vs all-ignored). No flag, no error, exit 0.

> **Refined 2026-05-10.** This plan supersedes a fail-fast framing with an opt-in
> `--allow-empty-knowledge` flag. After applying the project's CLI behaviour ladder, the case is
> tier-2 (warn), not tier-1 (fail). The flag, the bespoke `tests/unit/` and `tests/integration/`
> paths, the speculative `/health` HTTP endpoint, the speculative `--auto-heal-empty-knowledge`
> alias, and the speculative `docs/usage.md` references have all been dropped. See the brainstorm
> preamble for the rationale.

## Problem Frame

See the brainstorm — `Problem Frame` section is anchored against `src/ingest.rs` at commit
`f78d061`.

## Requirements (summary)

- **R1**: Unified effective-empty warning at the top of `ingest()` (`src/ingest.rs:199`); covers
  `delta_ingest` and `full_ingest` automatically. Removes the partial warning at
  `src/ingest.rs:691-693`. Two distinct messages — one per cause.
- **R2**: Same warning at `cmd_serve` startup (`src/main.rs:511`).
- **R3**: Both status surfaces report the state, via independent code paths. MCP `lore_status`
  (`handle_lore_status` at `src/server.rs:973-1014`) gains `empty_knowledge_dir: bool` and
  `knowledge_dir_status: "empty" | "populated"` in its JSON metadata; CLI `lore status`
  (`cmd_status` at `src/main.rs:707-761`) gains a corresponding `eprintln!` line. Detail in U3.
- **R4**: README + clap doc-comments describe the warning. No new doc file.
- **R5**: Inline unit tests via `ingest()` (not `full_ingest()` directly), one integration test, one
  `lore_status` test.

## Implementation Units

| Unit   | Goal                                                     | Files touched                                | Dependencies | Verification                                                                                   |
| ------ | -------------------------------------------------------- | -------------------------------------------- | ------------ | ---------------------------------------------------------------------------------------------- |
| **U1** | Unify warning at top of `ingest()` (covers delta + full) | `src/ingest.rs`                              | None         | Inline tests via `ingest()` (filesystem-empty, all-ignored, populated, delta-after-empty) pass |
| **U2** | Mirror warning at MCP server startup                     | `src/main.rs`, `src/server.rs` (or `lib.rs`) | U1           | Manual smoke: `lore serve` on empty dir prints warning once and stays running                  |
| **U3** | Add `empty_knowledge_dir` to MCP and CLI status          | `src/server.rs`, `src/main.rs`               | U1           | Inline test in `src/server.rs` asserts both JSON fields; manual smoke verifies CLI line        |
| **U4** | Integration test                                         | `tests/edge_cases.rs`                        | U1           | `assert_cmd` runs `lore ingest` on empty dir; asserts exit 0 + stderr warning                  |
| **U5** | Documentation                                            | `README.md`, `src/main.rs` (clap doc)        | U1, U2, U3   | `dprint check` passes; `cargo run -- --help` shows updated text                                |

## Implementation Details

### U1 — Unify effective-empty warning at top of `ingest()`

**File edits**: `src/ingest.rs`

- Add a small helper: `pub fn effective_scan_state(knowledge_dir: &Path) -> EffectiveScanState`,
  where `EffectiveScanState` is an enum with three variants:
  - `Populated` — at least one markdown file survives `.loreignore` filtering.
  - `FilesystemEmpty` — no `.md` files exist at all.
  - `AllIgnored` — files exist but `.loreignore` excludes every candidate.

  The helper internally calls `loreignore::load` and `walk_md_files`. A second helper
  `pub fn is_effective_empty(knowledge_dir: &Path) -> bool` wraps it for callers (U2, U3) that only
  need a bool.

- At the top of `ingest()` (after the `is_git_repo` check at line 207 but before the `last_commit`
  branching at line 213, so the warning fires regardless of git state and regardless of
  delta-vs-full path), call `effective_scan_state` and emit the matching message via `on_progress`:

  - `FilesystemEmpty`:
    `Warning: knowledge directory is empty — add at least one .md file under <path>`
  - `AllIgnored`: `Warning: .loreignore matched every markdown file; nothing will be indexed`
    (preserved verbatim from today's wording for diff minimisation and to keep the
    `full_ingest_with_all_files_excluded_indexes_nothing` test at `src/ingest.rs:2799` passing
    without rewording.)
  - `Populated`: no warning.

  This single call site covers `delta_ingest`, all four `full_ingest` short-circuit branches in
  `ingest()`, and the standalone "Not a git repository" / "No previous ingest recorded" / "Previous
  commit not found" / "Failed to resolve HEAD" / "git diff failed" paths uniformly.

- **Remove** the existing conditional warning at `src/ingest.rs:691-693`. The unified path in
  `ingest()` now fires it once, before either `full_ingest` or `delta_ingest` runs. Direct callers
  of `full_ingest` outside `ingest()` (today: tests only; verify with `git grep "full_ingest("`)
  lose the warning, which is acceptable because tests exercise the contract through `ingest()` per
  R5.

**Tests** (inline `#[cfg(test)] mod tests`, alongside `ingest_empty_directory_returns_zero` at line
1652):

- `ingest_empty_directory_warns`: empty `tempdir`, drive through `ingest()` (not `full_ingest()`
  directly), capture `on_progress` calls into a `Vec<String>` via a closure, assert one entry
  contains the filesystem-empty substring.
- `ingest_all_ignored_warns`: write `.md` files plus `.loreignore` excluding them all, drive through
  `ingest()`, assert the all-ignored substring fires.
- `ingest_populated_directory_does_not_warn`: positive control — write a `.md` file, drive through
  `ingest()`, assert no warning substring fires.
- `delta_ingest_after_emptying_warns`: write a `.md` file, run `ingest()` to record a commit; delete
  the file (`.loreignore` untouched), commit; run `ingest()` again; assert the filesystem-empty
  warning fires this time. Exercises the delta path explicitly.
- Keep `ingest_empty_directory_returns_zero` (line 1652) — it asserts the zero-result contract that
  the warning does not change.

**Verification**:

- `cargo test --features test-support`
- `cargo clippy --all-targets -- -D warnings`
- Confirm none of the existing tests at `src/ingest.rs:2667`
  (`delta_ingest_no_changes_returns_early`), `:2799`
  (`full_ingest_with_all_files_excluded_indexes_nothing`), or `:1652`
  (`ingest_empty_directory_returns_zero`) regress.

### U2 — Mirror warning at MCP server startup

**File edits**: `src/main.rs` (`cmd_serve`, line 511) and / or `src/server.rs` (`start_mcp_server`,
line 70)

- At server boot, after config load and before entering the read-loop, call
  `ingest::is_effective_empty(&config.knowledge_dir)` and emit the same warning to stderr via
  `eprintln!` if true. The exact call site (main vs server module) is up to implementation; placing
  it inside `start_mcp_server` keeps the `cmd_serve` wrapper thin and allows the in-process MCP
  server tests to exercise the path if useful.
- The warning fires once at boot. No per-request re-check.

**Verification**: manual `lore serve` against an empty `tempdir`; warning appears on stderr, server
stays running. (The in-process MCP server tests in `src/server.rs` already exercise startup; one of
them can assert the warning is captured.)

### U3 — Add `empty_knowledge_dir` to MCP and CLI status

The MCP tool (`handle_lore_status`) and the CLI command (`cmd_status`) render independently — the
CLI does not consume the MCP JSON. Both surfaces need separate edits.

#### U3.a — MCP `lore_status`

**File edits**: `src/server.rs`, `handle_lore_status` (line 973-1014)

- After the existing `loreignore_active` computation (line 993-995), add:

  ```rust
  let empty_knowledge_dir =
      crate::ingest::is_effective_empty(&ctx.config.knowledge_dir);
  let knowledge_dir_status = if empty_knowledge_dir {
      "empty"
  } else {
      "populated"
  };
  ```

- Add both fields to the `metadata` JSON object alongside `loreignore_active`,
  `inbox_workflow_configured`, etc.
- Update the tool description string at `src/server.rs:480` to mention the new fields. Update the
  corresponding insta snapshot
  (`src/snapshots/lore__server__tests__tools_list_returns_all_six_tools.snap`) via
  `cargo insta accept` after the change.

**Tests** (inline in `src/server.rs`):

- `lore_status_reports_empty_knowledge_dir`: tempdir with no `.md` files, call the handler, assert
  `empty_knowledge_dir == true` and `knowledge_dir_status == "empty"` in the JSON metadata.
- `lore_status_reports_populated_knowledge_dir`: tempdir with one `.md` file, assert `false` /
  `"populated"`.

#### U3.b — CLI `lore status`

**File edits**: `src/main.rs`, `cmd_status` (line 707-761)

- After the existing block that prints `Chunks` and `Sources` (around line 749-751), add a line
  printing the effective-empty state:

  ```rust
  let knowledge_state =
      if lore::ingest::is_effective_empty(&config.knowledge_dir) {
          "✗ empty"
      } else {
          "✓ populated"
      };
  eprintln!("  Knowledge:    {knowledge_state}");
  ```

  (The existing line 717 already prints the knowledge-dir _path_ as `Knowledge:`. Either rename the
  existing line to `Path:` and use `Knowledge:` for the state, or pick a different label such as
  `Scan set:` for the new line. Implementation may choose; the test asserts the substring, not the
  label.)

- The CLI output stays human-formatted (`✓` / `✗` decoration matches the existing Ollama / Model /
  sqlite-vec lines). No JSON output here — the CLI is human-only; agents use the MCP tool.

**Tests**: a CLI integration assertion in U4 covers this; no separate unit test, because
`cmd_status` is mostly composition of helpers that already have coverage.

### U4 — Integration tests

**File creation**: `tests/edge_cases.rs`. (Confirmed not present in `tests/` today — sibling files
are flat `tests/*.rs` and the directory has no `integration/` or `unit/` sub-tree. If
`feat/edge-case-handling` lands a `tests/edge_cases.rs` first, share the file rather than fork.)

Two `assert_cmd` tests:

- `ingest_empty_directory_warns_via_cli`: spawn `lore ingest --config <path>` against a tempdir
  holding nothing, with a config pointing at the tempdir. Assert exit code 0 and stderr contains the
  filesystem-empty warning substring.
- `status_reports_empty_knowledge_dir_via_cli`: spawn `lore status --config <path>` against the same
  tempdir setup. Assert stderr contains the empty-state substring written by `cmd_status` in U3.b.

Test placement matches project convention (`rust/testing-strategy.md`): flat `tests/*.rs`, not
`tests/integration/`.

**Verification**: `cargo test --features test-support --test edge_cases`.

### U5 — Documentation

**File edits**:

- `README.md`: a short paragraph in the existing usage section describing the warning behaviour. No
  new section unless one is already topical.
- `src/main.rs`: update the clap doc-comment on `Commands::Ingest` (around line 76-83) and
  `Commands::Serve` (around line 89) to mention the warning and that exit status stays 0. The
  `lore --help` rendering picks this up automatically.

**Verification**:

- `dprint check` passes (markdown formatting).
- `cargo run -- --help` shows the updated text.

## Verifying downstream safety (no separate unit)

The original plan included U7, "Verify downstream components do not assume at least one markdown
file", with proposed defensive guards in `src/search.rs` and `src/pattern.rs`. A code read of
`full_ingest` (`src/ingest.rs:678-734`) confirms zero files iterates an empty vector cleanly with a
zero-result `IngestResult`; search against a zero-pattern index returns zero results without panic.
**No guards needed; no separate unit.** If implementation surfaces a panic during U1 testing, scope
a guard to the specific call site rather than do a sweeping pass.

## Risks & Mitigation

- **Risk**: The unified warning fires twice — once via the new path, once via leftover code at
  `src/ingest.rs:691-693`. _Mitigation_: U1 explicitly _replaces_ the existing branch; the
  `ingest_all_ignored_warns` test asserts the new helper output, which catches any leftover
  fragment.

- **Risk**: `is_effective_empty` walks the directory a second time when called from `cmd_serve` and
  `handle_lore_status`, doubling I/O on every status call. _Mitigation_: the walk is cheap on
  realistic sizes (markdown files only, bounded by `walkdir`); status calls are agent-driven and
  infrequent. If benchmarks later show a hotspot, cache via a `Once` or move the result into the
  `ServerContext`.

- **Risk**: `cargo insta` snapshot drift on the MCP `tools/list` snapshot
  (`src/snapshots/lore__server__tests__tools_list_returns_all_six_tools.snap`) blocks CI when the
  description string for `lore_status` updates. _Mitigation_: U3 explicitly calls out running
  `cargo insta accept`. Treat the snapshot update as part of the same commit so reviewers see the
  diff intentionally.

- **Risk**: Integration test flakes on shared tempdir helpers under parallel test execution.
  _Mitigation_: each test creates its own `tempfile::tempdir()` — the pattern already used
  throughout `src/ingest.rs` tests.

## As shipped

All five units landed on `feat/empty-knowledge-dir-validation`:

| Unit | Commit    | Subject                                                            |
| ---- | --------- | ------------------------------------------------------------------ |
| U1   | `0176fef` | feat: warn on effective-empty knowledge dir from top of `ingest()` |
| U2   | `e4f52a0` | feat: mirror effective-empty warning at MCP server startup         |
| U3.a | `84357a2` | feat: report `empty_knowledge_dir` on `lore_status` MCP tool       |
| U3.b | `187bc98` | feat: surface effective-empty state in `lore status` CLI           |
| U4   | `e459558` | test: add CLI integration tests for empty-dir warning              |
| U5   | `247fe9d` | doc: document empty-knowledge-dir warning in README + `--help`     |

The implementation matches the spec above with two minor clarifications worth recording:

- **CLI `Scan set:` line uses one unified message**, while the ingest path emits two distinct
  warnings. The CLI status is a single line per state so a unified message reads cleaner; ingest has
  more room to spell out the recovery action.
- **The `delta_ingest_after_emptying_warns` test** explicitly drives through `ingest()` rather than
  `full_ingest()` so the delta path is exercised. Direct callers of `full_ingest()` no longer fire
  the warning; the existing `tests/loreignore.rs` integration test was updated to route through
  `ingest()` accordingly.

## Smoke verification

Verified locally on 2026-05-10 against four scenarios using a release build and a tempdir config:

- **Filesystem-empty** → `lore ingest` emits
  `Warning: knowledge directory is empty — add at least one .md file under <path>`, exit 0.
- **All-ignored** (single `.md` plus `.loreignore` of `*.md`) → `lore ingest` emits
  `Warning: .loreignore matched every markdown file; nothing will be indexed`, exit 0. The message
  is distinct from the filesystem-empty one.
- **Populated** (one `.md`) → `lore ingest` emits no warning; `lore status` shows
  `Scan set:     ✓ populated`.
- **Server startup** on the empty dir → `lore serve` prints the warning before the
  `[lore] MCP server started` banner.

`just ci` is green locally apart from a small set of pre-existing sandbox-bound test failures
unrelated to this feature (two `hook::tests::*` requiring `$HOME` write, two git-push tests
inheriting `GIT_CONFIG_*` env pollution, and `cargo deny` fetching the RustSec advisory database
over SSH). All four are tracked separately for refactor after this PR merges.

## Next Steps

1. Open a draft PR from `feat/empty-knowledge-dir-validation` → `main`.
2. Self-review the diff in the GitHub UI; capture any wording tweaks.
3. Mark ready for review, merge once green, flip this plan's frontmatter `status` to `completed`
   with `completed: <merge-date>`.
