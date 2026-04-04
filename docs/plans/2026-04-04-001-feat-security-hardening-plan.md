---
title: "feat: Security hardening — input validation, bounded reads, dedup integrity"
type: feat
status: completed
date: 2026-04-04
origin: docs/brainstorms/2026-04-04-security-hardening-requirements.md
deepened: 2026-04-04
---

# Security Hardening

## Overview

Close remaining security gaps at lore's trust boundaries before release: enforce input length limits
on MCP tool arguments, validate and bound transcript file reads in the hook pipeline, harden dedup
file concurrency and naming, expand FTS5 sanitisation test coverage, and document the threat model
in `SECURITY.md`.

## Problem Frame

Lore accepts input from agent hooks (Claude Code today, Cursor and Opencode soon), MCP clients over
stdio, CLI arguments, and markdown files on disk. While the codebase has strong foundations
(parameterised SQL, path traversal protection, FTS5 sanitisation, no shell execution, `unsafe`
denied globally), several gaps remain: unbounded string inputs at the MCP boundary, an unvalidated
transcript path in the hook pipeline, an unbounded transcript file read, a TOCTOU race on the dedup
file, and collision-prone session ID filenames. These are resource exhaustion and unintended file
access vectors that need closing before release.

(see origin: `docs/brainstorms/2026-04-04-security-hardening-requirements.md`)

## Requirements Trace

- R1. Enforce max length on MCP string inputs and cap `top_k`
- R2. Test MCP input limits inline in server tests
- R3. Validate transcript path is under `$HOME`; skip silently on failure
- R4. Bound transcript read to last ~32KB with partial-line/UTF-8 handling
- R5. Test transcript validation and bounded read inline in hook tests
- R6. Prevent dedup file race via advisory locking across read-filter-write
- R7. Hash session IDs for dedup filenames (deterministic, 16 hex chars)
- R8. Test session ID hashing and locking inline in hook tests
- R9. Expand FTS5 sanitisation tests for remaining gaps
- R10. Create `SECURITY.md` with threat model, trust boundaries, reporting

## Scope Boundaries

- No global memory limit or custom allocator
- No authentication or authorisation
- No network hardening (stdio MCP, localhost Ollama)
- No contributor security guidelines (project closed to contributions)
- Transcript path validation uses `$HOME`, not per-agent config
- Security tests baked into domain test modules, no separate security suite

## Context & Research

### Relevant Code and Patterns

- `src/ingest.rs:597-607` — `validate_within_dir()`: `canonicalize` + `starts_with` pattern to reuse
  for transcript path validation
- `src/database.rs:444-461` — `sanitize_fts_query()`: existing sanitisation with 8 inline unit tests
  covering most operators
- `src/hook.rs:457-463` — `dedup_file_path()`: current char-level sanitisation to be replaced with
  hashing
- `src/hook.rs:467-490` — `read_dedup()` / `write_dedup()`: no locking, append only
- `src/hook.rs:653-672` — `last_user_message()`: unbounded `read_to_string`
- `src/server.rs:286-313` — `handle_tool_call()` dispatch; each handler does its own field
  extraction and validation
- `src/server.rs` `TestHarness` pattern for server unit tests

### Institutional Learnings

- FTS5 interprets `.` `/` `\` `:` `{` `}` `[` `]` `"` `'` `*` `^` as syntax operators — all must be
  sanitised before MATCH (see
  `docs/solutions/database-issues/fts5-query-sanitization-crashes-on-special-chars-2026-04-02.md`)
- Session dedup must gate on file existence (SessionStart creates the file); deny-first-touch
  without dedup creates infinite loops (see
  `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`)

## Key Technical Decisions

- **`fd-lock` for advisory file locking (R6):** Atomic-write-via-rename does not prevent the race —
  two concurrent readers can both compute additions from the same base state and the second rename
  overwrites the first's additions. `fd-lock` (MIT/Apache-2.0) provides exclusive locking on file
  handles, naturally spanning the read-filter-write sequence. Chosen over `fs2` because `fs2` has an
  unmaintained advisory (RUSTSEC-2023-0035) that would fail `cargo deny check`. One new direct
  dependency.
- **Inline FNV-1a for session ID hashing (R7):** Zero new dependencies.
  `std::hash::DefaultHasher::new()` is deprecated in edition 2024 and Clippy pedantic will flag it.
  Instead, use a simple inline FNV-1a hash (same pattern already used by `FakeEmbedder` in
  `src/embeddings.rs`). FNV-1a is deterministic, fast, and has good distribution for short strings
  like session IDs. Format the `u64` as 16 zero-padded lowercase hex chars. Dedup files are
  ephemeral (session-scoped, `/tmp`), so cryptographic strength is irrelevant.
- **Per-handler input validation (R1):** Matches the existing pattern where each handler checks its
  own required fields. A small helper function (`check_limit`) reduces repetition without adding
  abstraction.
- **Silent skip on transcript validation failure (R3):** Consistent with the existing fallthrough
  where `last_user_message` returns `None`. No error, no bail — just skip the transcript signal.
- **`String::from_utf8_lossy` for bounded read buffer (R4):** The 32KB tail buffer may start
  mid-UTF-8 sequence. Lossy conversion replaces the partial character with a replacement char, which
  is then discarded when we skip to the first `\n`. Simpler than manual char-boundary detection.

## Open Questions

### Resolved During Planning

- **Which locking crate?** `fd-lock` — MIT/Apache-2.0, actively maintained. `fs2` rejected due to
  RUSTSEC-2023-0035 unmaintained advisory (would fail `cargo deny check`). `std::os::unix` does not
  expose `flock` in stable Rust.
- **Which hash function?** Inline FNV-1a — zero-dep, deterministic, good distribution for short
  strings. `DefaultHasher::new()` rejected because it is deprecated in edition 2024 and Clippy
  pedantic flags it. The `FakeEmbedder` already uses this exact FNV-1a pattern in the codebase.
- **Seek strategy for bounded read?** `SeekFrom::End(-32768)` with `SeekFrom::Start(0)` fallback for
  small files. Discard everything before the first `\n` in the buffer.

### Deferred to Implementation

- Exact `check_limit` helper signature — depends on how the existing `error_response` pattern
  composes. Note: `top_k` is a numeric bound (not a string length), so it needs a separate inline
  check rather than the string helper
- Whether `fd-lock` needs any `deny.toml` additions for license allowlisting

## Implementation Units

- [x] **Unit 1: MCP input validation**

  **Goal:** Enforce length limits on all MCP tool string inputs and cap `top_k`.

  **Requirements:** R1, R2

  **Dependencies:** None

  **Files:**
  - Modify: `src/server.rs`
  - Test: `src/server.rs` (inline `mod tests`)

  **Approach:**
  - Add a small helper (e.g. `check_limit(value, field_name, max_bytes)`) that returns an
    `error_response` if the string exceeds the limit. Keep it file-local, not exported.
  - Call it in each handler after extracting the field value:
    - `handle_search`: `query` ≤ 1024 bytes, `top_k` ≤ 100
    - `handle_add`: `title` ≤ 512 bytes, `body` ≤ 262144 bytes (256KB)
    - `handle_update`: `source_file` ≤ 512 bytes, `body` ≤ 262144 bytes
    - `handle_append`: `source_file` ≤ 512 bytes, `heading` ≤ 512 bytes, `body` ≤ 262144 bytes
    - `handle_add` and `handle_update`: `tags` array — cap total serialised size at ~8KB (reasonable
      ceiling for tag metadata)
  - `top_k` is a numeric bound, not a string length — validate it inline (e.g.
    `if top_k > 100 { return error_response(...) }`) rather than through the string helper.
  - Use JSON-RPC error code `-32000` (application error) with a message like
    `"query exceeds maximum length of 1024 bytes"`.

  **Patterns to follow:**
  - Existing per-handler required-field checks in `handle_search`, `handle_add`, etc.
  - `error_response()` helper in `src/server.rs`

  **Test scenarios:**
  - Happy path: query at exactly 1024 bytes succeeds
  - Edge case: query at 1025 bytes returns `-32000` error with descriptive message
  - Edge case: body at 256KB succeeds; body at 256KB + 1 byte rejects
  - Edge case: `top_k` at 100 succeeds; `top_k` at 101 rejects
  - Error path: oversized `title` in `add_pattern` returns error, does not write to disk
  - Error path: oversized `source_file` in `update_pattern` returns error, does not attempt file
    read

  **Verification:**
  - All four handlers reject oversized inputs with descriptive `-32000` errors
  - Existing server tests still pass unchanged

- [x] **Unit 2: Dedup file hardening (hashing + locking)**

  **Goal:** Replace character-level session ID sanitisation with deterministic hashing, and wrap the
  read-filter-write sequence in an exclusive file lock.

  **Requirements:** R6, R7, R8

  **Dependencies:** None (independent of Unit 1)

  **Files:**
  - Modify: `src/hook.rs`
  - Modify: `Cargo.toml` (add `fd-lock` dependency)
  - Test: `src/hook.rs` (inline `mod tests`)
  - Test: `tests/hook.rs` (integration tests)

  **Approach:**

  _Session ID hashing (R7):_
  - Replace the body of `dedup_file_path()` with an inline FNV-1a hash of the session ID bytes,
    formatted as 16 zero-padded lowercase hex chars. Filename becomes `lore-session-{hash}`. Follow
    the same FNV-1a pattern used by `FakeEmbedder::embed()` in `src/embeddings.rs`.
  - The existing tests `dedup_file_path_sanitizes_uuid` and
    `dedup_file_path_sanitizes_special_chars` must be **rewritten** (not just supplemented) — their
    hardcoded expected filenames will no longer match.

  _Advisory file locking (R6):_
  - Add `fd-lock` to `[dependencies]` in `Cargo.toml`.
  - Refactor the dedup read-filter-write sequence: open the file once with
    `OpenOptions::new().create(true).read(true).append(true)`, acquire an exclusive lock via
    `fd-lock`, read IDs from the locked handle, then append new IDs to the same handle. The lock
    releases when the guard drops.
  - The lock wraps the entire sequence in `handle_pre_tool_use` — not inside
    `read_dedup`/`write_dedup` individually. Prefer inline locking at each call site (two sites:
    `handle_pre_tool_use` and `reset_dedup`) over a closure-taking abstraction — two call sites
    don't justify the indirection.
  - `reset_dedup` (called by PostCompact) also needs to acquire the lock before truncating.
  - Note: the lock is held across the database search + format phase between read and write. For a
    single-user local tool this is acceptable — the search is fast and concurrent hook invocations
    for the same session are rare.

  **Patterns to follow:**
  - Existing `dedup_file_path` / `read_dedup` / `write_dedup` structure
  - `anyhow::Result` for error propagation
  - Hook contract: errors are swallowed by `cmd_hook` — locking failures should not crash the agent

  **Test scenarios:**
  - Happy path: `dedup_file_path("abc-123")` returns a deterministic path with a 16-char hex suffix
  - Happy path: same session ID always produces the same hash
  - Edge case: similar IDs (`"abc:123"` vs `"abc/123"`) produce _different_ hashes (collision
    avoidance — the whole point of replacing sanitisation)
  - Edge case: empty session ID produces a valid path (not a bare directory)
  - Happy path: locked read-filter-write correctly appends new IDs
  - Integration: two sequential invocations with the same session ID correctly accumulate IDs (no
    lost writes)
  - Happy path: `reset_dedup` clears all IDs under lock

  **Verification:**
  - `cargo test --features test-support` passes
  - Existing hook integration tests in `tests/hook.rs` pass (they exercise the dedup pipeline
    end-to-end)
  - `cargo deny check` passes with `fd-lock` added

- [x] **Unit 3: Transcript path validation and bounded read**

  **Goal:** Validate transcript paths are under `$HOME` and bound the file read to the last 32KB.

  **Requirements:** R3, R4, R5

  **Dependencies:** None (independent of Units 1-2)

  **Files:**
  - Modify: `src/hook.rs`
  - Test: `src/hook.rs` (inline `mod tests`)

  **Approach:**

  _Path validation (R3):_
  - Before calling `last_user_message`, validate the transcript path: `canonicalize` the path, then
    check `starts_with(home_dir)` where `home_dir` comes from `std::env::var("HOME")` (or
    `dirs::home_dir()` — but `std::env::var` is simpler and avoids a new dependency).
  - If `canonicalize` fails or path is outside `$HOME`, return `None` (skip transcript signal). No
    error, no bail.
  - Extract this into a small helper like `validate_transcript_path(path) ->
    Option<PathBuf>`
    for testability.

  _Bounded read (R4):_
  - Replace `std::fs::read_to_string(path)` in `last_user_message` with:
    1. Open the file, seek to `SeekFrom::End(-32768)` (or `Start(0)` if file is smaller)
    2. Read the remaining bytes into a buffer
    3. Convert with `String::from_utf8_lossy`
    4. Find the first `\n` and discard everything before it (partial JSONL line)
    5. Iterate remaining lines in reverse (same logic as current implementation)
  - Import `std::io::{Read, Seek, SeekFrom}`.

  **Patterns to follow:**
  - `validate_within_dir` in `src/ingest.rs` for the canonicalize + starts_with pattern
  - Existing `last_user_message` reverse-line scan logic

  **Test scenarios:**
  - Happy path: transcript path under `$HOME` passes validation, returns the last user message
  - Edge case: transcript path outside `$HOME` (e.g. `/tmp/evil.jsonl`) returns `None`
  - Edge case: nonexistent transcript path returns `None` (canonicalize fails)
  - Edge case: transcript path that is a symlink resolving outside `$HOME` returns `None`
  - Happy path: bounded read on a small file (< 32KB) reads the whole file and finds the last user
    message
  - Happy path: bounded read on a large file (> 32KB) reads only the tail and finds the last user
    message near the end
  - Edge case: bounded read correctly discards the first partial line (buffer starts mid-JSONL-line)
  - Edge case: file with no user messages returns `None`

  **Verification:**
  - Existing hook tests pass (they use fixture transcript content)
  - New inline tests cover all validation and boundary cases

- [x] **Unit 4: FTS5 sanitisation test coverage**

  **Goal:** Close remaining gaps in FTS5 sanitisation test coverage.

  **Requirements:** R9

  **Dependencies:** None

  **Files:**
  - Modify: `src/database.rs` (inline `mod tests`)

  **Approach:**
  - Audit the existing 8 tests against the full operator list: `. / \ : { } [ ] " ' * ^ -`
  - Add individual test for backslash (`\`) — currently only tested as part of
    `sanitize_strips_special_chars` but not isolated
  - Add combined multi-operator sequence test: e.g. `"foo/bar:baz\\qux"` → `"foo bar baz qux"`
  - Add test for input containing only operators: `"/:.\\"` → `""`
  - Verify parentheses and `AND`/`OR`/`NOT` keywords are preserved (already tested but confirm
    coverage)

  **Patterns to follow:**
  - Existing `sanitize_*` test naming and structure in `src/database.rs`

  **Test scenarios:**
  - Edge case: backslash alone → space: `"foo\\bar"` → `"foo bar"`
  - Edge case: combined operators: `"foo/bar:baz"` → `"foo bar baz"`
  - Edge case: all-operators input → empty string
  - Edge case: operators mixed with valid terms: `"rust-lang/rust:main"` → `"rust lang rust main"`
    (leading minus stripped from "lang" if applicable)

  **Verification:**
  - `cargo test --features test-support` passes
  - Every operator character from the sanitisation match arm has at least one dedicated or combined
    test exercising it

- [x] **Unit 5: SECURITY.md**

  **Goal:** Document the threat model, trust boundaries, and security reporting process.

  **Requirements:** R10

  **Dependencies:** Units 1-4 (write after code changes so the document reflects the final state)

  **Files:**
  - Create: `SECURITY.md`

  **Approach:**
  - Structure as: Threat Model (local single-user tool, stdio MCP, localhost Ollama), Trust
    Boundaries (table of input surfaces with trust level and validation), Assumptions, Security
    Reporting (how to report vulnerabilities — discussions or direct contact, no issues/PRs from
    external contributors).
  - Reference specific code: `validate_within_dir`, `sanitize_fts_query`, input length limits,
    transcript path validation, dedup file locking.
  - Keep it concise — this is a local CLI tool, not a web service.

  **Patterns to follow:**
  - Existing `CONTRIBUTING.md` tone (clear, no-nonsense)

  **Test expectation:** none — pure documentation

  **Verification:**
  - Document covers all trust boundaries identified in the origin requirements
  - Security reporting instructions are clear and actionable

## System-Wide Impact

- **Interaction graph:** MCP server handlers (Unit 1) and hook pipeline (Units 2-3) are independent
  code paths. No cross-unit interactions beyond shared database access (already handled by SQLite
  WAL + busy_timeout).
- **Error propagation:** MCP input limit violations return JSON-RPC `-32000` errors.
  Transcript/dedup failures are swallowed by the hook's error contract (`cmd_hook` catches all
  errors, exits 0). No new error paths surface to the user.
- **State lifecycle risks:** Dedup file locking (Unit 2) introduces a new failure mode: if lore
  crashes while holding the lock, the OS releases it on process exit. Advisory locks on Unix are
  tied to file descriptors, not files — no stale lock risk.
- **API surface parity:** MCP tool schemas do not change — limits are enforced server-side, not
  declared in `inputSchema`. No client-facing contract change.
- **Unchanged invariants:** Search behaviour, ingestion, git operations, and config loading are
  unaffected. The hook pipeline's external contract (JSON on stdout, exit 0 always) is preserved.

## Risks & Dependencies

| Risk                                                     | Mitigation                                                                                                                                            |
| -------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fd-lock` fails `cargo deny` check                       | MIT/Apache-2.0, actively maintained. Verify in implementation.                                                                                        |
| FNV-1a hash output changes if implementation is modified | Hash is inline code under our control — no external version risk. Dedup files are ephemeral (`/tmp`, session-scoped) regardless.                      |
| Bounded read misses last user message in edge cases      | Fallback: if no user message found in 32KB tail, return `None` (same as current behaviour for empty transcripts). Search proceeds with other signals. |
| `$HOME` not set on exotic systems                        | `std::env::var("HOME")` returns `Err`, transcript validation skips silently. Search proceeds with other signals.                                      |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-04-04-security-hardening-requirements.md](docs/brainstorms/2026-04-04-security-hardening-requirements.md)
- FTS5 sanitisation learning:
  `docs/solutions/database-issues/fts5-query-sanitization-crashes-on-special-chars-2026-04-02.md`
- Session dedup lifecycle learning:
  `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`
- `fd-lock` crate: https://crates.io/crates/fd-lock
- RUSTSEC-2023-0035 (`fs2` unmaintained): https://rustsec.org/advisories/RUSTSEC-2023-0035
