---
title: "feat: Add end-to-end lifecycle tests"
type: feat
status: active
date: 2026-03-31
---

# feat: Add end-to-end lifecycle tests

## Overview

Add integration tests that exercise the full data lifecycle: ingest markdown files, search for
patterns, create new patterns via write operations, and verify search reflects every mutation.
Closes the primary coverage gap between isolated unit tests and untested cross-module flows.

## Problem Frame

The codebase has 96 unit tests covering each module in isolation, and 6 CLI smoke tests covering
argument parsing. No test verifies that data flows correctly through the entire chain — a write
operation that indexes correctly in isolation might fail to appear in a subsequent search if the
database layer, chunking, or embedding pipeline interact incorrectly. The ROADMAP identifies this as
the next priority item.

## Requirements Trace

- R1. A test exercises the full ingest → search → add_pattern → search finds new pattern flow
- R2. Search correctness is verified after each mutation (add, update, append)
- R3. Re-ingest correctly replaces stale data when files are deleted or modified
- R4. MCP JSON-RPC round-trip exercises chained tool calls through the server dispatch layer
- R5. All tests run without Ollama (deterministic, CI-safe)
- R6. Tests use on-disk SQLite to exercise the same WAL/file path as production

## Scope Boundaries

- No CLI-level e2e tests (CLI's `init` requires Ollama; `serve` requires stdin piping)
- No snapshot testing for search scores (fragile); assert on presence/absence of results
- No testing of `provision` or `cmd_init` (requires live Ollama)
- No new test framework dependencies

## Context & Research

### Relevant Code and Patterns

- `src/embeddings.rs:142-198` — `FakeEmbedder` is `pub(crate)` under `#[cfg(test)]` at module level
  (not inside `mod tests`). Produces deterministic vectors via FNV-1a hash + xorshift64. Used by
  `database`, `ingest`, and `server` unit tests.
- `src/database.rs:82-94` — `KnowledgeDB::open(path, dims)` + `.init()`. Already `pub`. Accepts both
  `:memory:` and file paths. Registers sqlite-vec via process-global `Once`.
- `src/ingest.rs:53-131` — `ingest()` walks a directory, chunks, embeds, inserts. Clears DB first.
- `src/ingest.rs:138-179` — `add_pattern()` creates file, indexes, commits to git.
- `src/ingest.rs:181-219` — `update_pattern()` overwrites file, re-indexes, commits.
- `src/ingest.rs:221-266` — `append_to_pattern()` appends section, re-indexes, commits.
- `src/server.rs:119-168` — `handle_request()` dispatches JSON-RPC. Private to module.
- `src/server.rs:583-628` — `TestHarness` bundles DB + FakeEmbedder + Config + TempDir. Private to
  `server::tests`.
- `src/ingest.rs:423-436` — `git_init()` helper sets test identity + disables GPG signing.
- `tests/smoke.rs` — integration tests use `assert_cmd::Command::cargo_bin("lore")`.

### Institutional Learnings

- `docs/solutions/build-errors/sqlite-vec-no-rust-export-register-via-ffi.md` — sqlite-vec
  registration is process-global via `Once`; every connection gets it automatically. Tests that open
  DB connections do not need special setup beyond calling `KnowledgeDB::open()`.

## Key Technical Decisions

- **Cargo feature `test-support` to expose `FakeEmbedder` to integration tests:** Integration tests
  in `tests/` are separate crates. The library is compiled as a normal dependency for them, so
  `#[cfg(test)]` items are NOT available (cfg(test) is only set when the library itself is the test
  target). A Cargo feature `test-support` solves this: gate `FakeEmbedder` with
  `#[cfg(any(test, feature = "test-support"))]` and pass `--features test-support` when running
  tests. Unit tests still see `FakeEmbedder` via `cfg(test)`; integration tests see it via the
  feature flag. The feature is never enabled in release builds.

- **Library-level e2e in `tests/e2e.rs`, not CLI-level:** The CLI's `init` command requires Ollama
  running. The library API (`ingest::*`, `database::*`) is the actual contract the MCP server calls.
  CLI argument parsing is already covered by `tests/smoke.rs`.

- **On-disk temp SQLite DB, not `:memory:`:** Exercises the same WAL mode, file creation, and
  locking behaviour as production. Verifies the DB file is actually created and populated.

- **MCP round-trip test stays inside `src/server.rs`:** `handle_request` and `ServerContext` are
  intentionally private. Adding a chained test inside `server::tests` (using the existing
  `TestHarness`) avoids exposing internal types in the public API.

- **Single sequential test for the lifecycle flow, separate test for re-ingest:** The lifecycle flow
  (R1, R2) is inherently sequential — each mutation depends on prior state. One test with clear
  assert-per-step is more readable than splitting into independent tests that duplicate setup. The
  re-ingest scenario (R3) is a distinct concern with its own setup, so it lives in a separate test
  function.

## Open Questions

### Resolved During Planning

- **Can `tests/e2e.rs` access `FakeEmbedder`?** Not with `#[cfg(test)]` alone — the library is
  compiled without `cfg(test)` when used as a dependency of integration tests. Resolved by adding a
  Cargo feature `test-support` and gating with `#[cfg(any(test, feature = "test-support"))]`.

- **Where does the MCP round-trip test live?** Inside `src/server.rs::tests` using existing
  `TestHarness`, keeping `handle_request` and `ServerContext` private.

### Deferred to Implementation

- **Exact fixture content:** The test data should use terms distinctive enough for FTS to match
  unambiguously. Final wording will be tuned during implementation.

## Implementation Units

- [ ] **Unit 1: Add `test-support` feature and expose `FakeEmbedder` to integration tests**

  **Goal:** Make `FakeEmbedder` accessible from `tests/e2e.rs` via a Cargo feature, without
  including it in release builds.

  **Requirements:** R5

  **Dependencies:** None

  **Files:**
  - Modify: `Cargo.toml`
  - Modify: `src/embeddings.rs`
  - Modify: `justfile`

  **Approach:**
  - Add `[features] test-support = []` to `Cargo.toml`
  - Change `#[cfg(test)]` on `FakeEmbedder` (struct + all impl blocks) to
    `#[cfg(any(test, feature = "test-support"))]`
  - Change `pub(crate)` to `pub` on the struct
  - Update the `test` recipe in `justfile` to `cargo test --features test-support`
  - Update the `ci` recipe if it calls `test` indirectly (it does — `ci: fmt clippy test deny doc`)

  **Patterns to follow:**
  - Standard Rust feature-gated test support pattern
  - Existing `#[cfg(test)]` gating on `FakeEmbedder`

  **Test scenarios:**
  - Happy path: `cargo test --features test-support` passes (all existing tests still pass)
  - Happy path: `cargo build --release` does NOT include `FakeEmbedder` (feature not enabled)
  - Integration: `tests/e2e.rs` can import `lore::embeddings::FakeEmbedder` (verified by Unit 2)

  **Verification:**
  - `cargo test --features test-support` passes
  - `cargo clippy --all-targets --features test-support -- -D warnings` clean
  - `cargo build --release` succeeds (FakeEmbedder excluded)

- [ ] **Unit 2: Add library-level e2e tests in `tests/e2e.rs`**

  **Goal:** Test the full lifecycle (ingest → search → add → search → update → search → append →
  search) and re-ingest data replacement, using the library API with on-disk SQLite.

  **Requirements:** R1, R2, R3, R5, R6

  **Dependencies:** Unit 1

  **Files:**
  - Create: `tests/e2e.rs`

  **Approach:**
  - Each test creates a `tempdir`, seeds it with markdown files, initialises git, opens an on-disk
    `KnowledgeDB` in the tempdir, and uses `FakeEmbedder`
  - `full_lifecycle` test: sequential steps with assertions after each mutation
  - `ingest_replaces_stale_data` test: ingest, modify/delete files, re-ingest, verify
  - Helper function `git_init(dir)` following the pattern from `src/ingest.rs:423-436`
  - Assert on search result presence/absence and title/body content, not on scores
  - Use `db.stats()` to verify aggregate chunk/source counts after operations

  **Patterns to follow:**
  - `src/ingest.rs:423-436` — `git_init()` helper (init repo, set test identity, disable GPG)
  - `src/ingest.rs:459-489` — `ingest_tempdir_with_markdown_files` test structure
  - `src/database.rs:432-449` — assertion style for search results

  **Test scenarios:**

  `full_lifecycle`:
  - Happy path: ingest 3 markdown files → `IngestResult` has 3 files processed, >0 chunks, no errors
  - Happy path: FTS search for a term unique to one file → returns that file's pattern
  - Happy path: hybrid search with embedding → returns results (non-empty)
  - Happy path: `add_pattern` → `WriteResult` with file created on disk, chunks indexed > 0,
    committed to git
  - Happy path: FTS search for newly added pattern's content → found in results
  - Happy path: `update_pattern` with new body → old body absent from search results, new body
    present
  - Happy path: `append_to_pattern` with new section → appended content found in search results
  - Happy path: `db.stats()` at end reflects total chunks/sources from all operations

  `ingest_replaces_stale_data`:
  - Happy path: ingest 2 files → stats show 2 sources
  - Happy path: delete one file, modify the other, re-ingest → stats show 1 source
  - Happy path: search for deleted file's content → not found
  - Happy path: search for modified file's new content → found
  - Edge case: search for modified file's old content → not found

  **Verification:**
  - `cargo test --features test-support --test e2e` passes
  - `cargo clippy --all-targets --features test-support -- -D warnings` clean

- [ ] **Unit 3: Add MCP round-trip test in `src/server.rs`**

  **Goal:** Test chained JSON-RPC operations through the server dispatch layer, verifying that a
  pattern created via `add_pattern` is discoverable via `search_patterns`, and that `update_pattern`
  and `append_to_pattern` mutations are reflected in subsequent searches.

  **Requirements:** R4, R5

  **Dependencies:** None (uses existing `TestHarness` and private types)

  **Files:**
  - Modify: `src/server.rs` (add test to existing `#[cfg(test)] mod tests`)

  **Approach:**
  - Single test function `mcp_round_trip` using `TestHarness`
  - Sequential JSON-RPC calls: initialize → tools/list → add_pattern → search_patterns →
    update_pattern → search_patterns → append_to_pattern → search_patterns
  - Each step asserts on the response (no error, expected content)
  - Uses `request_value()` for assertions on `serde_json::Value`

  **Patterns to follow:**
  - `src/server.rs:717-742` — `add_pattern_creates_pattern` test structure
  - `src/server.rs:680-712` — `search_patterns_returns_results` assertion style
  - `src/server.rs:854-903` — `search_fts_only_path` for config variation pattern

  **Test scenarios:**
  - Happy path: `initialize` returns protocol version and server info
  - Happy path: `tools/list` returns 4 tools
  - Happy path: `add_pattern` with title and body → response contains "saved to"
  - Integration: `search_patterns` after add → response text contains the added pattern's title
  - Happy path: `update_pattern` with new body → response contains "updated"
  - Integration: `search_patterns` after update → response text contains new body content, not old
  - Happy path: `append_to_pattern` with heading and body → response contains "appended"
  - Integration: `search_patterns` after append → response text contains appended content

  **Verification:**
  - `cargo test server::tests::mcp_round_trip` passes
  - `cargo clippy --all-targets -- -D warnings` clean

## System-Wide Impact

- **Interaction graph:** Unit 1 adds a Cargo feature and widens `FakeEmbedder` visibility — no
  runtime impact, test-only. Units 2-3 are pure test additions with no production code changes.
- **Error propagation:** Not applicable — tests only.
- **Unchanged invariants:** All existing public APIs, module boundaries, and production behaviour
  remain untouched. `FakeEmbedder` is gated behind `cfg(any(test, feature = "test-support"))` and
  excluded from release builds.
- **Integration coverage:** Units 2 and 3 close the primary gap — cross-module data flow through
  ingest → database → search, and chained MCP tool calls through the server dispatch layer.

## Risks & Dependencies

| Risk                                                                                        | Mitigation                                                                                                               |
| ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| FTS query syntax is sensitive to phrasing — tests may be brittle if fixture terms overlap   | Use distinctive, non-overlapping terms in each fixture file (e.g., "anyhow" for error handling, "snake_case" for naming) |
| On-disk SQLite in tempdir may behave differently across platforms (WAL locking)             | `tempfile` creates proper OS-level temp dirs; SQLite WAL is well-tested on all platforms                                 |
| `test-support` feature adds build config surface                                            | Feature is well-scoped (one purpose), justfile handles the flag, CI inherits via `just test`                             |
| `cargo test` without `--features test-support` skips integration tests needing FakeEmbedder | Justfile's `test` recipe always passes the flag; plain `cargo test` still runs unit tests + smoke                        |

## Sources & References

- Related code: `src/embeddings.rs` (FakeEmbedder), `src/ingest.rs` (ingest, write ops),
  `src/database.rs` (search), `src/server.rs` (TestHarness, handle_request)
- Institutional learning:
  `docs/solutions/build-errors/sqlite-vec-no-rust-export-register-via-ffi.md`
- ROADMAP item: "End-to-end testing with real data"
