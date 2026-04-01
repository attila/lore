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

## Up Next

- [ ] MCP integration testing (wire up to Claude Code, verify tool discovery and invocation)
- [ ] Edge case handling (empty knowledge dir, non-git dir, Ollama down at query time, duplicate
      titles, unicode filenames)

## Future

- [ ] Agent integration hooks (PreToolUse domain map, auto-invocable skill, PostToolUse audit, error
      hook) — see `tmp/INTEGRATION_STRATEGY.md` for design notes
- [ ] Release process (prebuilt binaries via `cargo-zigbuild`, GitHub releases)
- [ ] Install on PATH without building from source (Homebrew tap or similar)
- [ ] Absolute path output in `lore init` MCP config instructions
