---
title: "feat: Phase 0 — Project Infrastructure and Quality Gates"
type: feat
status: active
date: 2026-03-25
origin: docs/brainstorms/2026-03-24-phase0-project-infrastructure-requirements.md
deepened: 2026-03-26
---

# Phase 0: Project Infrastructure and Quality Gates

## Overview

Stand up the complete Rust project skeleton for lore — a local semantic search MCP server — with all
quality gates (formatting, linting, testing, dependency auditing, documentation) enforced from the
first commit. Every subsequent change enters a repo that already passes `just ci`.

## Problem Frame

Lore has ~1,800 lines of scaffolded code that has never been compiled. Before porting any of that
logic, the project needs a solid engineering foundation: correct dependency versions, enforced code
quality, and a locally-runnable CI process. The project will eventually be open-sourced, so the
quality bar must be high from day one. (see origin:
docs/brainstorms/2026-03-24-phase0-project-infrastructure-requirements.md)

## Requirements Trace

- R1. Rust 2024 edition, MSRV 1.85
- R2. Toolchain pinning via rust-toolchain.toml (channel "1.85", rustfmt + clippy components)
- R3. Updated dependencies (rusqlite 0.39/bundled, ureq 3/json, sqlite-vec 0.1, clap 4, anyhow 1,
  serde 1, serde_json 1, toml 0.8, walkdir 2)
- R4. Clippy pedantic at warn (priority -1), selective allows
- R5. `unsafe_code = "deny"` globally; no unsafe in skeleton
- R6. Unified formatting via dprint (Rust via exec/rustfmt, Markdown, TOML, JSON)
- R6a. .editorconfig (UTF-8, LF, trim trailing whitespace, 4-space Rust/TOML, 2-space
  YAML/JSON/Markdown)
- R7. cargo-deny with advisory, license, ban, source checks; permissive license allowlist
- R8. justfile with fmt, clippy, test, deny, doc, ci recipes
- R9. Clean skeleton: src/lib.rs + src/main.rs + 7 module stubs; `just ci` passes
- R10. All references use `lore` naming
- R11. Dual MIT/Apache-2.0 license files
- R12. Release profile (strip, lto, opt-level "z")

## Scope Boundaries

- **In scope:** All config files, license files, src/ skeleton, one smoke test, dev-dependencies for
  testing, pre-commit hook (.githooks/)
- **Out of scope:** Scaffold logic porting, Embedder trait, sqlite-vec unsafe init, GitHub Actions,
  release automation, README content
- **Out of scope:** Any feature work

## Context & Research

### Relevant Code and Patterns

- Scaffold code lives in `tmp/scaffold/` (gitignored). It provides the reference for module names
  and the eventual structure, but none of it enters the repo in this phase.
- Existing commit convention: lowercase type prefix (`doc:`, `chore:`).
- `Cargo.lock` is not gitignored (correct for a binary crate — should be committed).

### External References

- **dprint:** Uses `dprint-plugin-exec` (not the deprecated rustfmt WASM plugin) to shell out to
  `rustfmt`. Plugins: markdown-0.21.1.wasm, toml-0.7.0.wasm, json-0.21.3.wasm,
  exec-0.6.0/plugin.json.
- **cargo-deny 0.19:** All vulnerability advisories are now always errors (not configurable).
  `[graph]` section replaces old target config. `unmaintained` uses scope values ("workspace",
  "all", etc.).
- **sqlite-vec 0.1.7:** rusqlite is only a dev-dependency (^0.31.0). No Cargo semver conflict with
  rusqlite 0.39. C ABI is stable so `sqlite3_auto_extension` should work. Verified during scaffold
  porting phase.

## Key Technical Decisions

- **rusqlite 0.39 despite sqlite-vec testing gap:** No semver conflict (dev-dep only). Deferred
  verification to scaffold phase since skeleton doesn't touch sqlite-vec init. (see origin:
  Outstanding Questions)
- **dprint-plugin-exec for Rust formatting:** The WASM rustfmt plugin is deprecated. exec plugin
  shells out to `rustfmt` binary, which is guaranteed available via rust-toolchain.toml's components
  list.
- **Hand-written fakes over mockall:** Lore's trait surface is small (~2-3 traits). Hand-written
  `FakeEmbedder`, `FakeGit` are simpler, more readable, and avoid proc-macro compile-time cost.
- **Test crate selection:** `assert_cmd` + `predicates` (CLI testing), `insta` (snapshot testing for
  MCP responses), `tempfile` (filesystem test fixtures). No proptest initially.
- **One smoke test in Phase 0:** `tests/smoke.rs` runs `lore --help` via assert_cmd. Proves the
  binary builds and the test toolchain works. Real tests arrive with scaffold porting.
- **Clippy pedantic allow-list:** Starting with `module_name_repetitions`, `must_use_candidate`,
  `missing_errors_doc`, `missing_panics_doc`. May need tuning once real code is linted — this is
  expected and acceptable.

## Open Questions

### Resolved During Planning

- **sqlite-vec + rusqlite 0.39 compatibility:** No Cargo conflict (dev-dep only). C ABI is stable.
  Use 0.39, verify during scaffold port.
- **dprint rustfmt plugin:** Use exec plugin (WASM plugin deprecated). Requires `rustfmt` on PATH
  (provided by rust-toolchain.toml).
- **Testing framework:** assert_cmd + predicates + insta + tempfile. Hand-written fakes, no mockall.
- **Phase 0 test scope:** One smoke test (`lore --help`). Test infrastructure in dev-dependencies.

### Deferred to Implementation

- **Exact dprint plugin versions:** Versions listed here are from research (March 2026). The
  implementer should verify latest stable versions via `dprint config add`.
- **Pedantic clippy allow-list tuning:** The initial four allows may need adjustment once the
  skeleton compiles. This is iterative by nature.
- **deny.toml license exceptions:** Some transitive dependencies may use licenses not in the initial
  allowlist (e.g., `Unicode-DFS-2016`, `OpenSSL`). Handle exceptions as they surface during
  `cargo deny check`.

## Testing Strategy

> _This section defines the testing philosophy and infrastructure for lore. Phase 0 establishes the
> foundation; later phases fill in the actual test coverage._

### Testing Layers

| Layer                 | Tool                              | What it covers                                                       | When introduced        |
| --------------------- | --------------------------------- | -------------------------------------------------------------------- | ---------------------- |
| **Unit tests**        | Built-in `#[cfg(test)]`           | Individual functions: chunking, RRF, slug generation, config parsing | Scaffold porting phase |
| **Integration tests** | `tests/` directory + `assert_cmd` | CLI invocation, end-to-end flows (init, ingest, search)              | Scaffold porting phase |
| **Snapshot tests**    | `insta`                           | MCP JSON-RPC responses, search result formatting, tool definitions   | MCP server porting     |
| **Smoke test**        | `assert_cmd` + `predicates`       | Binary builds and runs (`lore --help`)                               | **Phase 0**            |
| **Filesystem tests**  | `tempfile`                        | Operations on temp knowledge repos, config files, databases          | Scaffold porting phase |

### Test Dependencies (Skeleton Phase)

All dev-dependencies are declared in Phase 0 so the testing vocabulary is established:

```
[dev-dependencies]
assert_cmd = "2"
predicates = "3"
insta = "1"
tempfile = "3"
```

### Fake Strategy (Scaffold Phase, Not Phase 0)

Hand-written fakes for external boundaries:

- **`FakeEmbedder`** — Implements the `Embedder` trait. Returns deterministic fixed-dimension
  vectors (e.g., all zeros, or hash-based). Configurable to return errors for failure-path testing.
- **`FakeGit`** — Records calls (file path, commit message) without touching a real repo.
  Configurable success/failure.
- No faking of SQLite — real in-memory databases (`:memory:`) via rusqlite's bundled SQLite.

### Test Conventions

- Unit tests: inline `#[cfg(test)] mod tests` at the bottom of each source file
- Integration tests: `tests/` directory, one file per major feature area
- Shared test utilities: `tests/common/mod.rs`
- Snapshot files: insta stores snapshots in a `snapshots/` sibling directory relative to the test
  file (e.g., tests in `src/config.rs` produce `src/snapshots/config__*.snap`, tests in
  `tests/mcp.rs` produce `tests/snapshots/mcp__*.snap`). Committed to git and reviewed in PRs.
- Test data: `tests/fixtures/` for sample markdown files, configs, etc.

### What Phase 0 Delivers

- Dev-dependencies in Cargo.toml
- `tests/smoke.rs` — one test: `lore --help` exits 0 and output contains "lore"
- `just test` recipe that runs `cargo test`
- All of the above passing in `just ci`

## Implementation Units

- [ ] **Unit 1: Cargo.toml and rust-toolchain.toml**

  **Goal:** Create the project manifest and pin the toolchain.

  **Requirements:** R1, R2, R3, R4, R5, R10, R12

  **Dependencies:** None

  **Files:**
  - Create: `Cargo.toml`
  - Create: `rust-toolchain.toml`

  **Approach:**
  - Cargo.toml: package name `lore`, edition 2024, rust-version "1.85", license "MIT OR Apache-2.0"
  - All dependencies at versions specified in R3
  - Dev-dependencies: assert_cmd, predicates, insta, tempfile
  - `[lints.clippy]` section: pedantic at warn (priority -1), named allows for
    `module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`, `missing_panics_doc`
  - `[lints.rust]` section: `unsafe_code = "deny"`
  - `[profile.release]`: strip, lto, opt-level "z"
  - rust-toolchain.toml: channel "1.85", components ["rustfmt", "clippy"]

  **Patterns to follow:**
  - Lint configuration in Cargo.toml (not clippy.toml) per 2026 Rust convention
  - `priority = -1` on group-level lints so individual overrides take precedence

  **Verification:**
  - `cargo check` succeeds (requires Unit 2 for source files)

- [ ] **Unit 2: Source skeleton (lib.rs, main.rs, module stubs)**

  **Goal:** Create the minimum compilable source tree.

  **Requirements:** R9, R10

  **Dependencies:** Unit 1

  **Files:**
  - Create: `src/lib.rs`
  - Create: `src/main.rs`
  - Create: `src/config.rs`
  - Create: `src/database.rs`
  - Create: `src/embeddings.rs`
  - Create: `src/git.rs`
  - Create: `src/ingest.rs`
  - Create: `src/provision.rs`
  - Create: `src/server.rs`

  **Approach:**
  - `src/lib.rs`: declares all 7 modules as `pub mod config;` etc. This is the module tree root.
  - `src/main.rs`: minimal clap setup — derive a `Cli` struct with app name "lore", version, and
    about. Parse args in `main()`. This is needed so that the smoke test (`lore --help`) has
    meaningful output to assert against.
  - Each module stub: **empty files are sufficient**. Clippy pedantic lints only analyze code that
    exists — empty modules pass clean with zero warnings. No placeholder functions or comments
    needed. (Verified: `missing_docs` is a rustc lint, not part of clippy::pedantic, and is allowed
    by default.)
  - All naming uses `lore` — no `knowledge-mcp` references anywhere.

  **Patterns to follow:**
  - Standard Rust binary crate layout: src/main.rs (thin) + src/lib.rs (module tree)
  - Empty module files are idiomatic Rust for stubs — the compiler and clippy handle them correctly

  **Verification:**
  - `cargo build` succeeds
  - `cargo clippy --all-targets -- -D warnings` passes with zero warnings
  - Binary is named `lore`

- [ ] **Unit 3: License files**

  **Goal:** Add dual license files.

  **Requirements:** R11

  **Dependencies:** None (can be done in parallel with Units 1-2)

  **Files:**
  - Create: `LICENSE-MIT`
  - Create: `LICENSE-APACHE`

  **Approach:**
  - LICENSE-MIT: standard MIT license text with copyright holder and year
  - LICENSE-APACHE: standard Apache License 2.0 full text
  - Both reference the same copyright holder

  **Verification:**
  - Files exist at repo root
  - `license = "MIT OR Apache-2.0"` in Cargo.toml matches the files present

- [ ] **Unit 4: .editorconfig**

  **Goal:** Establish editor-level consistency for all file types.

  **Requirements:** R6a

  **Dependencies:** None (can be done in parallel)

  **Files:**
  - Create: `.editorconfig`

  **Approach:**
  - `root = true`
  - Default section `[*]`: charset utf-8, end_of_line lf, trim_trailing_whitespace true,
    insert_final_newline true, indent_style space, indent_size 4
  - `[*.{yml,yaml,json,md}]`: indent_size 2
  - `[Makefile]`: indent_style tab (defensive, even though we use just)
  - No Markdown trailing-whitespace exception — use `<br>` or `\` for explicit line breaks

  **Verification:**
  - File exists and is well-formed

- [ ] **Unit 5: dprint configuration**

  **Goal:** Configure unified formatting for Rust, Markdown, TOML, and JSON.

  **Requirements:** R6

  **Dependencies:** Unit 1 (rust-toolchain.toml must exist so rustfmt is available)

  **Files:**
  - Create: `dprint.json`

  **Approach:**
  - Global settings: lineWidth 100, indentWidth 4, useTabs false, newLineKind lf
  - Plugins: markdown, toml, json (WASM), exec (for rustfmt)
  - markdown plugin: `textWrap: "always"` — enforces prose wrapping at lineWidth (100). This is
    critical for readable markdown; the default `"maintain"` would preserve arbitrarily long lines.
  - exec commands: `rustfmt --edition 2024` for `.rs` files, cacheKeyFiles includes
    `rust-toolchain.toml`
  - json plugin: indentWidth 2
  - excludes: `**/target`, `tmp`
  - Plugin URLs from research: markdown-0.21.1, toml-0.7.0, json-0.21.3, exec-0.6.0 — verify latest
    at implementation time

  **Patterns to follow:**
  - dprint-plugin-exec pattern (not deprecated WASM rustfmt plugin)

  **Verification:**
  - `dprint check` passes on all tracked files (Cargo.toml, dprint.json, docs/_.md, src/_.rs)
  - All Markdown files in `docs/` are consistently formatted

- [ ] **Unit 6: Pre-commit hook**

  **Goal:** Enforce formatting on every commit via a git pre-commit hook.

  **Requirements:** R6, R8

  **Dependencies:** Unit 5 (dprint must be configured)

  **Files:**
  - Create: `.githooks/pre-commit`

  **Approach:**
  - Shell script that runs `dprint check`. If it fails, the commit is rejected with a message
    telling the developer to run `dprint fmt`.
  - The hook is stored in a tracked `.githooks/` directory. Activation is handled by the
    `just setup` recipe (Unit 8) which runs `git config core.hooksPath .githooks`.
  - The hook must be executable (`chmod +x`).
  - Keep the hook minimal — only `dprint check`, not the full CI pipeline. Fast feedback (~1s) on
    every commit; full CI is run manually via `just ci`.

  **Verification:**
  - An unformatted markdown file causes `git commit` to fail with a clear error message
  - A properly formatted commit succeeds without delay
  - The hook script is tracked in git and executable

- [ ] **Unit 7: cargo-deny configuration**

  **Goal:** Configure dependency auditing with all checks enabled.

  **Requirements:** R7

  **Dependencies:** Unit 1 (Cargo.toml with dependencies must exist)

  **Files:**
  - Create: `deny.toml`

  **Approach:**
  - `[graph]`: targets = ["x86_64-unknown-linux-gnu"], all-features = true
  - `[advisories]`: unmaintained = "workspace", yanked = "deny"
  - `[licenses]`: confidence-threshold = 0.93, allow-list: MIT, Apache-2.0, Apache-2.0 WITH
    LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib. Add exceptions as needed if
    transitive deps use other permissive licenses (e.g., Unicode-DFS-2016, OpenSSL).
  - `[bans]`: multiple-versions = "warn", wildcards = "deny"
  - `[sources]`: unknown-registry = "deny", unknown-git = "deny"

  **Verification:**
  - `cargo deny check` passes with all four check categories enabled
  - If license exceptions are needed, they are documented with rationale

- [ ] **Unit 8: justfile**

  **Goal:** Create the local CI task runner with all quality gate recipes and a setup recipe for the
  pre-commit hook.

  **Requirements:** R8

  **Dependencies:** Units 1, 2, 5, 6, 7 (all tools must be configured first)

  **Files:**
  - Create: `justfile`

  **Approach:**
  - Recipes:
    - `setup`: `git config core.hooksPath .githooks` — activates the pre-commit hook. Run once after
      clone.
    - `fmt`: `dprint check`
    - `fmt-fix`: `dprint fmt` (convenience, not part of CI)
    - `clippy`: `cargo clippy --all-targets -- -D warnings`
    - `test`: `cargo test`
    - `deny`: `cargo deny check`
    - `doc`: `cargo doc --no-deps`
    - `ci`: runs fmt, clippy, test, deny, doc in sequence
  - The `ci` recipe should call each sub-recipe sequentially (not as just dependencies) so that
    failures stop the pipeline immediately with a clear error
  - Default recipe (no arguments): list available recipes via `just --list`
  - Each recipe should have a `# comment` description for `just --list` output

  **Patterns to follow:**
  - just recipe-per-task pattern, `ci` as a sequence of recipe invocations

  **Verification:**
  - `just ci` passes end-to-end
  - `just setup` activates the pre-commit hook
  - `just --list` shows all recipes with descriptions

- [ ] **Unit 9: Smoke test**

  **Goal:** Prove the binary builds and the test toolchain works.

  **Requirements:** R8 (test recipe), R9 (skeleton compiles)

  **Dependencies:** Units 1, 2, 7

  **Files:**
  - Create: `tests/smoke.rs`

  **Approach:**
  - Single test function using assert_cmd
  - Runs `lore --help` as a subprocess
  - Asserts: exit code 0, stdout contains "lore"
  - This validates: binary compiles, clap is wired up, assert_cmd + predicates work

  **Test scenarios:**
  - `lore --help` exits 0 and output contains the binary name
  - (Future phases will add: `lore --version`, subcommand help, error cases)

  **Verification:**
  - `cargo test` passes
  - `just test` passes
  - `just ci` passes (full pipeline including this test)

- [ ] **Unit 10: Update .gitignore and final CI validation**

  **Goal:** Ensure .gitignore is complete and the full CI pipeline passes.

  **Requirements:** All

  **Dependencies:** All previous units

  **Files:**
  - Modify: `.gitignore`

  **Approach:**
  - Verify existing entries are correct (target/, *.rs.bk, *.pdb, tmp)
  - Ensure `Cargo.lock` is NOT gitignored (binary crate — lock file should be committed)
  - Add any dprint-specific ignores if needed

  **Verification:**
  - `just ci` passes end-to-end on a clean state
  - `cargo build --release` produces a binary named `lore`
  - `git status` shows all expected files as untracked (ready to commit)
  - No `knowledge-mcp` string appears anywhere in tracked files

## System-Wide Impact

- **Interaction graph:** Phase 0 has no runtime behavior. The skeleton's `main.rs` is a placeholder.
  No callbacks, middleware, or observers.
- **Error propagation:** Not applicable — no business logic in the skeleton.
- **State lifecycle risks:** None. The database and knowledge directory are not created or touched.
- **API surface parity:** Not applicable — no MCP tools exposed in the skeleton.
- **Integration coverage:** The smoke test validates the build-to-binary pipeline. Cross-module
  integration testing begins in the scaffold porting phase.

## Risks & Dependencies

- **dprint plugin version drift:** Plugin URLs are pinned to specific versions from research. If
  versions have moved by implementation time, use `dprint config add <plugin>` to get the latest.
- **cargo-deny license exceptions:** Transitive dependencies may use licenses not in the initial
  allowlist. This is expected — add exceptions with documented rationale rather than relaxing the
  allowlist.
- **Clippy pedantic on stubs:** Verified that empty module files pass clippy pedantic cleanly — no
  lints fire on zero-item modules. The pedantic allow-list (`must_use_candidate`,
  `missing_errors_doc`, etc.) only becomes relevant when real code is ported. Tuning will be needed
  then, not now.
- **C compiler requirement:** rusqlite's `bundled` feature compiles SQLite from C source. If
  gcc/clang is missing on the build machine, the build will fail with a clear error.

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-03-24-phase0-project-infrastructure-requirements.md](../brainstorms/2026-03-24-phase0-project-infrastructure-requirements.md)
- **Scaffold code:** `tmp/scaffold/` (gitignored, reference only)
- **dprint docs:** https://dprint.dev/config/
- **cargo-deny docs:** https://embarkstudios.github.io/cargo-deny/
- **sqlite-vec Rust usage:** https://alexgarcia.xyz/sqlite-vec/rust.html
