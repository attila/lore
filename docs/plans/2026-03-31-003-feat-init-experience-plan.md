---
title: "feat: Improve init experience with XDG config and MCP CLI output"
type: feat
status: completed
date: 2026-03-31
origin: docs/brainstorms/2026-03-31-init-experience-requirements.md
---

# feat: Improve init experience with XDG config and MCP CLI output

## Overview

Move config and database from CWD-relative paths to XDG standard locations, add `--database` flag to
init, and display both JSON and `claude mcp add` CLI command in init output. This makes lore work as
an installed system tool without requiring `--config` on every invocation.

## Problem Frame

Lore's config (`lore.toml`) and database (`knowledge.db`) default to the current working directory.
This means MCP setup requires `--config` with an absolute path, and the config is not discoverable
from other directories. Additionally, init output only shows the JSON MCP config block — users must
construct the `claude mcp add` command themselves. (see origin:
docs/brainstorms/2026-03-31-init-experience-requirements.md)

## Requirements Trace

- R1. Default config: `$XDG_CONFIG_HOME/lore/lore.toml`, fallback `~/.config/lore/lore.toml`
- R2. Default database: `$XDG_DATA_HOME/lore/knowledge.db`, fallback
  `~/.local/share/lore/knowledge.db`
- R3. No CWD search — `--config` is the only override
- R4. `lore init` creates parent directories; re-running overwrites
- R5. Global `--config` flag overrides default for all commands
- R6. `lore init` accepts `--database` flag
- R7. (Existing behavior) `database` field in `lore.toml` stores absolute path for non-init commands
- R8. Init displays both JSON MCP config block and `claude mcp add` CLI command
- R9. Default config location → omit `--config` from output
- R10. Non-default config location → include `--config <absolute-path>` in output
- SC1. `database` field in generated config is always absolute
- SC2. `lore search` (no flags) resolves database from config and returns results
- SC3. `lore init --config /custom/path.toml` → output includes `--config`

## Scope Boundaries

- Single global config — multi-project via explicit `--config` (see origin)
- No CWD search fallback
- No `LORE_CONFIG` environment variable
- No `--database` on non-init commands
- No migration of existing CWD configs — users re-run `lore init`
- No new crate dependencies
- `$HOME` env var is required for XDG fallback (R1/R2); error if unset and no `--config` provided
- `knowledge_dir` resolution unchanged — `--repo` canonicalized as-is

## Context & Research

### Relevant Code and Patterns

- `src/config.rs`: `Config` struct, `default_config_path()`, `Config::load()`, `Config::save()`
- `src/main.rs`: `Cli` struct with clap derive, `Commands` enum, `cmd_init()` MCP output block at
  end of function
- `src/config.rs` `GitConfig`: pattern for optional config sections using `Option<T>` +
  `#[serde(default)]`
- `tests/smoke.rs`: CLI smoke tests using `assert_cmd::Command`
- `tests/e2e.rs`: integration tests using `tempfile::tempdir()`
- `README.md` line 78: existing `claude mcp add` syntax example

### Institutional Learnings

- Config filename `lore.toml` is established (renamed from `knowledge-mcp.toml`)
- Config stores absolute paths for `knowledge_dir` and `database` (via `canonicalize` and
  `current_dir().join()`)
- `cargo-deny` is configured; new dependencies need justification (none needed here)

## Key Technical Decisions

- **Path helpers in `config.rs`**: Resolution functions stay alongside `default_config_path()` and
  `Config` — only 2-3 functions, not enough for a new module. (see origin, deferred question
  resolved)
- **`--config` as `Option<PathBuf>`**: Change from `default_value_os_t` to `Option<PathBuf>`,
  resolve to XDG default in main() when `None`. This enables R9/R10 conditional output and keeps
  cmd_ function signatures unchanged (they still receive `&Path`). (see origin)
- **Resolution returns `Result`**: Path helpers return `anyhow::Result<PathBuf>` since `$HOME` may
  be unset. Error message directs user to `--config`.
- **Database path always written to config**: `lore.toml` always contains the absolute `database`
  path (existing behavior preserved). No optional/sentinel approach — simpler and matches current
  code.

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

**Config resolution decision matrix:**

| `--config` | `--database` | Config path          | DB path              | Output has `--config` |
| ---------- | ------------ | -------------------- | -------------------- | --------------------- |
| omitted    | omitted      | XDG config dir       | XDG data dir         | No                    |
| provided   | omitted      | user path (absolute) | XDG data dir         | Yes                   |
| omitted    | provided     | XDG config dir       | user path (absolute) | No                    |
| provided   | provided     | user path (absolute) | user path (absolute) | Yes                   |

**Resolution flow (main):**

```
parse CLI args
  ↓
config: Option<PathBuf> → resolve:
  Some(path) → absolute() → (path, user_provided=true)
  None → xdg_config_path()? → (path, user_provided=false)
  ↓
dispatch to cmd_* with resolved &Path
  (cmd_init also receives user_provided flag + database override)
```

## Open Questions

### Resolved During Planning

- **Where do path helpers live?** In `config.rs` — only 2-3 functions, close to existing
  `default_config_path()` and `Config`.
- **How to handle `$HOME` unset?** Return `anyhow::Result` with error: "Cannot determine config
  directory: $HOME is not set. Use --config to specify a path."

### Deferred to Implementation

- Exact function naming for XDG helpers (e.g., `xdg_config_path` vs `default_config_path`)
- Whether `default_config_path()` keeps its name (changing return type to Result is a breaking
  signature change — may rename)

## Implementation Units

- [x] **Unit 1: XDG path resolution helpers**

**Goal:** Add functions that resolve default config and data directory paths using XDG env vars with
`$HOME` fallback.

**Requirements:** R1, R2

**Dependencies:** None

**Files:**

- Modify: `src/config.rs`
- Test: `src/config.rs` (inline `#[cfg(test)]` module)

**Approach:**

- Replace or rename `default_config_path()` to return `anyhow::Result<PathBuf>`
- Add equivalent function for data directory
- Resolution: check env var → fallback to `$HOME/.config/lore/` or `$HOME/.local/share/lore/`
- Both return the full file path (config dir + `lore.toml`, data dir + `knowledge.db`)

**Patterns to follow:**

- Existing `default_config_path()` in `config.rs`
- `std::env::var("XDG_CONFIG_HOME")` / `std::env::var("HOME")`

**Test scenarios:**

- Happy path: `XDG_CONFIG_HOME` set → returns `$XDG_CONFIG_HOME/lore/lore.toml`
- Happy path: `XDG_DATA_HOME` set → returns `$XDG_DATA_HOME/lore/knowledge.db`
- Happy path: XDG vars unset, `HOME` set → returns `$HOME/.config/lore/lore.toml` and
  `$HOME/.local/share/lore/knowledge.db`
- Edge case: Both XDG var and `HOME` unset → returns error mentioning `--config`
- Edge case: `XDG_CONFIG_HOME` is empty string → falls back to `$HOME/.config/lore/`

**Verification:**

- All path resolution unit tests pass
- Functions compose correctly (return full file paths, not just directories)

---

- [x] **Unit 2: CLI restructuring**

**Goal:** Change `--config` to `Option<PathBuf>`, add `--database` to Init, resolve paths in main()
before dispatching to command functions.

**Requirements:** R3, R5, R6

**Dependencies:** Unit 1

**Files:**

- Modify: `src/main.rs`

**Approach:**

- Change `Cli.config` from `PathBuf` with `default_value_os_t` to `Option<PathBuf>` with no default
- Add `--database` as `Option<PathBuf>` to the `Init` variant of `Commands`
- In `main()`, resolve config path: `cli.config.map(Ok).unwrap_or_else(xdg_config_path)`
- Track whether config was user-provided (simple bool from `cli.config.is_some()`)
- Use `std::path::absolute()` (not `canonicalize`) on user-provided `--config` — the file may not
  exist yet
- Pass resolved `&Path` to `cmd_ingest`, `cmd_serve`, `cmd_search`, `cmd_status` — their signatures
  stay unchanged
- Pass `user_provided: bool` and `database: Option<PathBuf>` to `cmd_init` (signature changes)
- Update existing smoke tests in the same unit to keep the test suite green (pass `--config`
  explicitly or set XDG env vars where needed)

**Patterns to follow:**

- Existing clap derive pattern in `Cli` and `Commands`
- Existing `cmd_init` signature with per-command args

**Test scenarios:**

- Happy path: No `--config` flag → resolves to XDG default path
- Happy path: `--config /tmp/custom.toml` → uses that path, `user_provided=true`
- Happy path: `lore init --database /tmp/custom.db --repo ...` → passes database override to init
- Error path: XDG resolution fails (no HOME) and no `--config` → clear error message exits

**Verification:**

- `lore --help` shows `--config` without a default value displayed
- `lore init --help` shows `--database` option
- All existing smoke tests still pass (with `--config` explicitly provided where needed)

---

- [x] **Unit 3: Init path defaults and directory creation**

**Goal:** Update `cmd_init` to use XDG default paths for config and database, create parent
directories, and handle `--database` override.

**Requirements:** R4, R6, R7, SC1

**Dependencies:** Unit 2

**Files:**

- Modify: `src/main.rs` (`cmd_init` function)

**Approach:**

- Resolve database path: `--database` override (via `std::path::absolute()`, not `canonicalize` —
  file may not exist) or call XDG data path helper
- Create parent directories for both config and database paths (`std::fs::create_dir_all`)
  **before** `config.save()` — `Config::save` calls `std::fs::write` directly and does not create
  parents
- Replace `std::env::current_dir()?.join("knowledge.db")` with resolved database path
- Config save and provisioning flow otherwise unchanged

**Patterns to follow:**

- Existing `std::fs::canonicalize` usage for `--repo` in cmd_init (repo must exist, so canonicalize
  is correct there)
- Existing `config.save(config_path)` pattern

**Test scenarios:**

- Happy path: Init without `--database` → database at `$XDG_DATA_HOME/lore/knowledge.db`
- Happy path: Init with `--database /tmp/custom.db` → that path stored in config
- Happy path: Parent directories don't exist → created automatically
- Edge case: Re-running init → overwrites existing config at same location
- Integration: Config written by init contains absolute database path → `Config::load` reads it back
  correctly

**Verification:**

- `lore init` creates config at XDG config dir and database at XDG data dir
- Generated `lore.toml` has absolute `database` path
- Parent directories are created if missing

---

- [x] **Unit 4: Init MCP output**

**Goal:** Display both JSON MCP config and `claude mcp add` CLI command after init, with conditional
`--config` based on whether user provided it.

**Requirements:** R8, R9, R10, SC3

**Dependencies:** Unit 3

**Files:**

- Modify: `src/main.rs` (`cmd_init` function, MCP output block at end of function)

**Approach:**

- Use `user_provided_config: bool` parameter (added to `cmd_init` signature in Unit 2)
- Build args list: if user_provided, include `"--config", "<absolute-path>"`; else just `"serve"`.
  Do NOT include `--database` in MCP output — database path is stored in config and resolved at
  serve time
- Print JSON block with constructed args
- Print `claude mcp add` CLI command with same args
- Use `config_path.display()` for the absolute path in both

**Patterns to follow:**

- Existing `eprintln!` output block in `cmd_init`
- README.md line 78 for `claude mcp add` syntax:
  `claude mcp add --scope user --transport stdio lore -- lore serve [--config <path>]`

**Test scenarios:**

- Happy path: Default config → output shows `"args": ["serve"]` and
  `claude mcp add ... lore -- lore serve` (no --config)
- Happy path: Custom `--config /tmp/c.toml` → output shows
  `"args": ["serve", "--config", "/tmp/c.toml"]` and
  `claude mcp add ... lore -- lore serve --config /tmp/c.toml`
- Happy path: Both JSON block and CLI command are present in output
- Edge case: `--database` override does not appear in MCP output (R8 note)

**Verification:**

- Init output contains both JSON and CLI command
- `--config` presence in output matches whether user provided it
- Output paths are absolute

---

- [x] **Unit 5: Test updates and smoke tests**

**Goal:** Update existing tests broken by the path change and add smoke tests for new behavior.

**Requirements:** All — cross-cutting verification

**Dependencies:** Units 1-4

**Files:**

- Modify: `src/config.rs` (update `default_config_path_is_lore_toml` test)
- Modify: `tests/smoke.rs`

**Approach:**

- Update or delete `default_config_path_is_lore_toml` test: if `default_config_path()` was renamed,
  delete; if kept with new return type (`Result`), rewrite to test XDG resolution
- Verify `search_without_query_shows_error` still works — it passes
  `--config /tmp/nonexistent-lore.toml` which must be accepted (non-existent path is valid since we
  use `std::path::absolute`, not `canonicalize`)
- Add smoke test: `lore init --help` shows `--database`
- Add smoke test: `lore status` without `--config` → error mentions `lore init` (use
  `.env("XDG_CONFIG_HOME", tempdir)` for isolation from developer's real config)
- Add smoke/integration test for SC2: set `XDG_CONFIG_HOME` to tempdir with valid `lore.toml`, run
  `lore search <query>` without `--config`, verify it resolves the database from config

**Patterns to follow:**

- Existing `assert_cmd::Command` pattern in `tests/smoke.rs`
- Existing `predicates::prelude::*` for output assertions
- Use `.env()` on `Command` to set XDG env vars for test isolation

**Test scenarios:**

- Happy path: `lore init --help` output contains `--database`
- Happy path: `lore --help` does not show a default value for `--config`
- Happy path: SC2 — `lore search` (no `--config`) resolves database from XDG config
- Integration: `lore init --repo <tempdir>` with `XDG_CONFIG_HOME` set → stderr contains
  `"args": ["serve"]` and `claude mcp add` (no `--config` in output)
- Integration: `lore init --config /tmp/custom.toml --repo <tempdir>` → stderr contains `"--config"`
  in both JSON args and CLI command
- Error path: `lore status` with no config at XDG path → error mentions `lore init`
- Regression: All existing smoke tests continue to pass

**Verification:**

- `just ci` passes with all tests green
- No clippy warnings from changed code

## System-Wide Impact

- **Interaction graph:** All 5 command functions (`cmd_init`, `cmd_ingest`, `cmd_serve`,
  `cmd_search`, `cmd_status`) consume config_path — all benefit from XDG resolution but their
  internal logic is unchanged.
- **Error propagation:** New error path when `$HOME` is unset propagates as `anyhow::Error` through
  existing error handling in `main()`. Existing "Config not found" error in `Config::load` now
  points to XDG path instead of CWD-relative `lore.toml`.
- **API surface parity:** MCP server (`cmd_serve`) is affected only in that it now finds config at
  XDG default — the server protocol and tools are unchanged.
- **Unchanged invariants:** `Config` struct fields and serialization format are unchanged.
  `Config::load()` and `Config::save()` signatures unchanged. All MCP tools, database schema, and
  embedding logic unchanged.

## Risks & Dependencies

| Risk                                                                                | Mitigation                                                                 |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Breaking change for existing users with CWD-relative configs                        | Pre-1.0, documented in scope. Error message guides to `lore init`.         |
| `$HOME` unset in containers/CI where MCP server runs                                | Clear error message + `--config` override. Claude Code sets HOME.          |
| Init writes config before provisioning — partial failure leaves stale config at XDG | Existing behavior unchanged. Config at XDG is no worse than config at CWD. |

## Documentation / Operational Notes

- README.md MCP config section should be updated to reflect that `--config` is optional when using
  default location
- ROADMAP.md "Absolute path output" item can be checked off (addressed by R9/R10)

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-03-31-init-experience-requirements.md](docs/brainstorms/2026-03-31-init-experience-requirements.md)
- Related code: `src/config.rs` (`default_config_path`, `Config`), `src/main.rs` (`Cli`, `cmd_init`)
- Related ROADMAP item: "Absolute path output in `lore init` MCP config instructions"
- XDG Base Directory spec: environment variable semantics for `$XDG_CONFIG_HOME` and
  `$XDG_DATA_HOME`
