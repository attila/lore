# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `lore status` inserts a new `Languages:` line between `Sources:` and `Last commit:` reporting a
  per-language source-count breakdown plus an `undeclared` bucket; the same data is exposed as
  `languages_declared` / `languages_undeclared` on the MCP `lore_status` tool's metadata fence for
  agent or script consumers. (#58)

## [0.3.1] - 2026-05-14

### Fixed

- `update_pattern` and `append_to_pattern` no longer leave stale chunks in the index when the
  knowledge directory is reached through a symlink (typical on macOS, and on any symlinked or
  bind-mounted setup). (#55)

## [0.3.0] - 2026-05-14

### Added

- An optional `language:` frontmatter field declares the languages a pattern targets, so declared
  patterns surface on relevant tool calls even when their bodies omit the canonical token. (#50)
- `lore ingest` reports a coverage tally distinguishing patterns that declare `language:` from those
  that fall back to FTS coincidence, and aggregates unknown-token warnings per token across the run.
  (#50)

### Changed

- Bash command language inference matches on whole tokens, so `bundle install` no longer falsely
  infers TypeScript via the `bun` substring. (#50)
- Shared signals across languages (e.g. `npm test`) now infer the full applicable set
  (`{javascript, typescript}`) rather than collapsing to a single arbitrary winner. (#50)
- Knowledge database schema bumped to v4 via a forward-compatible `ALTER TABLE` migration on first
  open; no `lore ingest --force` required. (#50)
- The XDG state tier (`$XDG_STATE_HOME`) is now documented and reachable via a new
  `default_trace_dir()` helper; XDG path resolution moves to the `etcetera` crate without changing
  default paths or `$HOME`-fallback semantics. (#52)

## [0.2.0] - 2026-05-12

### Added

- `add_pattern` distinguishes real slug collisions from intentional re-use (with a
  `(no title heading)` variant for orphan-frontmatter targets), NFC-normalises titles before
  slugifying, and rejects whitespace/newline injection at the write boundary. (#42)
- `lore ingest`, `lore serve`, and `lore_status` warn when the knowledge directory's effective scan
  set is empty, distinguishing filesystem-empty, all-`.loreignore`-excluded, and missing-directory
  cases. (#41)
- Universal-tagged patterns may declare an `applies_when` predicate to gate re-injection by tool
  class and Bash command prefix (OR within keys, AND across). (#39)
- `min_relevance_universal` adds a per-tier relevance floor under `[search]`, defaulting to inherit
  `min_relevance`. (#39)
- Engine/adapter split moves predicate evaluation, smart-prefix matching, and query extraction into
  agent-agnostic `src/engine/`, with `src/hook.rs` as the Claude-Code adapter. (#39)
- `lore ingest` skips non-UTF-8 filenames during the directory walk and surfaces them on
  `IngestResult::errors` instead of indexing them under U+FFFD-substituted keys. (#46)

### Changed

- `lore ingest` on a fresh `git init` with zero commits now prints
  `No commits yet — HEAD will be recorded after your first commit.` instead of the misleading "No
  previous ingest recorded" wording. (#45)
- Knowledge database schema bumped to v3 via a forward-compatible ALTER TABLE migration on first
  open; no `lore ingest --force` required. (#39)
- Predicated universal patterns are no longer pinned at SessionStart; they re-inject on every
  matching `PreToolUse` call via the predicate path. (#40)

### Notes

- Additive at every user-facing surface; the v3 schema bump is one-way — a v0.1.x binary against a
  v3 database requires `lore ingest --force` after re-install.

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
