# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `just install` recipe for development installs via `cargo install --path .` (#5)
- Progress bar for model pull during `lore init` (#4)
- Full CLI with five commands: `init`, `ingest`, `serve`, `search`, `status` (#3)
- MCP server with four tools: `search_patterns`, `add_pattern`, `update_pattern`,
  `append_to_pattern` (#3)
- Hybrid search combining FTS5 and sqlite-vec with Reciprocal Rank Fusion (#3)
- Markdown chunking by heading with frontmatter tag extraction (#3)
- Ollama provisioning: detection, auto-start, model pull (#3)
- Git integration for write tools (auto-commit on add/update/append) (#3)
- GitHub Actions CI workflow (#2)
- Project infrastructure: clippy pedantic lints, cargo-deny, dprint formatting, git hooks (#2)
- 96 tests covering all modules

### Changed

- Rust toolchain pinned to `stable` instead of 1.85 (#5)
