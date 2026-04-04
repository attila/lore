# Roadmap

## Up Next

- [ ] Edge case handling (empty knowledge dir, non-git dir, duplicate titles, unicode filenames)
- [ ] Dogfooding deferred — search relevance regression tests, pattern strengthening, memory→lore
      migration evaluation. See `docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md`

## Future

- [ ] Pattern authoring guide — product documentation on how to write effective lore patterns.
      Covers descriptive vs. imperative content, incident context, tag strategy, chunking awareness,
      query-friendly vocabulary, and anti-patterns. Based on dogfooding evidence, not speculation.
      Iterated through real memory→lore migration cycles

- [ ] Cycle-based dedup TTL — re-inject a pattern after N tool call cycles since last injection, so
      long sessions don't bury early conventions deep in context
- [ ] Deny-first-touch mode — block the first Edit/Write per domain with conventions as the deny
      reason, forcing Claude to retry with conventions visible. Requires solid dedup to avoid
      infinite loops (see
      `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`)
- [ ] Universal patterns via tag-based SessionStart injection — patterns tagged `universal` get full
      content at SessionStart, not just titles. Covers process-level conventions that don't surface
      through file-edit hooks
- [ ] Code content analysis for query enrichment — extract meaningful terms from `content` /
      `new_string` fields in Edit/Write tool input to improve search relevance
- [ ] Plugin marketplace distribution (Claude Code marketplace or self-hosted)
- [ ] Additional agent integrations (Cursor, opencode) under `integrations/`
- [ ] Release process (prebuilt binaries via `cargo-zigbuild`, GitHub releases)
- [ ] Install on PATH without building from source (Homebrew tap or similar)
- [ ] Absolute path output in `lore init` MCP config instructions

## Completed

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
  - [x] SessionStart priming, session dedup, PostCompact reset, error hook
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
      (32KB), dedup file locking (`fd-lock`) with FNV-1a hashing, `SECURITY.md`. See
      `docs/plans/2026-04-04-001-feat-security-hardening-plan.md`
