---
title: "feat: Port scaffold into project skeleton"
type: feat
status: completed
date: 2026-03-27
origin: docs/brainstorms/2026-03-27-scaffold-porting-requirements.md
deepened: 2026-03-27
---

# feat: Port scaffold into project skeleton

## Overview

Port the ~1,840-line scaffold from `tmp/scaffold/` into the project skeleton's empty module stubs.
Adapt to current dependency versions (rusqlite 0.39, ureq 3), introduce an `Embedder` trait for
testability, extract a dedicated chunking module, and refactor for clippy pedantic compliance. Work
is organized in two layers (core → interface), each compiling and passing CI independently.

## Problem Frame

Lore has a working project skeleton with quality gates but no functionality. The scaffold in
`tmp/scaffold/` implements the full feature set but targets older dependencies, has never been
compiled, and doesn't meet the project's quality standards. This plan bridges that gap. (see origin:
`docs/brainstorms/2026-03-27-scaffold-porting-requirements.md`)

## Requirements Trace

- R1. Rename all `knowledge-mcp` references to `lore`
- R2. Migrate sqlite-vec from `load_extension` to `sqlite3_auto_extension` with targeted
  `#[allow(unsafe_code)]`
- R3. Migrate ureq 2 → 3 API
- R4. Introduce `Embedder` trait for testability (real SQLite, mock Ollama)
- R5. Clippy pedantic compliance across all ported code
- R6. Extract chunking logic from ingest into dedicated module
- R7. Replace `which` with direct binary invocation in provision
- R8. Refactor MCP server for clarity while keeping hand-rolled implementation
- R9. Tests per layer — config, database, chunking, ingest, git, provision, server, CLI
- R10. Working binary with all 5 subcommands (init, ingest, serve, search, status)
- R11. `just ci` passes after each layer

**Deviation from origin:** The requirements document places git in Layer 2 (Infrastructure). This
plan moves git to Layer 1 because ingest depends on it for write operations — Layer 1 cannot compile
without it. The requirements document's three-layer structure (core / infrastructure / interface) is
collapsed to two layers: Layer 1 (core + git, 6 units) and Layer 2 (provision + server + CLI, 3
units). This is a dependency correction, not a scope change.

## Scope Boundaries

- **In:** All 7 scaffold modules, dependency migrations, Embedder trait, chunking module, tests,
  refactoring
- **Out:** New features, async migration, MCP SDK, alternative embedding providers, cross-platform
  CI, README updates, live Ollama integration tests
- **Out:** Changing the MCP protocol implementation approach (hand-rolled per GENESIS decision)
- **Out:** Incremental ingestion — the scaffold's `ingest()` wipes and rebuilds the database on
  every call. This is known technical debt preserved intentionally; changing it would be a new
  feature.

## Context & Research

### Relevant Code and Patterns

- `src/lib.rs` — module declarations; needs `pub mod chunking;` added
- `src/main.rs` — minimal clap Cli struct; needs full subcommand wiring. The project uses a lib+bin
  split (not the scaffold's binary-only `mod` declarations). `main.rs` imports via
  `use lore::{...}`.
- `tests/smoke.rs` — existing test pattern: `assert_cmd` + `predicates`
- `Cargo.toml` — resolved: rusqlite 0.39.0, sqlite-vec 0.1.7, ureq 3.3.0
- `Cargo.lock` — sqlite-vec 0.1.7 depends only on `cc`, not on rusqlite; FFI bridge is app-level
- `dprint.json` — rustfmt via exec plugin, edition 2024, line width 100
- `deny.toml` — license allowlist; may need additions for new transitive deps
- Phase 0 plan — testing strategy: inline `#[cfg(test)] mod tests`, hand-written fakes
  (`FakeEmbedder`) over mockall, insta for snapshots, `tests/fixtures/` for committed test data

### Institutional Learnings

- `docs/solutions/build-errors/rust-toolchain-action-does-not-read-toml.md` — CI uses
  `actions-rust-lang/setup-rust-toolchain@v1` with `rustflags: ""`. Already configured correctly; no
  CI changes needed for the port.

## Key Technical Decisions

- **sqlite-vec registration via `std::sync::Once`**: `sqlite3_auto_extension` is process-global — it
  must be called once before any `Connection::open`. Wrap in a `register_sqlite_vec()` function in
  `database.rs` with `#[allow(unsafe_code)]` and a SAFETY comment. `KnowledgeDB::open` calls this
  internally via `Once`, making it transparent to callers. This replaces the scaffold's
  per-connection `load_extension` pattern entirely. Note: `Once` is permanent within a process, so
  the "not registered" error path is untestable in the standard test harness — this is fine since
  registration always runs. **Important:** sqlite-vec 0.1.7 is a build-only crate that compiles C
  code and links a static library (`libsqlite_vec0.a`). It does not provide a Rust-level export of
  `sqlite3_vec_init`. The implementer must declare the symbol via `extern "C"` with the correct
  `rusqlite::ffi` types. Write a minimal verification test early in Unit 4 (open in-memory DB,
  create a vec0 table) to validate the FFI pattern before porting the full module.

- **`Chunk` struct lives in `chunking` module**: `Chunk` is the output of text processing, not a
  database concept. `database.rs` imports it from `chunking`. `SearchResult` keeps flat fields
  matching the scaffold (not composed from `Chunk`) — the composition refactor would add
  cross-module ripple during the port without being required by any requirement. Defer to a
  post-port cleanup if field drift becomes a real problem. `DBStats` stays in `database.rs`.
  Dependency flow: chunking (leaf) ← database ← ingest.

- **Embedder trait is minimal**: Only `embed(&self, input: &str) -> Result<Vec<f32>>` and
  `dimensions(&self) -> usize`. Health checks, model pull, and model name stay on `OllamaClient`
  directly — they're provisioning concerns, not embedding concerns. Database takes
  `dimensions: usize` as a parameter (not `&dyn Embedder`), keeping database independent of the
  embeddings module.

- **`FakeEmbedder` placement**: Define as a `pub(crate)` struct at module scope in
  `src/embeddings.rs` behind `#[cfg(test)]`, outside the `mod tests` block. This makes it importable
  by all other modules' test code via `crate::embeddings::FakeEmbedder` without affecting production
  builds. Placing it inside `#[cfg(test)] mod tests` would make it invisible to other modules.

- **ureq 3 with stored `Agent` and timeout strategy**: `OllamaClient` stores a `ureq::Agent`
  configured with a moderate global timeout (e.g., 30 seconds) for general API calls (embed, show,
  health). The `pull_model` method creates a separate short-lived agent with no timeout since model
  downloads can take minutes. This avoids the conflict between a 3-second health check timeout and
  minute-long streaming downloads.

- **Server context struct**: The server introduces a context struct holding `&KnowledgeDB`,
  `&dyn Embedder`, and `&Config` with a single lifetime parameter. The MCP server loop is
  synchronous and single-threaded — no `Arc`/`Mutex` needed. This replaces the scaffold's
  4-parameter threading through every handler function.

- **Server refactoring approach**: Clean up in place rather than restructure. Extract shared
  response construction into helpers, use the context struct for parameter reduction, validate
  `jsonrpc == "2.0"` (the scaffold deserializes but ignores it — adding validation is a minor
  spec-compliance addition, not a feature), but keep the overall JSON-RPC loop and dispatch
  structure.

- **Provision testing**: Test `check_status` logic with a struct that captures the state machine
  result. Do not mock system commands (`ollama`, `systemctl`, `brew`) — the cost/value ratio is
  poor. The `provision` function itself is tested manually during integration.

- **`&PathBuf` → `&Path`**: Clippy pedantic flags `&PathBuf` parameters. All function signatures in
  the scaffold that take `&PathBuf` change to `&Path`. This is a mechanical fix applied across all
  modules.

- **No `FakeGit` in this phase**: Ingest write-operation tests that involve git will use real git
  repos in tempdirs (via `git init`). A `FakeGit` trait boundary is a future improvement when
  testing complexity warrants it.

## Open Questions

### Resolved During Planning

- **sqlite3_auto_extension pattern**: sqlite-vec 0.1.7 is a build-only crate that compiles C code
  and links `libsqlite_vec0.a`. The `sqlite3_vec_init` symbol is not a Rust export — it must be
  declared via `extern "C"` with `rusqlite::ffi` types. Registration pattern:
  `sqlite3_auto_extension(Some(transmute(sqlite3_vec_init as *const ())))`. Wrapped in a function
  with `#[allow(unsafe_code)]` behind `std::sync::Once`. Validate early with a minimal test.

- **ureq 3 API changes**: Three confirmed migrations plus one deferred:
  - `.send_json(&req)?.into_json()?` → `.send_json(&req)?.body_mut().read_json()?`
  - Per-request `.timeout()` → `Agent` with `timeout_global` config
  - `ureq::get()`/`ureq::post()` → `agent.get()`/`agent.post()` (stored agent)
  - Streaming response (`pull_model`): the scaffold's `resp.into_reader()` does not exist in ureq 3.
    The exact replacement API (`into_body()`, `into_parts()`, or `body_mut()`) should be resolved
    during implementation since this code path is only tested manually. Defer to implementation
    discovery.

- **Embedder trait surface**: `embed()` + `dimensions()` on the trait. Everything else stays on
  `OllamaClient`. Database takes `dimensions: usize`, not a trait object.

- **Chunking module ownership**: Pure text processing only (chunking, frontmatter, heading parsing).
  Write operations (`add_pattern`, `update_pattern`, `append_to_pattern`) stay in `ingest` because
  they need database, embeddings, and git access.

- **Transmute clippy allows**: `clippy::missing_transmute_source`, `clippy::transmute_ptr_to_ptr`,
  and possibly `clippy::crosspointer_transmute` may fire on the sqlite-vec FFI call. Add targeted
  allows on the `register_sqlite_vec` function alongside `unsafe_code`.

- **Hardcoded 768 dimensions**: The scaffold hardcodes `768` in `server.rs` and `cmd_status` when
  opening the database, ignoring the embedder's `dimensions()`. This is a latent bug — if a user
  configures `all-minilm` (384 dimensions) or `mxbai-embed-large` (1024), the database would open
  with the wrong vector table size. All `KnowledgeDB::open` calls must derive dimensions from the
  embedder, never hardcode.

- **`index_single_file` ignores chunking strategy**: The scaffold's `index_single_file` always calls
  `chunk_by_heading` regardless of the configured strategy. The main `ingest()` function correctly
  checks the strategy parameter. Fix this by threading the strategy parameter through
  `index_single_file` and all write operations that call it.

### Deferred to Implementation

- **Exact clippy pedantic fixes**: The scaffold will trigger various pedantic lints beyond
  `&PathBuf`. These are mechanical and discoverable only by compiling the ported code.
- **cargo-deny license additions**: New transitive dependencies may introduce licenses not in the
  current allowlist. Discoverable only after porting and running `cargo deny check`.
- **MCP server snapshot format**: Use `insta::assert_json_snapshot!` for JSON-RPC responses (not
  `assert_snapshot!`) since it normalizes formatting. Exact snapshot content depends on final
  response shape.
- **`max_tokens` config field**: The scaffold's `ChunkingConfig` has a `max_tokens` field but
  chunking code never uses it. Preserve with a `// TODO: implement token-based chunk size limiting`
  comment. Whether to keep or remove is deferred pending a future brainstorm on chunking strategy.
- **`bind` config field**: The scaffold includes a `bind` field for a TCP address, but the MCP
  server currently uses stdio transport. Preserve with a
  `// TODO: evaluate TCP transport as alternative to
  stdio` comment. Whether lore remains
  stdio-only is an open product decision deferred to a future brainstorm.

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

### Module dependency graph

```
chunking             config            embeddings           git
  (pure text)          (TOML)           (Embedder trait      (subprocess)
       │                 │               + OllamaClient)
       ▼                 │                    │                │
   database              │                    │                │
  (SQLite+FTS5           │               ┌────┘                │
   +sqlite-vec)          │               │                     │
       │                 │               ▼                     │
       │                 │            ingest ◄─────────────────┘
       │                 │           (orchestration
       │                 │            + write ops)
       │                 │               │
       ▼                 ▼               ▼
   provision          server ◄───── ingest, database
  (Ollama            (MCP stdio)
   lifecycle)            │
       │                 │
       ▼                 ▼
         main (CLI wiring)
```

Key: arrows point from dependent to dependency. Database depends on chunking (for `Chunk` type) but
not on config — it takes `path` and `dimensions` as parameters. Config flows through main, server,
and ingest. `provision` and `server` do not depend on each other — both feed into `main`.

### Embedder trait boundary

```
trait Embedder {
    fn embed(&self, input: &str) -> Result<Vec<f32>>
    fn dimensions(&self) -> usize
}

OllamaClient : Embedder
    + is_healthy() -> bool
    + has_model() -> bool
    + pull_model(on_progress) -> Result<()>
    + model_name() -> &str
    stores ureq::Agent with 30s global timeout
    pull_model uses separate no-timeout agent

FakeEmbedder : Embedder     (pub(crate), #[cfg(test)])
    returns deterministic vectors (hash-based)
    default dimensions: 768 (matches nomic-embed-text)
    placed at module scope in embeddings.rs
```

### sqlite-vec registration flow

```
KnowledgeDB::open(path, dimensions)
  └─► register_sqlite_vec()         [#[allow(unsafe_code)]]
        └─► Once::call_once
              └─► sqlite3_auto_extension(sqlite3_vec_init)
                    (process-global, all future connections get sqlite-vec)
  └─► Connection::open(path)         (sqlite-vec already loaded)
  └─► init() creates FTS5 + vec0 tables
```

## Implementation Units

### Layer 1: Core Data Path

- [ ] **Unit 1: Config module**

  **Goal:** Port configuration loading, saving, and defaults with `lore` naming.

  **Requirements:** R1, R5

  **Dependencies:** None (leaf module)

  **Files:**
  - Modify: `src/config.rs`
  - Test: `src/config.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Port `Config`, `OllamaConfig`, `SearchConfig`, `ChunkingConfig` structs
  - Add `PartialEq` derive to all config structs (needed for round-trip test assertions)
  - Rename default config path from `knowledge-mcp.toml` to `lore.toml`
  - Rename default database from `knowledge.db` to `lore.db`
  - Rename error message: `"Run 'knowledge-mcp init' first."` → `"Run 'lore init' first."`
  - Change all `&PathBuf` parameters to `&Path`
  - `default_config_path()` returns `PathBuf::from("lore.toml")`

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/config.rs` as reference
  - Existing `Cargo.toml` serde/toml versions

  **Test scenarios:**
  - Config round-trip: create with defaults, save to tempfile, load back, assert equality
  - Default values are sensible (`localhost:3100`, `nomic-embed-text`, hybrid=true, top_k=5)
  - Load from nonexistent path returns a descriptive error mentioning `lore init`
  - Default config path is `lore.toml`

  **Verification:**
  - `cargo clippy --all-targets -- -D warnings` passes
  - `cargo test` passes with config round-trip test
  - No `knowledge-mcp` in any string literal

- [ ] **Unit 2: Chunking module**

  **Goal:** Extract markdown chunking from the scaffold's ingest.rs into a dedicated,
  dependency-free module.

  **Requirements:** R6, R5, R9

  **Dependencies:** None (leaf module)

  **Files:**
  - Create: `src/chunking.rs`
  - Modify: `src/lib.rs` (add `pub mod chunking;`)
  - Test: `src/chunking.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Move from scaffold's `ingest.rs`: `Chunk` struct, `chunk_by_heading`, `chunk_as_document`,
    `parse_heading`, `extract_frontmatter_tags`, `extract_frontmatter`, `extract_title`,
    `strip_frontmatter`, `file_stem`
  - `Chunk` struct is the primary public type — other modules import it
  - All functions are pure (no I/O, no database, no embeddings)
  - Public API: `Chunk`, `chunk_by_heading`, `chunk_as_document` (used by ingest)
  - Internal: parsing helpers

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/ingest.rs` lines 97-276 (chunking functions)

  **Test scenarios:**
  - Heading-based chunking: file with multiple headings produces correct chunks with heading paths
  - Nested headings: `## A` then `### B` then `## C` — heading stack pops correctly
  - Frontmatter extraction: YAML frontmatter with inline tags `[a, b, c]`
  - Frontmatter extraction: block-style tags (`- a`, `- b`)
  - No frontmatter: returns empty tags string
  - Empty file: returns no chunks (body < 10 chars threshold)
  - File with no headings: falls back to document-mode chunking
  - Title extraction from first `# Heading`
  - Strip frontmatter returns content after closing `---`
  - Chunk IDs follow `source_file:heading_path` pattern

  **Verification:**
  - Module compiles with no warnings
  - All chunking tests pass
  - No I/O dependencies in the module

- [ ] **Unit 3: Embedder trait and OllamaClient**

  **Goal:** Define the `Embedder` trait, port `OllamaClient` with ureq 3 migration, and create
  `FakeEmbedder` for downstream test use.

  **Requirements:** R3, R4, R5

  **Dependencies:** None (leaf module; depends only on external crates)

  **Files:**
  - Modify: `src/embeddings.rs`
  - Test: `src/embeddings.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Define `Embedder` trait with `embed()` and `dimensions()`
  - Port `OllamaClient` implementing `Embedder`, plus health/model methods
  - Store a `ureq::Agent` on `OllamaClient` with 30-second global timeout for general API calls
  - `pull_model` creates a separate short-lived agent with no timeout for streaming downloads
  - Apply ureq 2 → 3 API changes:
    - `.send_json(&req)?.into_json()?` → `.send_json(&req)?.body_mut().read_json()?`
    - Per-request timeout → agent-level `timeout_global`
    - Streaming response in `pull_model`: exact ureq 3 replacement API to be resolved during
      implementation (deferred — this code path is only tested manually)
  - Define `FakeEmbedder` as `pub(crate)` at module scope behind `#[cfg(test)]` (outside
    `mod tests`). Returns deterministic vectors (hash-based) with default dimension 768 (matching
    `nomic-embed-text`). Using real dimensions costs nothing measurable for in-memory SQLite test
    volumes and avoids divergence from production vector separation behavior. This enables all
    downstream modules' tests to import it via `crate::embeddings::FakeEmbedder`.

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/embeddings.rs`
  - ureq 3 API (researched: agent config builder, `body_mut().read_json()`, `into_parts()`)

  **Test scenarios:**
  - `FakeEmbedder` returns vectors of correct length matching `dimensions()`
  - `FakeEmbedder` returns consistent vectors for the same input
  - `OllamaClient` construction stores agent with timeout config
  - `dimensions()` returns correct values for known models (768 for nomic-embed-text, etc.)

  **Verification:**
  - Trait compiles and is object-safe (`&dyn Embedder` works)
  - `FakeEmbedder` is importable from other modules' `#[cfg(test)]` code
  - No ureq 2 API remnants

- [ ] **Unit 4: Database module**

  **Goal:** Port SQLite + FTS5 + sqlite-vec database layer with the new registration pattern.

  **Requirements:** R2, R5, R9

  **Dependencies:** Unit 2 (Chunk struct from chunking module)

  **Files:**
  - Modify: `src/database.rs`
  - Test: `src/database.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Implement `register_sqlite_vec()` with `#[allow(unsafe_code)]` and `std::sync::Once`
  - `KnowledgeDB::open` calls `register_sqlite_vec()` then `Connection::open`
  - Remove all `load_extension` code from scaffold
  - Import `Chunk` from `crate::chunking`
  - Keep `SearchResult` with flat fields matching the scaffold (composition refactor deferred to
    post-port cleanup)
  - Keep `DBStats` in this module
  - **Early FFI verification:** Before porting the full module, write a minimal test that declares
    `sqlite3_vec_init` via `extern "C"`, calls `register_sqlite_vec()`, opens an in-memory database,
    and creates a vec0 virtual table. This validates the FFI pattern early.
  - Port all methods: `init`, `clear_all`, `delete_by_source`, `insert_chunk`, `search_fts`,
    `search_vector`, `search_hybrid`, `stats`
  - Port `vec_to_blob` as a private helper
  - Refactor `reciprocal_rank_fusion` to use `f64::total_cmp()` instead of `partial_cmp().unwrap()`
    — avoids potential panics on NaN and is more idiomatic
  - Change all `&PathBuf` to `&Path`

  **Patterns to follow:**
  - sqlite-vec 0.1.7 docs: `sqlite3_auto_extension(Some(transmute(sqlite3_vec_init as *const ())))`
  - Scaffold `tmp/scaffold/database.rs`

  **Test scenarios:**
  - Open in-memory database, init tables, verify no errors
  - Insert a chunk with embedding, retrieve via FTS5 search
  - Insert a chunk with embedding, retrieve via vector search
  - Hybrid search with both FTS5 and vector results, verify RRF ranking
  - Hybrid search with `None` embedding falls back to FTS-only
  - `clear_all` removes all data
  - `delete_by_source` removes only chunks for that file
  - `stats` returns correct chunk and source counts
  - RRF unit test: two ranked lists → merged output with correct ordering (using `total_cmp`)
  - `vec_to_blob` round-trip: f32 slice → blob → verify byte layout

  **Verification:**
  - Database tests pass with `FakeEmbedder`-generated vectors (no Ollama needed)
  - sqlite-vec extension loads successfully (vec0 table creation works)
  - `#[allow(unsafe_code)]` is scoped to `register_sqlite_vec()` only

- [ ] **Unit 5: Git module**

  **Goal:** Port git operations for committing pattern changes.

  **Requirements:** R5, R9

  **Dependencies:** None (leaf module, uses subprocess)

  **Files:**
  - Modify: `src/git.rs`
  - Test: `src/git.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Port `add_and_commit` and `is_git_repo` from scaffold
  - Change `&PathBuf` to `&Path`
  - The module is small (~40 lines) and straightforward

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/git.rs`

  **Test scenarios:**
  - `is_git_repo` returns true for a tempdir with `git init`
  - `is_git_repo` returns false for a plain tempdir
  - `add_and_commit` in a git-initialized tempdir creates a commit (verify with `git log`)
  - `add_and_commit` with nonexistent file returns error

  **Verification:**
  - Tests pass with real git operations in tempdirs
  - No unsafe code, no external dependencies beyond `std::process::Command`

  Note: Git is placed in Layer 1 (not Layer 2) because `ingest` (Unit 6) depends on it for write
  operations. Without git compiled, Layer 1 would not pass CI.

- [ ] **Unit 6: Ingest module**

  **Goal:** Port ingestion orchestration and write operations, wired to chunking, database, Embedder
  trait, and git.

  **Requirements:** R5, R9

  **Dependencies:** Unit 2 (chunking), Unit 3 (Embedder), Unit 4 (database), Unit 5 (git)

  **Files:**
  - Modify: `src/ingest.rs`
  - Test: `src/ingest.rs` (inline `#[cfg(test)] mod tests`)
  - Create: `tests/fixtures/` (committed test markdown files for ingestion tests)

  **Approach:**
  - `ingest()` function takes `&dyn Embedder` instead of `&OllamaClient`
  - Write operations (`add_pattern`, `update_pattern`, `append_to_pattern`) also take
    `&dyn Embedder`
  - Import chunking functions from `crate::chunking`
  - Keep `IngestResult`, `WriteResult`, `slugify`, `index_single_file`, `try_commit` here
  - Fix `index_single_file` to accept the chunking strategy parameter instead of hardcoding
    `chunk_by_heading` — thread the strategy through all write operations that call it
  - Rename git commit message prefixes: `"knowledge-mcp: add pattern"` → `"lore: add pattern"` (and
    similarly for update/append)
  - Change all `&PathBuf` to `&Path`
  - Write-operation tests use real git repos in tempdirs (no `FakeGit`)

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/ingest.rs` (orchestration parts only, not chunking)

  **Test scenarios:**
  - Ingest a directory of test markdown files with `FakeEmbedder`, verify chunk counts
  - Ingest empty directory returns zero files/chunks
  - Ingest with an unreadable file records an error but continues
  - `add_pattern` creates a file with correct frontmatter and content
  - `add_pattern` to existing file returns error
  - `update_pattern` overwrites content, preserves title
  - `append_to_pattern` adds a section to existing content
  - `slugify` produces filename-safe strings
  - `index_single_file` deletes old chunks before inserting new ones
  - `index_single_file` respects the configured chunking strategy (not just heading mode)

  **Verification:**
  - All ingest tests pass with `FakeEmbedder` and real SQLite (in-memory)
  - Write operations create correct markdown files in tempdir
  - No `knowledge-mcp` in git commit messages or anywhere else
  - `just ci` passes after Layer 1 is complete

### Layer 2: Interface and Infrastructure

- [ ] **Unit 7: Provision module**

  **Goal:** Port Ollama provisioning with portability fix.

  **Requirements:** R5, R7, R9

  **Dependencies:** Unit 3 (OllamaClient for health/model checks)

  **Files:**
  - Modify: `src/provision.rs`
  - Test: `src/provision.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Port `provision`, `check_status`, `check_ollama_binary`, `start_ollama`
  - Replace `Command::new("which").arg("ollama")` with `Command::new("ollama").arg("--version")`
    (R7)
  - Port `ProvisionResult` struct
  - Change `&PathBuf` to `&Path` where applicable

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/provision.rs`

  **Test scenarios:**
  - `ProvisionResult` struct has correct default state
  - `check_status` returns expected structure (test the struct, not system commands)
  - `check_ollama_binary` uses `ollama --version` not `which` (verify by reading the source)

  **Verification:**
  - Module compiles with no warnings
  - `just ci` passes with provision added

- [ ] **Unit 8: MCP server**

  **Goal:** Port and refactor the hand-rolled MCP JSON-RPC server.

  **Requirements:** R1, R5, R8, R9

  **Dependencies:** Unit 4 (database), Unit 3 (Embedder), Unit 6 (ingest), Unit 1 (config)

  **Files:**
  - Modify: `src/server.rs`
  - Test: `src/server.rs` (inline `#[cfg(test)] mod tests`)

  **Approach:**
  - Port `start_mcp_server`, `handle_request`, `handle_tool_call`, all tool handlers
  - Rename server info from `knowledge-mcp` to `lore`
  - Rename log prefixes from `[knowledge-mcp]` to `[lore]`
  - Refactoring: introduce a context struct with a single lifetime parameter holding `&KnowledgeDB`,
    `&dyn Embedder`, and `&Config` to reduce parameter threading. No `Arc`/`Mutex` needed — the
    server loop is synchronous and single-threaded.
  - Refactoring: consolidate response construction helpers; add validation that `jsonrpc == "2.0"`
    instead of silently ignoring it
  - Tool handlers take `&dyn Embedder` instead of `&OllamaClient`
  - **Fix hardcoded 768 dimensions**: The scaffold's `start_mcp_server` opens the database with
    `KnowledgeDB::open(&config.database, 768)`. Derive dimensions from the embedder instead.
  - `start_mcp_server` takes `&dyn Embedder` for the server to use during search/write operations
  - Use `insta::assert_json_snapshot!` for JSON-RPC response snapshots (normalizes formatting).
    **Prerequisite:** Add `features = ["json"]` to the `insta` dev-dependency in `Cargo.toml` —
    `assert_json_snapshot!` is feature-gated and won't compile without it.
  - **Test strategy:** Unit-test `handle_request` directly (pass JSON string in, get response out)
    rather than testing the full `start_mcp_server` stdio loop. This avoids needing threaded piped
    I/O in tests while covering dispatch logic, tool handling, and response format. The stdio loop
    itself is trivial (read line, call handle_request, write response).

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/server.rs`
  - MCP protocol: JSON-RPC 2.0, `initialize`, `tools/list`, `tools/call`

  **Test scenarios:**
  - `initialize` response has correct protocol version and server name `lore`
  - `tools/list` returns all 4 tools with correct schemas
  - `search_patterns` tool call returns formatted results
  - `add_pattern` tool call creates a pattern and returns confirmation
  - Unknown method returns JSON-RPC error -32601
  - Unknown tool returns JSON-RPC error -32602
  - Missing required fields return appropriate errors
  - Snapshot tests (insta) for `initialize` and `tools/list` responses

  **Verification:**
  - All server tests pass with `FakeEmbedder` and in-memory database
  - No `knowledge-mcp` references in server output
  - No hardcoded dimension values — derived from embedder
  - Response format matches MCP protocol expectations

- [ ] **Unit 9: CLI and integration wiring**

  **Goal:** Port the full CLI with all 5 subcommands and wire everything together.

  **Requirements:** R1, R5, R9, R10, R11

  **Dependencies:** All previous units

  **Files:**
  - Modify: `src/main.rs`
  - Modify: `tests/smoke.rs` (extend with subcommand tests)
  - Test: `tests/smoke.rs`

  **Approach:**
  - Port `Cli` struct with `Commands` enum (Init, Ingest, Serve, Search, Status)
  - Port command handler functions (`cmd_init`, `cmd_ingest`, `cmd_serve`, `cmd_search`,
    `cmd_status`)
  - Use `use lore::{config, database, embeddings, ...}` imports — not the scaffold's `mod`
    declarations. The project uses a lib+bin split; `main.rs` is a thin binary that delegates to the
    library crate.
  - **Fix hardcoded 768 in `cmd_status`**: The scaffold opens the database with
    `KnowledgeDB::open(
    &config.database, 768)`. Derive dimensions from the embedder or config
    instead.
  - `register_sqlite_vec()` is called internally by `KnowledgeDB::open` via `Once` — no explicit
    call needed in `main.rs`
  - All `eprintln!` prefixes change from `knowledge-mcp` to `lore`
  - Default config path is `lore.toml`
  - Wire `OllamaClient` as the concrete `Embedder` implementation
  - MCP config example in `cmd_init` output uses `lore` naming (both the command and the server key
    name)

  **Patterns to follow:**
  - Scaffold `tmp/scaffold/main.rs`
  - Existing `tests/smoke.rs` pattern (assert_cmd + predicates)

  **Test scenarios:**
  - `--help` shows `lore` name and all subcommands
  - `--version` shows version from Cargo.toml
  - `search` without query shows usage error
  - `init` without `--repo` shows required argument error
  - Each subcommand is parseable (clap validation)
  - No `knowledge-mcp` in any help text or output

  **Verification:**
  - `cargo build` produces working `lore` binary
  - `just ci` passes — all formatting, clippy, tests, deny, doc checks pass
  - Smoke tests verify subcommand availability
  - `grep -r "knowledge-mcp" src/` returns no results
  - No hardcoded dimension values in any `KnowledgeDB::open` call

## System-Wide Impact

- **Interaction graph:** Config is read by every command. Database is used by ingest, search, serve,
  and status. Embedder is used by ingest, search, and serve. Git is used only by write operations in
  ingest. Provision is used only by init and status.
- **Error propagation:** All modules use `anyhow::Result`. Errors propagate up to `main()` which
  prints them and exits with code 1. No panics in library code. The ingest module uses a different
  pattern: `IngestResult` collects errors into a `Vec<String>` and continues processing. Write
  operations use `anyhow::Result`. This three-way error pattern (Result, IngestResult,
  JsonRpcResponse error) is intentional divergence across different concerns.
- **State lifecycle risks:** sqlite-vec registration is process-global via `Once` — safe for
  concurrent test execution. Database uses WAL mode for concurrent reads. Write operations
  (add/update/append) are not atomic across file write + DB update + git commit — a failure mid-way
  leaves partial state. This is acceptable because the database is a derived artifact (re-ingest
  recovers). Note: `std::fs::write` is not atomic on most filesystems; a write-then-rename pattern
  would be more robust but is deferred as a future hardening opportunity.
- **API surface parity:** The 4 MCP tools and 5 CLI commands are the only interfaces. Both must use
  the same `Embedder` trait boundary and database layer. Both must derive dimensions from the
  embedder, never hardcode.
- **Integration coverage:** Unit tests cover each module in isolation. The CLI smoke tests provide
  basic integration coverage. Full end-to-end testing (init → ingest → search → serve) requires
  Ollama and is deferred.

## Risks & Dependencies

- **sqlite-vec FFI compatibility**: The `transmute` pattern is documented but brittle across
  rusqlite/sqlite-vec version bumps. Mitigated by pinning versions in Cargo.toml and testing
  extension registration in the database test suite.
- **ureq 3 streaming API**: The `pull_model` streaming response uses `resp.into_reader()` which does
  not exist in ureq 3. The exact replacement API is deferred to implementation discovery. Mitigated
  by the fact that pull functionality is only used during `lore init` and is tested manually, not by
  the automated test suite.
- **Clippy pedantic surprises**: The scaffold code will trigger lints we haven't anticipated.
  Mitigated by fixing lint-by-lint during compilation — this is mechanical work, not design risk.
- **cargo-deny license changes**: New transitive deps from ureq 3 may need license additions.
  Mitigated by running `cargo deny check` after each layer and adding licenses as needed.
- **SearchResult field drift**: `SearchResult` and `Chunk` share many fields. Composition
  (`SearchResult { chunk: Chunk, score }`) was considered but deferred to post-port cleanup to avoid
  cross-module ripple during the initial port. If fields diverge, a follow-up refactor can address
  it.

## Sources & References

- **Origin document:**
  [scaffold-porting-requirements.md](../brainstorms/2026-03-27-scaffold-porting-requirements.md)
- **Phase 0 plan:**
  [phase0-project-infrastructure-plan.md](2026-03-25-001-feat-phase0-project-infrastructure-plan.md)
  (testing strategy, fake strategy, snapshot conventions)
- **GENESIS:** `tmp/GENESIS.md` (architecture decisions, MCP design, naming conventions)
- **Phase 0 requirements:**
  [phase0-project-infrastructure-requirements.md](../brainstorms/2026-03-24-phase0-project-infrastructure-requirements.md)
  (R5 unsafe_code, R3 dependency versions, sqlite-vec init pattern)
- Scaffold reference: `tmp/scaffold/` (all 8 source files)
- sqlite-vec docs: `sqlite3_auto_extension` pattern with rusqlite
- ureq 3 docs: agent config, `body_mut().read_json()`, `into_parts()`
