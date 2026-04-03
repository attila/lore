---
title: "feat: Add LORE_DEBUG verbose logging and --json structured output"
type: feat
status: completed
date: 2026-04-03
deepened: 2026-04-03
---

# feat: Add LORE_DEBUG verbose logging and --json structured output

## Overview

Add two developer-facing features: (1) `LORE_DEBUG=1` environment variable that enables verbose
diagnostic logging to stderr for hook pipeline troubleshooting, and (2) `--json` flag on
`lore search` and `lore list` for machine-readable structured output.

## Problem Frame

The hook pipeline is opaque — when patterns don't surface as expected, there is no way to see what
query was extracted, what results came back, what was filtered by dedup, or why a threshold was not
met. Separately, `lore search` and `lore list` emit human-formatted text, which is hard to consume
from scripts or other tools.

## Requirements Trace

- R1. `LORE_DEBUG=1` emits verbose diagnostics to stderr for all hook pipeline stages (query
  extraction, search results, dedup filtering, threshold decisions)
- R2. `LORE_DEBUG=1` also emits diagnostics for `lore search` and `lore ingest` commands
- R3. Debug output never appears on stdout (preserving the data contract)
- R4. `lore search --json` outputs results as a JSON array to stdout
- R5. `lore list --json` outputs pattern summaries as a JSON array to stdout
- R6. JSON output includes all fields without truncation (unlike human format which truncates body
  at 500 bytes)
- R7. Both features are independently usable and composable
  (`LORE_DEBUG=1 lore
  search --json rust` should work)

## Scope Boundaries

- No logging framework (log, tracing, env_logger) — use a lightweight `debug!()` macro wrapping
  `eprintln!`
- No `--format` flag or format enum — just `--json` boolean, matching the roadmap item
- No JSON output for other commands (init, ingest, status, hook) — hook already outputs JSON; others
  are out of scope
- No config file option for debug — environment variable only

## Context & Research

### Relevant Code and Patterns

- `src/main.rs`: CLI definition (clap derive), command dispatch, all user-facing output formatting
- `src/hook.rs`: hook pipeline (`handle_hook`, `extract_query`, `search_with_threshold`, dedup
  logic), `HookOutput` types
- `src/database.rs`: `SearchResult` (line 62), `PatternSummary` (line 80) — neither has `Serialize`
  derive yet
- `tests/smoke.rs`: CLI integration tests with `setup_populated_env` helper
- `tests/hook.rs`: hook pipeline E2E tests

### Institutional Learnings

- CLI data commands must output to stdout; diagnostics to stderr
  (`docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md`)
- Hook errors are intentionally swallowed to stderr, never breaking Claude Code
- The `eprintln!` pattern is the established diagnostic output method

## Key Technical Decisions

- **Macro over function**: A `debug!()` macro (defined in a new `src/debug.rs`) checks `LORE_DEBUG`
  once via `std::sync::LazyLock<bool>` and calls `eprintln!` with a `[lore debug]` prefix. This
  avoids passing a debug flag through every function signature while keeping the check cheap after
  first call. `LazyLock` is cleaner than `OnceLock` (single expression, no `get_or_init`) and stable
  since Rust 1.80, well within MSRV 1.85.

- **Serialize on data types**: Add `#[derive(Serialize)]` to `SearchResult` and `PatternSummary` in
  `database.rs`. serde is already a dependency. Add `use serde::Serialize` import since
  `database.rs` does not currently use serde.

- **JSON field naming: `snake_case`**: CLI `--json` output uses Rust's native `snake_case` field
  names (`source_file`, `heading_path`). This is distinct from the MCP wire format in `server.rs`
  which uses explicit `#[serde(rename)]` for its JSON-RPC types. The CLI output schema is a separate
  contract.

- **Global `--json` flag**: Add `#[arg(long, global = true)] json: bool` on the `Cli` struct
  (alongside `--config`), not per-command. This avoids duplicating the flag on every command variant
  and is the standard CLI pattern (`gh`, `kubectl`). Commands that support it (`search`, `list`)
  read `cli.json`; others ignore it.

- **No body truncation in JSON**: The human format truncates body at 500 bytes for readability. JSON
  output includes the full body since consumers can handle it programmatically.

- **Debug placement priority**: The hook error-swallowing path (`cmd_hook` → `cmd_hook_inner`) is
  the highest-value debug site. Errors there are currently invisible since Claude Code does not
  surface hook stderr. `debug!()` calls must cover: raw stdin input, each `eprintln!` error site in
  `hook.rs`, and the dispatch/output boundaries.

## Open Questions

### Resolved During Planning

- **Should debug be a CLI flag or env var?** Env var — hooks are invoked by Claude Code which sets
  env vars but cannot pass extra CLI flags. The env var works uniformly across CLI and hook
  invocations.
- **Should we add `--format` instead of `--json`?** No — YAGNI. `--json` is simpler and matches the
  roadmap item. If more formats are needed later, we can evolve.
- **Should `--json` be per-command or global?** Global — avoids duplication across commands, is the
  standard CLI pattern, and future commands get it for free. Commands that don't support it simply
  ignore the flag.

### Deferred to Implementation

- Exact debug message wording — will be refined during implementation based on what is most useful
  when reading actual output.

## Implementation Units

- [x] **Unit 1: Debug macro infrastructure**

  **Goal:** Create a lightweight `debug!()` macro that conditionally emits diagnostics to stderr
  based on `LORE_DEBUG` env var.

  **Requirements:** R1, R3

  **Dependencies:** None

  **Files:**
  - Create: `src/debug.rs`
  - Modify: `src/lib.rs` (add `pub mod debug`)

  **Approach:**
  - Define a `static IS_DEBUG: LazyLock<bool>` that reads `LORE_DEBUG` env var once and caches the
    result
  - Define a public `fn is_debug() -> bool` that dereferences the `LazyLock`
  - Define a `debug!()` macro that calls `is_debug()` and, if true, calls
    `eprintln!("[lore debug] {}", format_args!(...))`
  - Export from `lib.rs` as `pub mod debug`

  **Patterns to follow:**
  - Existing `eprintln!` diagnostic pattern throughout the codebase
  - `LazyLock` is in `std::sync` (stable since Rust 1.80, well within MSRV 1.85)

  **Test scenarios:**
  - Happy path: `is_debug()` returns true when `LORE_DEBUG=1` is set
  - Happy path: `is_debug()` returns false when `LORE_DEBUG` is unset
  - Edge case: `LORE_DEBUG=0` returns false
  - Edge case: `LORE_DEBUG=true` returns true (lenient parsing)

  **Verification:**
  - `debug!("test message")` compiles and emits to stderr only when `LORE_DEBUG=1`

- [x] **Unit 2: Instrument hook pipeline with debug logging**

  **Goal:** Add `debug!()` calls at key decision points in the hook pipeline.

  **Requirements:** R1, R3

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `src/hook.rs`
  - Modify: `src/main.rs` (cmd_hook_inner instrumentation)

  **Approach:**
  - Priority 1 — error-swallowing paths (currently invisible failures):
    - `cmd_hook_inner` in `main.rs`: log raw stdin input at entry
    - Each `eprintln!("lore hook: ...")` site in `hook.rs` (lines 118, 197, 215): add `debug!()`
      with full error context alongside the existing `eprintln!`
  - Priority 2 — pipeline decision points:
    - `handle_hook`: event name received, session_id
    - `extract_query`: extracted query string (or "no query extracted")
    - `search_with_threshold`: query, embed success/fallback, result count, threshold applied,
      results after filtering (titles + scores)
    - `handle_pre_tool_use`: dedup state (file path, seen count, filtered count)
    - `handle_pre_tool_use`: final output (injecting N chunks from M sources)
    - `handle_post_tool_use`: stderr extraction, constructed query
  - All output via `debug!()` — never appears without `LORE_DEBUG=1`

  **Patterns to follow:**
  - Existing `eprintln!("lore hook: ...")` pattern for error diagnostics

  **Test scenarios:**
  - Happy path: with `LORE_DEBUG=1`, PreToolUse hook emits debug lines to stderr containing the
    extracted query
  - Happy path: with `LORE_DEBUG=1`, search_with_threshold emits result count and threshold to
    stderr
  - Happy path: without `LORE_DEBUG`, no debug output on stderr (only existing warnings/errors)

  **Verification:**
  - Running a hook with `LORE_DEBUG=1` produces visible diagnostic output on stderr showing the full
    pipeline flow

- [x] **Unit 3: Instrument cmd_search and cmd_ingest with debug logging**

  **Goal:** Add debug output for search and ingest commands so users can troubleshoot outside the
  hook context too.

  **Requirements:** R2, R3

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `src/main.rs`

  **Approach:**
  - `cmd_search`: log query, config values (top_k, min_relevance, hybrid), result count
  - `cmd_ingest`: log knowledge dir, delta vs full mode, files processed/skipped/errored

  **Patterns to follow:**
  - Existing `eprintln!` status messages in `cmd_ingest`

  **Test scenarios:**
  - Happy path: `LORE_DEBUG=1 lore search --config <path> rust` emits debug lines to stderr showing
    query and config
  - Happy path: debug output goes to stderr, not stdout (pipe stdout and confirm no debug prefix in
    it)

  **Verification:**
  - Debug output visible on stderr when `LORE_DEBUG=1` is set for both commands

- [x] **Unit 4: Add Serialize to SearchResult and PatternSummary**

  **Goal:** Enable JSON serialization of the data types used by search and list.

  **Requirements:** R4, R5

  **Dependencies:** None

  **Files:**
  - Modify: `src/database.rs`

  **Approach:**
  - Add `Serialize` to the existing `#[derive(...)]` on `SearchResult` (line 62) and
    `PatternSummary` (line 80)
  - serde is already in dependencies — no Cargo.toml change needed

  **Patterns to follow:**
  - `HookOutput` and `HookSpecificOutput` in `src/hook.rs` already derive `Serialize`

  **Test expectation:** none — pure derive addition, verified transitively by Unit 5 tests

  **Verification:**
  - `serde_json::to_string(&result)` compiles for both types

- [x] **Unit 5: Add --json flag to search and list commands**

  **Goal:** Add `--json` CLI flag that switches output to a JSON array on stdout.

  **Requirements:** R4, R5, R6, R7

  **Dependencies:** Unit 4

  **Files:**
  - Modify: `src/main.rs`
  - Modify: `tests/smoke.rs`

  **Approach:**
  - Add `#[arg(long, global = true)] json: bool` to the `Cli` struct (global flag, alongside
    `--config`)
  - Thread `cli.json` through the command dispatch match to `cmd_search` and `cmd_list` function
    signatures
  - In `cmd_search`: if `json`, serialize full results vec as JSON array via
    `serde_json::to_string(&results)?` to stdout (no truncation, no separator lines)
  - In `cmd_list`: if `json`, serialize patterns vec as JSON array to stdout
  - Human-format path unchanged (existing output preserved exactly)
  - "No results found." stderr message still emitted even with `--json` (empty array on stdout)
  - Other commands ignore the `json` flag

  **Patterns to follow:**
  - `cmd_hook` already outputs JSON via `serde_json::to_string` + `println!`

  **Test scenarios:**
  - Happy path: `lore search --json --config <path> rust` outputs valid JSON array with expected
    fields (title, body, tags, source_file, heading_path, score)
  - Happy path: `lore list --json --config <path>` outputs valid JSON array with expected fields
    (title, source_file, tags)
  - Happy path: search JSON output includes full body (not truncated at 500 bytes)
  - Edge case: `lore search --json --config <path> nonexistent` outputs empty JSON array `[]`
  - Edge case: `lore list --json` on empty database outputs empty JSON array `[]`
  - Integration: `--json` and `--top-k` compose correctly (JSON array respects top_k limit)

  **Verification:**
  - JSON output is parseable by `serde_json::from_str` round-trip
  - Human output unchanged when `--json` is not passed (existing tests still pass)

## System-Wide Impact

- **Interaction graph:** Debug logging is write-only to stderr — no callbacks, no middleware
  changes, no API surface changes
- **Error propagation:** Unchanged — `debug!()` never returns errors or affects control flow
- **State lifecycle risks:** None — `LazyLock` is read-only after first access
- **`lore serve` safety:** Debug output goes to stderr via `eprintln!`. MCP uses stdout for
  JSON-RPC, so `LORE_DEBUG=1` is safe during `lore serve` — debug lines will not interfere with the
  protocol
- **API surface parity:** The MCP `search_patterns` tool (in `server.rs`) already returns JSON via
  the MCP protocol — the `--json` flag brings CLI parity
- **Unchanged invariants:** Hook JSON output format is unchanged. All existing stderr diagnostics
  remain. Human-format output for search and list is unchanged when `--json` is not passed.

## Risks & Dependencies

| Risk                                                         | Mitigation                                                                                      |
| ------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| Debug output in hooks could be noisy for long sessions       | Prefix all lines with `[lore debug]` so they're greppable; only active when explicitly opted in |
| LazyLock caching means env var change mid-process is ignored | Acceptable — env vars are set at process start for CLI tools                                    |

## Sources & References

- Related roadmap items: `ROADMAP.md` lines 34-37
- Existing JSON output pattern: `src/hook.rs` `HookOutput` serialization
- CLI data/diagnostic separation:
  `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md`
