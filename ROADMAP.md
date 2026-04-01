# Roadmap

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

## Up Next

- [ ] Edge case handling (empty knowledge dir, non-git dir, duplicate titles, unicode filenames)

## Future

- [ ] Agent integration — layered strategy for reliable pattern delivery to agents
  - [ ] Layer 1: PreToolUse domain hook — deterministic injection of relevant patterns as
        `additionalContext` before tool execution, driven by a configurable domain map in
        `lore.toml`
  - [ ] Layer 2: Auto-invocable skill — `.claude/skills/` entry for novel situations the domain map
        doesn't cover
  - [ ] Layer 3: PostToolUse audit hook — review agent output against patterns, catch taste
        violations that produce working code
  - [ ] Layer 4: Error hook — search lore on build/test failures for known gotchas
  - [ ] `lore hooks install` command — generate agent-specific hook/skill files from the domain map
  - [ ] See `tmp/INTEGRATION_STRATEGY.md` for full design notes
- [ ] Score normalization — normalize RRF scores to 0–1 range for intuitive thresholds and display
- [ ] Release process (prebuilt binaries via `cargo-zigbuild`, GitHub releases)
- [ ] Install on PATH without building from source (Homebrew tap or similar)
- [ ] Absolute path output in `lore init` MCP config instructions
