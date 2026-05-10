---
title: "feat: Empty Knowledge-Directory Detection"
type: feat
status: in-progress
date: 2026-05-04
deepened: 2026-05-10
origin: docs/brainstorms/2026-05-04-empty-knowledge-dir-validation-requirements.md
---

# Empty Knowledge-Directory Detection

## Overview

Surface a tier-2 warning when the effective scan set in the knowledge directory is empty — either
because no `.md` files exist or because `.loreignore` excludes every candidate. Replace today's
silent-zero-result on filesystem-empty and unify with the existing partial warning at
`src/ingest.rs:691-693`. Mirror the warning at MCP server startup (`cmd_serve`) and surface the disk
state on the `lore_status` MCP tool. No flag, no error, exit 0.

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

- **R1**: Unified effective-empty warning in `full_ingest`, replacing the partial warning at
  `src/ingest.rs:691-693`.
- **R2**: Same warning at `cmd_serve` startup (`src/main.rs:511`).
- **R3**: `lore_status` MCP tool (`src/server.rs:973`) and `lore status` CLI output gain
  `empty_knowledge_dir: bool` and `knowledge_dir_status: "empty" | "populated"`.
- **R4**: README + clap doc-comments describe the warning. No new doc file.
- **R5**: Inline unit tests, one integration test, one `lore_status` test.

## Implementation Units

| Unit   | Goal                                           | Files touched                                | Dependencies | Verification                                                                          |
| ------ | ---------------------------------------------- | -------------------------------------------- | ------------ | ------------------------------------------------------------------------------------- |
| **U1** | Unify effective-empty warning in `full_ingest` | `src/ingest.rs`                              | None         | New inline tests (filesystem-empty, all-ignored, populated control) pass              |
| **U2** | Mirror warning at MCP server startup           | `src/main.rs`, `src/server.rs` (or `lib.rs`) | U1           | Manual smoke: `lore serve` on empty dir prints warning once and stays running         |
| **U3** | Add `empty_knowledge_dir` fields to status     | `src/server.rs`                              | U1           | New inline test in `src/server.rs` asserts both fields for empty and populated states |
| **U4** | Tests: integration + status fields             | `tests/edge_cases.rs`, `src/server.rs`       | U1, U3       | `assert_cmd` integration test asserts exit 0 + stderr warning; status test passes     |
| **U5** | Documentation                                  | `README.md`, `src/main.rs` (clap doc)        | U1, U2, U3   | `dprint check` passes; `cargo run -- --help` shows updated text                       |

## Implementation Details

### U1 — Unify effective-empty warning

**File edits**: `src/ingest.rs`

- Replace the conditional warning at `src/ingest.rs:691-693`. The current branch fires only when
  `.loreignore` is the cause; the new shape fires whenever `md_files.is_empty()` after
  `discover_md_files` returns, branching on whether `.loreignore` filtering was responsible.
- Extract a helper, e.g.
  `fn empty_warning_message(walked_count: usize, has_loreignore: bool) -> &'static str`, that
  returns one of two messages (per the brainstorm's deferred question; default: two distinct
  messages):
  - filesystem-empty (`walked_count == 0`):
    `Warning: knowledge directory is empty — add at least one .md file`
  - all-ignored (`walked_count > 0 && md_files.is_empty()`):
    `Warning: .loreignore matched every markdown file; nothing will be indexed` (preserved verbatim
    from today for diff minimisation).
- Expose `pub fn is_effective_empty(knowledge_dir: &Path) -> bool` from `lore::ingest` so
  `cmd_serve` (U2) and `handle_lore_status` (U3) can reuse the check without re-walking from
  scratch. The helper internally calls `loreignore::load` and `walk_md_files` and returns `true`
  when the filtered list is empty.

**Tests** (inline `#[cfg(test)] mod tests`, alongside `ingest_empty_directory_returns_zero` at line
1652):

- `ingest_empty_directory_warns`: empty `tempdir`, capture `on_progress` calls into a `Vec<String>`
  via a closure, assert one entry contains the filesystem-empty substring.
- `ingest_all_ignored_warns`: write `.md` files plus `.loreignore` excluding them all, assert the
  all-ignored substring fires. Confirms the existing partial warning still works through the new
  path.
- `ingest_populated_directory_does_not_warn`: positive control — write a `.md` file, assert no
  warning substring fires.
- Keep `ingest_empty_directory_returns_zero` (line 1652) — it asserts the zero-result contract that
  the warning does not change.

**Verification**:

- `cargo test --features test-support`
- `cargo clippy --all-targets -- -D warnings`

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

### U3 — Add `empty_knowledge_dir` to `lore_status`

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

### U4 — Integration test

**File creation** (or extend if it exists): `tests/edge_cases.rs`

- Use `assert_cmd` to spawn `lore ingest --config <path>` against a tempdir holding nothing, with a
  config pointing at the tempdir.
- Assert exit code 0 and stderr contains the filesystem-empty warning substring.
- Test placement matches project convention (`rust/testing-strategy.md`): flat `tests/*.rs`, not
  `tests/integration/`.

**Verification**: `cargo test --features test-support --test edge_cases`.

### U5 — Documentation

**File edits**:

- `README.md`: a short paragraph in the existing usage section describing the warning behaviour. No
  new section unless one is already topical.
- `src/main.rs`: update the clap doc-comment on `Commands::Ingest` (around line 76-83) and
  `Commands::Serve` (around line 89) to mention the warning and that exit status stays 0. The
  `lore --help` rendering picks this up automatically.

**Verification**: `dprint check` passes (markdown formatting); `cargo run --
--help` shows the
updated text.

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

## Next Steps

1. Implement U1 first (unifying warning + helper + inline tests). Run
   `cargo test --features test-support`.
2. Implement U2 and U3 in either order — they are independent.
3. Implement U4 (integration test) once U1 + U3 are stable.
4. Implement U5 (documentation) last, so the copy reflects final wording.
5. Run `just ci` before opening the PR.
6. Open a draft PR; the existing `feat/empty-knowledge-dir-validation` branch is the target.
