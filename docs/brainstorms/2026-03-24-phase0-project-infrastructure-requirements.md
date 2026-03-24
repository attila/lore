---
date: 2026-03-24
topic: phase0-project-infrastructure
---

# Phase 0: Project Infrastructure and Quality Gates

## Problem Frame

Lore is a new Rust project with ~1,800 lines of scaffolded code that has never been compiled. Before any feature work begins, the project needs a solid engineering foundation: correct dependency versions, enforced code quality, and a locally-runnable CI process that validates every change. The project will eventually be open-sourced, so the quality bar must be high from day one.

## Requirements

- R1. **Rust 2024 edition** — Cargo.toml uses `edition = "2024"` with MSRV set to `rust-version = "1.85"`.
- R2. **Toolchain pinning** — A `rust-toolchain.toml` pins the channel to a specific version (matching MSRV, e.g. `"1.85"`) and includes `rustfmt` and `clippy` components. Toolchain updates are intentional, not automatic.
- R3. **Updated dependencies** — All crates at current stable versions:
  - `rusqlite = "0.39"` (bundled feature, drop `load_extension` — not needed with `sqlite3_auto_extension`)
  - `ureq = "3"` (features = ["json"])
  - `sqlite-vec = "0.1"` (resolves to 0.1.7+)
  - `clap`, `anyhow`, `serde`, `serde_json`, `toml`, `walkdir` — keep current major version specs
- R4. **Clippy pedantic** — `[lints.clippy]` in Cargo.toml enables `pedantic` at warn level (priority -1), with selective allows for noisy lints (`module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`, `missing_panics_doc`).
- R5. **Unsafe denied globally** — `[lints.rust] unsafe_code = "deny"`. No unsafe code in the skeleton. The targeted `#[allow(unsafe_code)]` for sqlite-vec FFI initialization (via `sqlite3_auto_extension` + `std::mem::transmute`) is added during the scaffold-porting phase.
- R6. **Unified formatting via dprint** — `dprint` as the single formatter for all file types: Rust (wraps rustfmt), Markdown, TOML, JSON. Config in `dprint.json`. Replaces standalone `rustfmt.toml` — rustfmt config lives inside dprint's Rust plugin settings. `cargo fmt` still works locally but CI uses `dprint check`.
- R6a. **Editor config** — An `.editorconfig` with: `root = true`, UTF-8, LF line endings, `trim_trailing_whitespace = true` (all files, no Markdown exception — use `<br>` or `\` for explicit line breaks), `insert_final_newline = true`, 4-space indent for Rust/TOML, 2-space for YAML/JSON/Markdown.
- R7. **Dependency auditing** — A `deny.toml` configured for advisory, license, ban, and source checks via `cargo-deny`. License: `MIT OR Apache-2.0` (dual license, Rust convention). License allowlist includes standard permissive licenses (MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib).
- R8. **Local CI via just** — A `justfile` with these recipes:
  - `fmt` — `dprint check` (formats Rust, Markdown, TOML, JSON)
  - `clippy` — `cargo clippy --all-targets -- -D warnings`
  - `test` — `cargo test`
  - `deny` — `cargo deny check`
  - `doc` — `cargo doc --no-deps` (catches broken doc links)
  - `ci` — runs all of the above in sequence; this is the gate for every change
- R9. **Clean skeleton** — Phase 0 delivers a compiling project with `src/lib.rs` (module tree) + `src/main.rs` (thin CLI entry point) and module stubs (not the full scaffold logic). Every module (`config`, `database`, `embeddings`, `git`, `ingest`, `provision`, `server`) exists as a file with minimal placeholder content. `just ci` passes on the skeleton.
- R10. **Naming** — All references use `lore` (binary name, config file `lore.toml`, database `lore.db`, Cargo.toml package name). No leftover `knowledge-mcp` references.
- R11. **License** — Project licensed as `MIT OR Apache-2.0`. Include both `LICENSE-MIT` and `LICENSE-APACHE` files. Set `license = "MIT OR Apache-2.0"` in Cargo.toml.
- R12. **Release profile** — `[profile.release]` retains `strip = true`, `lto = true`, `opt-level = "z"` for small binary size.

## Success Criteria

- `just ci` passes on a fresh clone with `rustup`, `just`, `dprint`, and `cargo-deny` installed (no Ollama needed at build/test time)
- `cargo build --release` produces a single binary named `lore`
- The skeleton compiles with zero warnings under clippy pedantic + `-D warnings`
- `cargo deny check` passes (advisories, licenses, bans, sources — all checks enabled)

## Scope Boundaries

- **In scope:** Cargo.toml, rust-toolchain.toml, dprint.json, .editorconfig, deny.toml, justfile, .gitignore, LICENSE-MIT, LICENSE-APACHE, src/main.rs, src/lib.rs, module stubs
- **Out of scope:** Porting scaffold logic into the skeleton (that is the next phase), Embedder trait (introduced when tests need it), sqlite-vec unsafe init (introduced during scaffold port), GitHub Actions CI, release automation, README/documentation content
- **Out of scope:** Any feature work — this phase is purely infrastructure

## Key Decisions

- **dprint over standalone rustfmt**: Single formatter for Rust + Markdown + TOML + JSON. Written in Rust, single binary. Wraps rustfmt for Rust code so formatting is identical. Avoids separate tooling for non-Rust files.
- **just over make/cargo-xtask**: `just` is the dominant Rust task runner in 2026. Simple syntax, no build system ambitions, self-documenting.
- **Clippy pedantic at warn, not deny**: Pedantic lints warn during development. CI uses `-D warnings` so they block merges but don't break local iteration.
- **Real SQLite, mock Ollama** (deferred to scaffold phase): SQLite is bundled and fast — no reason to abstract it. Ollama is an external network service — extracting an `Embedder` trait keeps tests fast and offline. Trait introduced when the first test needs it.
- **Dual MIT/Apache-2.0 license**: Rust ecosystem convention. Maximum compatibility.
- **Clean skeleton before scaffold port**: Ensures the quality infrastructure is validated independently. Every subsequent change enters a repo that already passes CI.
- **sqlite-vec via `sqlite3_auto_extension`** (deferred to scaffold phase): The scaffold's `loadable_extension_path()` call doesn't exist in the crate. The correct pattern is `unsafe { sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ()))) }` from `rusqlite::ffi`. Requires targeted `#[allow(unsafe_code)]` and possibly transmute-related clippy allows.
- **ureq 3.x**: Major rewrite from 2.x. The scaffold's `.into_json()` calls need migration to `.body_mut().read_json()`. Worth doing now rather than building on a deprecated API.

## Dependencies / Assumptions

- Build target is the developer's current Ubuntu server (x86_64-unknown-linux-gnu)
- `just` is installed or installable via `cargo install just`
- `cargo-deny` is installed or installable via `cargo install cargo-deny`
- `dprint` is installed or installable via `cargo install dprint`
- Rust toolchain managed via `rustup`
- A C compiler (gcc/clang) is available for rusqlite's bundled SQLite compilation

## Outstanding Questions

### Deferred to Planning

- [Affects R4][Technical] Exact list of pedantic lints to `allow` — may need tuning once real code is being linted
- [Affects R3][Needs research] Does sqlite-vec 0.1.7 have a rusqlite version constraint that excludes 0.39? If so, may need to pin rusqlite lower.

## Next Steps

→ `/ce:plan` for structured implementation planning
