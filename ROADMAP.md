# Roadmap

## Completed

- [x] Project infrastructure (CI, quality gates, formatting, lints)
- [x] Port scaffold into working Rust binary (all modules compile, 96 tests)
- [x] Progress bar during model pull
- [x] Dev install via `just install`

## Up Next

- [ ] End-to-end testing with real data (init -> ingest -> search -> add_pattern -> search finds it)
- [ ] MCP integration testing (wire up to Claude Code, verify tool discovery and invocation)
- [ ] Edge case handling (empty knowledge dir, non-git dir, Ollama down at query time, duplicate
      titles, unicode filenames)

## Future

- [ ] Agent integration hooks (PreToolUse domain map, auto-invocable skill, PostToolUse audit, error
      hook) — see `tmp/INTEGRATION_STRATEGY.md` for design notes
- [ ] Release process (prebuilt binaries via `cargo-zigbuild`, GitHub releases)
- [ ] Install on PATH without building from source (Homebrew tap or similar)
- [ ] Absolute path output in `lore init` MCP config instructions
