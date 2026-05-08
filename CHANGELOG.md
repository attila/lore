# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Universal-pattern predicate (`applies_when`)** — universal-tagged patterns may now declare an
  optional frontmatter block gating their re-injection by tool class and Bash command prefix. Two
  keys (`tools`, `bash_command_starts_with`) compose with OR semantics within each list and AND
  semantics across keys. The Bash command-prefix matcher walks past `sudo`, `sudo -u USER`, `env`,
  `env -i`, `env -u VAR`, and `env KEY=VAL` wrapper tokens before checking the prefix. Documented
  limitations: nested env wrappers and quoted-command (`bash -c "..."`) extraction. See
  [`docs/pattern-authoring-guide.md`](docs/pattern-authoring-guide.md).
- **`min_relevance_universal` config knob** — optional per-tier score floor under `[search]`.
  Defaults to inheriting from `min_relevance` so an upgrade introduces no behaviour change without
  explicit config. Numerical complement to `applies_when`'s categorical gate.
- **Engine/adapter split** — predicate evaluator, smart-prefix matcher, query extraction, and
  pure-string helpers now live in a new agent-agnostic `src/engine/` module operating on a minimal
  `CallContext`. `src/hook.rs` becomes a Claude-Code-specific adapter that owns `HookInput`
  deserialisation, the `HookInput → CallContext` conversion, and the eager transcript-tail read.
  Future agent integrations (Cursor, opencode, etc.) build their own adapter and reuse the same
  engine.

### Changed

- **Knowledge database schema bumped to v3** with a forward-compatible ALTER TABLE migration on
  first open — no `lore ingest --force` required. The migration is wrapped in a transaction and uses
  column-presence checks for idempotency, so a partial migration that crashed (or lost a race
  against a concurrent open) re-enters the branch safely. Existing chunks have NULL in the new
  `applies_when_json` column, behaving as if no predicate were set (R11). The hard-bail schema
  advisory remains for any future non-additive bump.

### Notes

- This release is additive at every user-facing surface (CLI, MCP tool params, config). The schema
  bump is one-way: a v3 database cannot be opened by a v0.1.x lore binary, which would see a
  higher-than-expected `user_version` and bail with the existing schema-mismatch error.
  Re-installing a v0.1.x binary against a v3 database requires a manual rebuild via
  `lore ingest --force`.

## [0.1.0] - 2026-05-01

First stable release. No user-facing changes since `0.1.0-alpha.1`; promoting after end-to-end
pipeline validation. See `[0.1.0-alpha.1]` below for the full feature list.

## [0.1.0-alpha.1] - 2026-05-01

### Added

First public release. Lore is a local semantic search engine for software patterns and conventions,
exposed as an MCP server for Claude Code and other MCP clients. Single Rust binary with SQLite,
FTS5, and sqlite-vec compiled in; only runtime dependency is Ollama for embeddings.

#### Core

- **MCP server** — `lore serve` exposes five tools over stdio: `search_patterns`, `add_pattern`,
  `update_pattern`, `append_to_pattern`, `lore_status`. Designed for Claude Code's MCP transport but
  works with any MCP client.
- **Hybrid search** — combines FTS5 lexical search and sqlite-vec vector similarity via Reciprocal
  Rank Fusion. Title and tag matches weighted above body text. RRF scores normalised to 0–1 range.
  FTS5 porter stemming for improved recall. Ollama fallback warning and `min_relevance` threshold
  guard against poor-quality results. Set `hybrid = false` in `lore.toml` to skip Ollama at query
  time.
- **DB as sole runtime read surface** — pattern bodies live in a `patterns` table (`source_file` PK,
  `title`, `tags`, `is_universal`, `raw_body`, `content_hash`, `ingested_at`); `SessionStart` and
  `PostCompact` render from the DB instead of re-reading source markdown. Sandboxed read-only agents
  no longer need filesystem access to the patterns directory for the pinned-render path. Agents that
  call write tools still need write access because those tools persist markdown to disk as the
  authoring surface. See `docs/architecture.md` for the codified invariant.

#### Ingestion

- **Delta ingest** — `lore ingest` only re-indexes changed, added, moved, and deleted files since
  the last-ingested commit (via `git diff --name-status`), eliminating the Ollama round-trip penalty
  for unchanged files.
- **Single-file ingest** — `lore ingest --file <path>` indexes one file without requiring a git
  commit, enabling the fast edit-ingest-search loop for pattern authoring. Respects `.loreignore`;
  `--force` overrides.
- **`.loreignore`** — gitignore-style exclude file in pattern repositories. Filters files during
  full and delta ingest, with reconciliation when the file changes. Supports negation patterns,
  directory globs, and recursive globs via the `ignore` crate.

#### Claude Code integration

- **Plugin** (`integrations/claude-code/`) — SessionStart priming with pinned universal patterns,
  PreToolUse hook for relevance-gated pattern injection, PostCompact reset, error hook. Includes
  `/search` and `/coverage-check` skills.
- **Universal patterns** — patterns whose `tags:` frontmatter list contains `universal` get
  always-on injection at SessionStart (full body in a `## Pinned conventions` section) and bypass
  the PreToolUse dedup filter so they re-inject on every relevant tool call. Additive beyond
  `top_k`; relevance gate intact. Closes the always-on discoverability gap for process-level
  conventions like commit messages, push discipline, and branch naming. See
  `docs/pattern-authoring-guide.md` for the new "When to use the universal tag" section.
- **`/coverage-check` skill** — automates the Vocabulary Coverage Technique from the pattern
  authoring guide by simulating the PreToolUse hook's own query extraction on synthetic tool calls
  (via `lore extract-queries`), ingesting a draft pattern, searching in parallel, and iterating on
  edit suggestions until the surfaced-query set stabilises.

#### Operations and tooling

- **Release process** — pushing a `v*` tag to GitHub triggers a workflow
  (`.github/workflows/release.yml`) that cross-compiles `lore` for four targets via `cargo-zigbuild`
  from a single Linux runner (`x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`,
  `aarch64-apple-darwin`, `x86_64-apple-darwin`), packages each binary into a tarball with both
  license files and the README, computes a `SHA256SUMS` file for integrity verification, and
  publishes a GitHub Release with the matching CHANGELOG section as the body. The publish step is
  gated by a `release` GitHub Environment that requires owner approval — push permission alone
  cannot ship a release. CI gains a cross-compile smoke job for the same four targets so
  cross-compile breakage surfaces on every PR. New `just release-prep VERSION` recipe rotates the
  CHANGELOG and bumps `Cargo.toml`. Maintainer runbook at
  [`docs/release-process.md`](docs/release-process.md).
- **`LORE_DEBUG=1`** verbose logging via env var; **`--json`** structured output for `search`,
  `list`, and `status` for script and agent consumers.
- **Security hardening** — input limits, transcript path validation under `$HOME`, bounded tail-read
  (32 KB), deduplication file locking via `fd-lock` with FNV-1a hashing. See `SECURITY.md`.
- **Documentation** — pattern authoring guide, search mechanics reference, hook pipeline and plugin
  reference, configuration reference, release-process runbook.
