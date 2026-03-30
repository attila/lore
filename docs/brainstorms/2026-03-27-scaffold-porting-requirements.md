---
date: 2026-03-27
topic: scaffold-porting
---

# Scaffold Porting

## Problem Frame

Lore has a complete project skeleton (Phase 0) with quality gates, CI, and empty module stubs — but
no functionality. The ~1,800 lines of scaffold code in `tmp/scaffold/` implement the full feature
set (config, database, embeddings, ingestion, search, provisioning, MCP server, CLI) but have never
been compiled. The scaffold targets older dependency versions (`rusqlite 0.31`, `ureq 2`) and uses
patterns incompatible with the project's quality standards (clippy pedantic,
`unsafe_code = "deny"`).

This phase ports the scaffold into the project skeleton, adapting it to current dependencies, fixing
known issues, refactoring for clarity, and adding tests — producing a working `lore` binary that
passes all quality gates.

## Requirements

- R1. **Rename** — All references to `knowledge-mcp` become `lore`: binary name, config filename
  (`lore.toml`), database filename (`lore.db`), log prefixes, git commit message prefixes, MCP
  server info, default config path.
- R2. **rusqlite 0.39 + sqlite-vec** — Migrate from `load_extension` (scaffold) to
  `sqlite3_auto_extension` with `rusqlite::ffi`. Requires targeted `#[allow(unsafe_code)]` on the
  initialization function. Drop the `load_extension` feature from rusqlite.
- R3. **ureq 3 migration** — Adapt all HTTP calls to ureq 3 API (`.into_json()` →
  `.body_mut().read_json()` and similar changes).
- R4. **Embedder trait** — Extract an `Embedder` trait from `OllamaClient` so tests can use a mock
  implementation. Real SQLite, mock Ollama — as decided in Phase 0.
- R5. **Clippy pedantic compliance** — All ported code must pass
  `cargo clippy --all-targets --
  -D warnings` with the project's existing lint configuration.
- R6. **Split chunking from ingest** — Extract markdown chunking logic (heading-based chunking,
  frontmatter parsing, document chunking) into a dedicated module. `ingest` handles orchestration
  and write operations; chunking is pure text processing with no database or embedding dependencies.
- R7. **Improve provision portability** — Replace `which ollama` with a direct binary invocation
  check (e.g., `ollama --version`). The `which` command is not available on all Linux distributions.
- R8. **Clean up MCP server** — Refactor `server.rs` for clarity: reduce boilerplate in tool
  handlers, improve JSON-RPC response construction, but keep the hand-rolled implementation (no SDK
  dependency — per GENESIS design decision).
- R9. **Tests per layer** — Each porting layer includes tests that validate the ported code:
  - Layer 1 (core): config round-trip, database CRUD + search with mock embeddings, markdown
    chunking (heading splitting, frontmatter extraction, edge cases), RRF algorithm
  - Layer 2 (infrastructure): git operations (requires tempdir + git init), provision status
    checking
  - Layer 3 (interface): MCP request/response handling (tool list, search, add/update/append), CLI
    subcommand parsing
- R10. **Working binary** — At the end of all three layers, `lore init`, `lore ingest`,
  `lore search`, `lore status`, and `lore serve` work end-to-end (with Ollama running for the
  integration test, offline for unit tests).
- R11. **CI passes** — `just ci` passes after each layer: formatting, clippy, tests, cargo-deny, doc
  build.

## Porting Layers

### Layer 1: Core Data Path

config, database, embeddings, chunking (new module), ingest

This is the heart of lore — everything needed to read config, store/search data, talk to Ollama, and
process markdown into searchable chunks. The `Embedder` trait is introduced here.

### Layer 2: Infrastructure

git, provision

Supporting services: git commit automation and Ollama lifecycle management. Lower risk, fewer
dependency changes.

### Layer 3: Interface

server (MCP), CLI (main.rs)

The user-facing layer: MCP protocol handling and the clap CLI that wires everything together.
Depends on all prior layers.

## Success Criteria

- `cargo build --release` produces a working `lore` binary
- `just ci` passes with zero warnings
- Unit tests cover the core logic: chunking, search ranking, config parsing, database operations
- The `Embedder` trait allows all database and ingest tests to run without Ollama
- No `knowledge-mcp` references remain in source code

## Scope Boundaries

- **In:** Porting all 7 scaffold modules, renaming, dependency migration, Embedder trait, module
  reorganization (chunking split), tests, refactoring for quality
- **Out:** New features not in the scaffold, async/tokio migration, MCP SDK adoption, alternative
  embedding providers, cross-platform CI, release automation, README updates (separate task)
- **Out:** End-to-end integration testing with a live Ollama instance (deferred — unit tests with
  mock embeddings are sufficient for this phase)

## Key Decisions

- **Layered porting:** Three layers (core → infrastructure → interface) in dependency order. Each
  layer compiles and passes CI independently.
- **Test as you go:** Each layer ships with its own tests. No deferred test phase.
- **Refactor during port:** Improve design while porting rather than carrying scaffold debt. Split
  chunking, fix portability issues, clean up server code.
- **Hand-rolled MCP stays:** Per GENESIS design decision — no MCP SDK dependency.
- **Real SQLite, mock Ollama:** Per Phase 0 decision — SQLite is fast and bundled, Ollama is
  external and slow. The `Embedder` trait boundary keeps tests offline and fast.

## Dependencies / Assumptions

- Phase 0 infrastructure is complete and merged (CI, quality gates, skeleton)
- `tmp/scaffold/` is the reference implementation — it defines intended behavior
- Ollama is not required for building or running tests (only for live integration)
- The scaffold's feature set is complete — no new MCP tools or CLI commands in this phase

## Outstanding Questions

### Deferred to Planning

- [Affects R2][Needs research] Exact `sqlite3_auto_extension` API with `rusqlite 0.39` + current
  `sqlite-vec` crate — verify the FFI pattern and any transmute-related clippy allows needed
- [Affects R3][Needs research] Full inventory of ureq 2 → 3 API differences affecting the scaffold's
  HTTP calls (embed, pull, show, health check)
- [Affects R4][Technical] `Embedder` trait surface — what methods beyond `embed()` and
  `dimensions()` need to be on the trait vs. remaining on `OllamaClient` directly
- [Affects R6][Technical] Whether the chunking module should also own the write operations
  (`add_pattern`, `update_pattern`, `append_to_pattern`) or if those stay in ingest
- [Affects R8][Technical] Specific refactoring approach for server.rs — how much to restructure vs.
  clean up in place
- [Affects R9][Technical] Whether provision tests need to mock system commands or can test only the
  status-checking logic

## Next Steps

→ `/ce:plan` for structured implementation planning
