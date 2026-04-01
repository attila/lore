---
title: "feat: Add Ollama integration test suite"
type: feat
status: completed
date: 2026-04-01
origin: docs/brainstorms/2026-04-01-ollama-integration-tests-requirements.md
---

# feat: Add Ollama integration test suite

## Overview

Add integration tests that exercise ingest, search, and MCP tool operations with a real Ollama
instance, validating both semantic quality and plumbing correctness. All tests use the existing
`#[ignore]` / `just test-integration` pattern.

## Problem Frame

The existing `tests/e2e.rs` proves the database and ingest plumbing works with `FakeEmbedder`, but
says nothing about whether real Ollama embeddings produce semantically useful search results. We
also lack end-to-end coverage of the MCP server wired to a real Ollama backend. (see origin:
`docs/brainstorms/2026-04-01-ollama-integration-tests-requirements.md`)

## Requirements Trace

- R1. Ingest + vector search: correct source in top 3
- R2. Hybrid search with FTS-negative proof: `search_fts` returns nothing, `search_hybrid` finds
  target
- R3. `add_pattern` with real embeddings: searchable in top 3
- R4. `update_pattern`: new content found, old content gone
- R5. `append_to_pattern`: appended content in top 3
- R6. Re-ingest after deletion: stale embeddings removed
- R7. MCP subprocess round-trip: JSON-RPC `search_patterns` through `lore serve` stdin/stdout
- R8. Ollama unreachable: error, not panic or hang
- R9. All tests `#[ignore = "requires running Ollama instance"]`
- R10. Runs via existing `just test-integration`
- R11. Top-N (N=3) assertions, not exact rank

## Scope Boundaries

- No CI Ollama setup — local-only tests
- No new just recipes or feature flags
- No benchmarking or latency assertions
- No alternative embedding models
- No shared test helper module (follow existing duplication convention)

## Context & Research

### Relevant Code and Patterns

- `tests/e2e.rs` — lifecycle test structure: `seed_patterns()`, `git_init()`, `open_db()`, then
  ingest → search → write → search. New tests mirror this but swap `FakeEmbedder` for `OllamaClient`
- `tests/init_output.rs` — `#[ignore = "requires running Ollama instance"]` pattern
- `tests/smoke.rs` — binary invocation via `assert_cmd::Command::cargo_bin("lore")`
- `src/server.rs` tests — `TestHarness` with `handle_request()` (private, in-module only)
- `src/embeddings.rs:52` — `OllamaClient::new(host, model)`, 30s timeout via ureq

### Institutional Learnings

- sqlite-vec registration is process-global via `Once` guard — no test parallelism concerns.
  `KnowledgeDB::open()` handles it. (from
  `docs/solutions/build-errors/sqlite-vec-no-rust-export-register-via-ffi.md`)

## Key Technical Decisions

- **R7: Subprocess over in-process**: `ServerContext` and `handle_request` are private to
  `server.rs`. Making them public would leak test concerns into the API. The subprocess approach
  (`lore serve` with piped stdin/stdout) is a true integration test, matches the `assert_cmd`
  pattern in `smoke.rs` and `init_output.rs`, and requires no visibility changes. Use
  `std::process::Command` with `Stdio::piped()` since the MCP server is interactive (not
  fire-and-forget like the init tests).

- **R8: Dead localhost port**: Use `OllamaClient::new("http://127.0.0.1:1", "nomic-embed-text")`.
  Connection-refused returns instantly. No 30s timeout wait, no service disruption. (see origin:
  Resolved section)

- **One large lifecycle test + focused satellite tests**: R1-R5 share a common ingest step with real
  Ollama (slow). A single `ollama_lifecycle` test covers the full ingest → search → add → update →
  append flow, avoiding redundant Ollama calls. R6, R7, R8 are separate focused tests with
  independent setup.

- **Seed data designed for R2**: The seed patterns must include at least one file where a semantic
  query has zero FTS token overlap with the content, so the two-phase assertion (FTS returns
  nothing, hybrid finds it) is meaningful. The test should use vocabulary that is semantically
  related but lexically disjoint.

## Open Questions

### Resolved During Planning

- **Subprocess vs in-process for R7**: Subprocess. `ServerContext` is private, and changing
  visibility just for tests is not justified. The subprocess approach also tests the real binary
  path.
- **How to simulate unreachable Ollama for R8**: Dead port `127.0.0.1:1`. Instant RST, no service
  disruption.
- **Test granularity**: One lifecycle test (R1-R5) + three focused tests (R6, R7, R8) = 4 test
  functions. Balances Ollama call overhead against test isolation.

### Deferred to Implementation

- **Exact seed data content**: The implementer should design seed patterns where at least one pair
  has semantic similarity without keyword overlap (for R2). The exact text depends on what
  `nomic-embed-text` handles well — may need iteration.
- **MCP subprocess timing**: The `lore serve` process enters a stdin read loop. The test must write
  the request and read the response before the process blocks. If timing is tricky, a short read
  timeout on stdout may be needed.

## Implementation Units

- [ ] **Unit 1: Test file scaffold and helpers**

  **Goal:** Create `tests/ollama_integration.rs` with helper functions and the `#[ignore]` pattern
  established.

  **Requirements:** R9, R10

  **Dependencies:** None

  **Files:**
  - Create: `tests/ollama_integration.rs`

  **Approach:**
  - Define helpers following `tests/e2e.rs` conventions: `git_init()`, `open_db()`,
    `ollama_client()` (returns `OllamaClient::new("http://127.0.0.1:11434", "nomic-embed-text")`)
  - Define `seed_patterns()` with content designed for both FTS and semantic search testing. At
    least 3 files with distinctive domains. One file must be semantically related to a test query
    but share no FTS tokens with it (for R2). Consider topics like: error handling (explicit terms),
    naming conventions, testing strategy — but with one file using vocabulary that allows a
    lexically-disjoint semantic query
  - All helpers are private to this test file (no shared helper module — matches existing
    convention)

  **Patterns to follow:**
  - `tests/e2e.rs` — `git_init()`, `open_db()`, `seed_patterns()` structure
  - `tests/init_output.rs` — `#[ignore = "requires running Ollama instance"]` annotation

  **Test expectation:** None — this unit is scaffolding.

  **Verification:**
  - File compiles with `cargo test --features test-support --no-run`
  - `just test` still passes (ignored tests skipped)

- [ ] **Unit 2: Semantic lifecycle test (R1-R5)**

  **Goal:** Single test function `ollama_lifecycle` covering ingest, vector search, hybrid search
  with FTS-negative proof, add_pattern, update_pattern, and append_to_pattern with real Ollama
  embeddings.

  **Requirements:** R1, R2, R3, R4, R5, R11

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `tests/ollama_integration.rs`

  **Approach:**
  - Create tempdir, seed patterns, git init, open DB with `ollama.dimensions()`
  - Ingest with real `OllamaClient` as `&dyn Embedder`
  - R1: Call `embedder.embed(query)` to get the query vector, then `db.search_vector(&embedding, 3)`
    — assert correct file in top 3
  - R2: Two-phase assertion — first `search_fts(query)` returns empty, then call
    `embedder.embed(query)` and pass the result to `search_hybrid(query, Some(&embedding), 3)`.
    Assert the target appears in results. Critical: passing `None` instead of an embedding silently
    falls back to FTS-only, which would make R2 vacuously pass. The query must be lexically disjoint
    from the target file's content
  - R3: `add_pattern` a new pattern, then vector search confirms it in top 3
  - R4: `update_pattern` with new content, search for new content finds it, search for old
    distinctive term does not return the file
  - R5: `append_to_pattern` with new section, search for appended content finds it in top 3
  - All "top 3" checks use `results.iter().take(3).any(|r| r.source_file == expected)`

  **Patterns to follow:**
  - `tests/e2e.rs::full_lifecycle` — same flow structure, swapping FakeEmbedder for OllamaClient

  **Test scenarios:**
  - Happy path: Ingest 3 seed files → `search_vector("related semantic query")` returns correct file
    in top 3 results (R1)
  - Happy path: `search_fts("lexically disjoint query")` returns empty, then `search_hybrid` with
    same query + embedding returns correct file in top 3 (R2)
  - Happy path: `add_pattern` new content → `search_vector` finds it in top 3 (R3)
  - Happy path: `update_pattern` replaces content → search for new term finds file, search for old
    distinctive term does not return it (R4)
  - Happy path: `append_to_pattern` adds section → search for appended content finds file in top 3
    (R5)

  **Verification:**
  - `just test-integration` passes with Ollama running
  - Each R1-R5 assertion exercises real embeddings, not FakeEmbedder

- [ ] **Unit 3: Stale embedding removal test (R6)**

  **Goal:** Verify that re-ingest after file deletion removes stale vector embeddings.

  **Requirements:** R6

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `tests/ollama_integration.rs`

  **Approach:**
  - Seed 2 files, ingest with real embeddings
  - Delete one file, re-ingest
  - Vector search for deleted file's distinctive content must return no results from that file

  **Patterns to follow:**
  - `tests/e2e.rs::ingest_replaces_stale_data` — same structure, swap FakeEmbedder for OllamaClient

  **Test scenarios:**
  - Happy path: Ingest 2 files → delete file A → re-ingest → `search_vector` for file A's
    distinctive content returns no results with `source_file == "a.md"`
  - Happy path: After re-ingest, `search_vector` for file B's content still returns file B

  **Verification:**
  - `just test-integration` passes
  - Stale vector data is confirmed gone (not just FTS data)

- [ ] **Unit 4: MCP subprocess round-trip (R7)**

  **Goal:** Send a `tools/call` JSON-RPC request to a real `lore serve` process and receive a valid
  search result backed by real Ollama embeddings.

  **Requirements:** R7

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `tests/ollama_integration.rs`

  **Approach:**
  - Set up: tempdir, seed patterns, git init. Use `Config::default_with()` + `config.save()` to
    write a valid TOML config pointing at the tempdir and a DB path within it. Ingest data to
    pre-populate the DB. Drop the DB handle before spawning the subprocess (WAL mode allows
    concurrent access, but dropping avoids any lock contention)
  - Locate the binary with `assert_cmd::cargo::cargo_bin("lore")` (returns `PathBuf`), then spawn
    with `std::process::Command::new(bin)` — not `assert_cmd::Command`, which is fire-and-forget and
    doesn't support interactive piped I/O
  - Spawn with `.args(["serve", "--config", &config_path])`, `stdin(Stdio::piped())`,
    `stdout(Stdio::piped())`, `stderr(Stdio::null())`
  - Write `initialize` request + newline to stdin, read one response line from stdout
  - Important: `notifications/initialized` returns no response (server returns `None`). Either skip
    sending it, or send it but do not attempt to read a response line — otherwise the test deadlocks
  - Write `tools/call` request for `search_patterns` with a query, read one response line
  - Parse response as JSON, assert it contains `result.content[0].text` with a non-empty search
    result
  - Kill child process (use a drop guard to ensure cleanup on panic)

  **Patterns to follow:**
  - `tests/smoke.rs` — `assert_cmd::cargo::cargo_bin("lore")` for binary path
  - `src/server.rs` tests — JSON-RPC request format and expected response structure

  **Test scenarios:**
  - Happy path: Spawn `lore serve` → send `initialize` → send `tools/call search_patterns` with
    query → response contains valid result with non-empty content text
  - Edge case: Response JSON has correct `jsonrpc: "2.0"` envelope and matching `id`

  **Verification:**
  - `just test-integration` passes
  - The test exercises the real binary, real config, real Ollama, real DB — full stack

- [ ] **Unit 5: Ollama unreachable error handling (R8)**

  **Goal:** Verify that operations return errors (not panics or hangs) when Ollama is unreachable.

  **Requirements:** R8

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `tests/ollama_integration.rs`

  **Approach:**
  - Create `OllamaClient::new("http://127.0.0.1:1", "nomic-embed-text")` — dead port, instant
    connection-refused
  - Test 1: Call `embedder.embed("anything")` directly, assert it returns `Err`
  - Test 2: Call `ingest::ingest()` with the dead-port client, assert `errors` is non-empty (ingest
    catches embed failures per-chunk and records them). Note: chunks are still inserted with `None`
    embeddings — `chunks_created` will be > 0. The assertion should check that `errors` is
    non-empty, not that `chunks_created` is zero

  **Patterns to follow:**
  - `src/embeddings.rs` — `embed()` uses `?` on ureq call, propagates as `anyhow::Error`
  - `src/ingest.rs:118-129` — `ingest()` catches embed errors per-chunk, stores `None` embedding,
    still calls `insert_chunk` which increments `chunks_created`

  **Test scenarios:**
  - Error path: `embed("text")` with dead-port client returns `Err`, not panic
  - Error path: `ingest()` with dead-port client returns `IngestResult` with non-empty `errors` vec
    (chunks are created but lack embeddings)

  **Verification:**
  - `just test-integration` passes
  - Test completes in under 2 seconds (instant RST, no 30s timeout)

## System-Wide Impact

- **Interaction graph:** No production code changes. The new test file only consumes existing public
  APIs (`OllamaClient`, `KnowledgeDB`, `ingest::*`, `database::search_*`).
- **Error propagation:** R8 tests confirm that ureq connection errors propagate correctly through
  `embed()` → `ingest()`. No new error paths introduced.
- **API surface parity:** No changes to public API.
- **Unchanged invariants:** `just test` and `just ci` behavior unchanged — ignored tests are
  skipped. The `test-support` feature flag is already used by `just test-integration`.

## Risks & Dependencies

| Risk                                                        | Mitigation                                                                                                                              |
| ----------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| R2 seed data doesn't achieve true FTS-zero overlap          | Design seed content carefully; verify with `search_fts` assertion in the test itself. If flaky, adjust vocabulary                       |
| MCP subprocess stdout read blocks if server doesn't respond | Use `BufReader` on child stdout with a reasonable approach (e.g., read one line per request). Kill child in a drop guard or defer block |
| Ollama model version changes affect ranking                 | Top-3 assertions (R11) provide headroom. If flaky across versions, widen to top-5                                                       |
| Tests slow due to real Ollama calls                         | Acceptable — these are `#[ignore]` local-only tests. The lifecycle test batches operations to minimize round-trips                      |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-04-01-ollama-integration-tests-requirements.md](docs/brainstorms/2026-04-01-ollama-integration-tests-requirements.md)
- Related code: `tests/e2e.rs` (lifecycle pattern), `tests/init_output.rs` (`#[ignore]` pattern),
  `src/server.rs` (MCP request format)
- Related learnings: `docs/solutions/build-errors/sqlite-vec-no-rust-export-register-via-ffi.md`
  (FFI registration is process-global)
