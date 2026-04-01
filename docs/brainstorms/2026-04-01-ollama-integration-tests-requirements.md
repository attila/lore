---
date: 2026-04-01
topic: ollama-integration-tests
---

# Ollama Integration Test Suite

## Problem Frame

The existing end-to-end tests (`tests/e2e.rs`) use `FakeEmbedder`, which produces deterministic
pseudo-random vectors. This proves the database and ingest plumbing works, but says nothing about
whether real Ollama embeddings produce semantically useful search results. We also lack end-to-end
coverage of the MCP server wired to a real Ollama backend. We need tests that exercise the full
stack with a live Ollama instance to validate semantic quality, plumbing correctness, and server
integration.

## Requirements

**Semantic Search Quality**

- R1. Ingest seed patterns with real embeddings, then vector-search a semantically related query.
  The correct source file must appear in the top 3 results.
- R2. Hybrid search must return the semantically correct result for a query where the target file
  shares no exact keywords with the query (proving vector contribution). The test must first assert
  that `search_fts` returns no results for the query, then assert that `search_hybrid` finds the
  target — otherwise FTS could silently carry it.
- R3. Add a new pattern via `add_pattern` with real embeddings. A semantic search must find it in
  the top 3 results.
- R4. Update a pattern via `update_pattern`. A search for the new content must find it; a search for
  the old content must not return it.
- R5. Append to a pattern via `append_to_pattern`. A search for the appended content must find it in
  the top 3 results.

**Plumbing Correctness**

- R6. Re-ingest after file deletion must remove stale embeddings. Vector search for deleted content
  must return no results from the removed file.
- R7. MCP tool round-trip: send a `tools/call` JSON-RPC request for `search_patterns` through the
  server's stdin/stdout with a real Ollama backend and receive a valid result.

**Error Handling**

- R8. When Ollama is unreachable at embed time (query or ingest), the operation must return an error
  rather than panic or hang.

**Test Infrastructure**

- R9. All tests use `#[ignore = "requires running Ollama instance"]`, matching the existing pattern
  in `tests/init_output.rs`.
- R10. Tests run via the existing `just test-integration` recipe. No new recipes or CI changes
  needed.
- R11. Semantic assertions use "appears in top N" (N=3), not exact rank order, to tolerate model
  version variance.

## Success Criteria

- All R1-R8 tests pass locally with Ollama running the default model (`nomic-embed-text`)
- `just test` continues to pass without Ollama (ignored tests skipped)
- No flaky failures across 3 consecutive runs

## Scope Boundaries

- No CI Ollama setup — these remain local-only tests
- No new just recipes or feature flags
- No benchmarking or latency assertions
- No testing of alternative embedding models
- Coverage expansion deferred to later iteration

## Key Decisions

- **Top-N assertions over exact rank**: Embedding models may reorder results across versions.
  Asserting "in top 3" is robust enough to catch regressions without brittleness.
- **Same `#[ignore]` pattern**: No reason to introduce a second tier of test filtering. All
  Ollama-dependent tests share one mechanism.
- **Single test file**: All new tests go in one file (`tests/ollama_integration.rs`) to keep
  Ollama-dependent tests discoverable.

## Dependencies / Assumptions

- Ollama is running locally with `nomic-embed-text` pulled (same as `lore init` default)
- Tests create isolated temp directories — no shared state between tests

## Outstanding Questions

### Deferred to Planning

- [Affects R7][Technical] Subprocess vs in-process for MCP test? Subprocess (`lore serve` via
  stdin/stdout) is a true integration test but requires config file setup and child process
  management. In-process (`handle_request()` directly) is simpler but `ServerContext` is currently
  private — would need a visibility change or public test helper. Planning should pick one.

### Resolved

- [Affects R8] Use a dead localhost port (`http://127.0.0.1:1`) to simulate unreachable Ollama.
  Connection-refused returns instantly (no 30s timeout wait), doesn't disrupt a running Ollama
  instance, and works reliably across platforms.

## Next Steps

-> `/ce:plan` for structured implementation planning
