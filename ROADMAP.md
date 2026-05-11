# Roadmap

## Up Next

- [ ] Universal-pattern predicate (`applies_when`) and engine/adapter split — Track 1 in flight on
      `feat/applies-when-predicate`. Adds an optional tool/command predicate to universal-tagged
      patterns, ships `min_relevance_universal` as a per-tier score floor, and reorganises hook code
      into an agent-agnostic engine plus a Claude-Code adapter. See
      `docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`
- [ ] Edge case handling — two remaining slices: no-HEAD progress line on a fresh `git init`
      (Slice C, R9–R10 + R11.2, R11.3) and lossy-path warning during directory walk (Slice D, R8 +
      R11.9, Unix-only test gating). Both are mutually independent. See
      `docs/brainstorms/2026-04-08-edge-case-handling-requirements.md` for the brainstorm and the
      _Implementation Slices_ table for the per-slice mapping. Slices A (Unicode NFC normalisation)
      and B (slug-collision detection) shipped together (see Completed below); the
      empty-knowledge-dir slice shipped earlier on its own branch; the missing-`git` binary
      regression test (Slice E) also shipped (see Completed below).
- [ ] Extend language detection dictionaries — currently six languages (Rust, TypeScript,
      JavaScript, YAML, Python, Go) in both extension-to-language and command-to-language maps. Add
      Ruby, Java, C/C++, C#, PHP, Swift, Kotlin, shell scripts, and keep both maps in sync. The Bash
      inference side is non-trivial: each language has multiple tools (`bundle`/`gem`/`rake` → Ruby,
      `javac`/`gradle`/`mvn` → Java, `dotnet` → C#, `swift build` → Swift, etc.). Consider
      extracting both maps into a shared data structure to prevent drift between them

## Future

- [ ] Evaluate transcript tail truncation limit — currently 200 bytes, which often cuts
      mid-sentence. Increasing to 400-500 bytes may improve search recall for longer user
      instructions without adding excessive noise. Use `LORE_DEBUG` traces to measure what gets
      truncated in practice
- [ ] Cycle-based deduplication TTL — re-inject a pattern after N tool call cycles since last
      injection, so long sessions don't bury early conventions deep in context
- [ ] Deny-first-touch mode — block the first Edit/Write per domain with conventions as the deny
      reason, forcing Claude to retry with conventions visible. Requires solid deduplication to
      avoid infinite loops (see
      `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`)
- [ ] Code content analysis for query enrichment — extract meaningful terms from `content` /
      `new_string` fields in Edit/Write tool input to improve search relevance
- [ ] Plugin marketplace distribution (Claude Code marketplace or self-hosted)
- [ ] Additional agent integrations (Cursor, opencode) under `integrations/`
- [ ] Install on PATH without building from source (Homebrew tap or similar)
- [ ] Absolute path output in `lore init` MCP config instructions

## Completed

- [x] Release process (prebuilt binaries via `cargo-zigbuild`, GitHub releases). Tag-triggered
      `release.yml` cross-compiles four targets from a single Linux runner, packages tarballs +
      `SHA256SUMS`, publishes via `gh release create` behind an owner-approval Environment gate.
      Per-platform install snippets in README, maintainer runbook at `docs/release-process.md`.
      First tag (`v0.1.0-alpha.1`) is the owner's follow-up. See
      `docs/plans/2026-04-30-001-feat-release-process-plan.md`
- [x] Project infrastructure (CI, quality gates, formatting, lints)
- [x] Port scaffold into working Rust binary (all modules compile, 96 tests)
- [x] Progress bar during model pull
- [x] Dev install via `just install`
- [x] End-to-end lifecycle tests (ingest → search → add/update/append → verify)
- [x] Branch push for agent submissions (per-submission branches via git plumbing)
- [x] XDG config paths and MCP CLI output in `lore init`
- [x] Integration tests for init output (requires Ollama)
- [x] Ollama integration tests for semantic search quality
- [x] CI action versions pinned to full commit SHAs
- [x] MCP integration testing with Claude Code (tool discovery, invocation, edge cases)
- [x] Ollama fallback warning and min_relevance threshold for search quality
- [x] Search relevance boosting (FTS5 column weights + embedding enrichment)
- [x] Score normalization (RRF scores mapped to 0–1 range)
- [x] Agent integration — Claude Code plugin with deterministic pattern injection
  - [x] Validation spike — confirmed `additionalContext` influences agent behavior
  - [x] `lore hook` subcommand — unified hook handler for all lifecycle events
  - [x] `lore list` subcommand + `--top-k` CLI flag + FTS5 query sanitization fix
  - [x] Plugin assembly (`integrations/claude-code/`)
  - [x] SessionStart priming, session deduplication, PostCompact reset, error hook
  - [x] Hook unit tests + search relevance regression tests (CI)
  - [x] See `docs/plans/2026-04-01-005-feat-agent-integration-claude-code-plan.md`
- [x] Delta ingest via git diff — only re-index changed, added, moved, and deleted files instead of
      full re-embed. Use `git diff --name-status` against the last-ingested commit to detect
      changes. Eliminates the Ollama round-trip penalty for unchanged files. See
      `docs/plans/2026-04-02-001-feat-delta-ingest-plan.md`
- [x] Dogfooding fixes — FTS5 hyphen crash, frontmatter chunk noise. See
      `docs/plans/2026-04-03-001-fix-dogfooding-findings-plan.md`
- [x] `LORE_DEBUG=1` verbose logging and `--json` structured output. See
      `docs/plans/2026-04-03-002-feat-debug-logging-json-output-plan.md`
- [x] FTS5 porter stemming for improved search recall. See
      `docs/plans/2026-04-04-001-feat-fts5-porter-stemming-plan.md`
- [x] Security hardening — input limits, transcript path validation under `$HOME`, bounded tail-read
      (32KB), deduplication file locking (`fd-lock`) with FNV-1a hashing, `SECURITY.md`. See
      `docs/plans/2026-04-04-001-feat-security-hardening-plan.md`
- [x] Product documentation — pattern authoring guide, search mechanics reference, hook pipeline and
      plugin reference, configuration reference. See
      `docs/plans/2026-04-05-001-doc-product-documentation-plan.md`
- [x] Dogfooding deferred — search relevance regression tests (PR #24), pattern strengthening
      (`rust/tooling.md`, `workflows/git-branch-pr.md`), memory→lore migration (3 memories retired).
      See `docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md`
- [x] `.loreignore` — gitignore-style exclude file in pattern repositories. Filters files during
      full and delta ingest, with reconciliation when the file changes. Supports negation patterns,
      directory globs, and recursive globs via the `ignore` crate. See
      `docs/plans/2026-04-06-001-feat-loreignore-plan.md`
- [x] Single-file ingest (`lore ingest --file <path>`) — index one file without requiring a git
      commit, enabling the fast edit-ingest-search feedback loop for pattern authoring. Orthogonal
      to delta state, respects `.loreignore` with a `--force` override. See
      `docs/plans/2026-04-06-002-feat-single-file-ingest-plan.md`
- [x] Coverage-check skill (`/coverage-check`) — automates the Vocabulary Coverage Technique from
      the pattern authoring guide by simulating the PreToolUse hook's own query extraction on
      synthetic tool calls (via `lore extract-queries`), ingesting the draft, searching in parallel,
      and iterating on edit suggestions until the surfaced-query set stabilises. Ships alongside the
      fenced `lore-metadata` content-block pivot for MCP tool responses. See
      `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md`
- [x] Edge case handling — Slices A + B (Unicode NFC normalisation in `slugify` and slug-collision
      detection in `add_pattern`). NFC normalisation makes `café` (NFC) and `café` (NFD) produce
      identical slugs. The collision discriminator distinguishes a real slug collision (two distinct
      titles sharing a slug, tier-1 hard-fail) from intentional re-use (same title, pointed at
      `update_pattern`); error names slug, filename, and existing title (or
      `(no title
      heading)`). Multi-reviewer pass added title sanitisation at the write
      boundary (trim whitespace, reject embedded newlines) and graceful read fallback for
      non-regular files at the slug path. R5 (NFD-on-disk vs NFC-incoming filename mismatch on
      Mac→Linux sync) is documented as a deferred limitation. See
      `docs/plans/2026-05-10-001-feat-unicode-nfc-slug-collisions-plan.md` and
      `docs/solutions/design-patterns/round-trip-discriminator-canonicalise-both-sides-2026-05-10.md`.
- [x] Edge case handling — Slice E (missing-`git` binary regression test). Integration test in
      `tests/edge_cases.rs` spawns `lore ingest` with `PATH` cleared on the child process only and
      asserts the missing-binary fallback fires the unique progress marker
      `Not a git repository —
      running full ingest` and exits 0. Codifies tier-3 silent
      fallback behaviour per the CLI behaviour ladder; no user-visible behaviour change. See
      `docs/plans/2026-05-11-001-feat-missing-git-regression-test-plan.md`.
- [x] Effective-empty knowledge directory warning — `lore ingest`, `lore serve`, and `lore_status`
      surface when the knowledge directory's effective scan set is empty (filesystem-empty,
      all-ignored, or missing). Tier-2 per the project's CLI behaviour ladder: warning to stderr,
      exit 0, no opt-out flag. The MCP `lore_status` tool reports `empty_knowledge_dir` and
      `knowledge_dir_status` (`"populated" | "empty" | "missing"`); the `lore status` CLI prints a
      `Scan set:` line with the same discrimination. Originated as one bullet of the
      edge-case-handling roadmap line; pivoted to a dedicated branch when the design crystallised
      the CLI behaviour ladder convention. See
      `docs/plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md` and
      `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md`.
- [x] Universal patterns via tag-based SessionStart injection — patterns whose `tags:` frontmatter
      list contains `universal` get full body emitted in a `## Pinned conventions` section at every
      SessionStart and PostCompact, AND bypass the PreToolUse dedup filter so they re-inject on
      every relevant tool call (additively beyond `top_k`, with the relevance gate intact). Closes
      the always-on discoverability gap for process-level conventions like push discipline that the
      coverage-check skill cannot address. Schema change requires `lore ingest --force` once after
      upgrading; a startup `PRAGMA table_info` probe refuses to start with a friendly advisory
      otherwise. See `docs/plans/2026-04-20-001-feat-universal-patterns-plan.md`
