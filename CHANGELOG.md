# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Universal patterns** — patterns whose `tags:` frontmatter list contains `universal` get
  always-on injection at SessionStart (full body in a `## Pinned conventions` section) and bypass
  the PreToolUse dedup filter so they re-inject on every relevant tool call. Additive beyond
  `top_k`; relevance gate intact. Closes the always-on discoverability gap for process-level
  conventions (commit messages, push discipline, branch naming) that the coverage-check skill cannot
  address. **Breaking for existing knowledge bases:** the chunks table gains an `is_universal`
  column. After upgrading, run `lore ingest --force` once before your next Claude Code session —
  `lore` will refuse to start otherwise with a friendly advisory. `--force` is a destructive rebuild
  that re-embeds every chunk through Ollama; budget time accordingly. See
  `docs/pattern-authoring-guide.md` for the new "When to use the universal tag" section.
- Add Phase 0 project infrastructure and quality gates
- Port scaffold into project skeleton (#3)
- Show progress during model pull (#4)
- Add dev install recipe (#5)

### Changed

- Add .gitignore

### Documentation

- Add foundation brainstorm
- Mark Phase 0 plan as completed
- Add getting started guide to README
- Add CONTRIBUTING.md, CHANGELOG.md, and ROADMAP.md
