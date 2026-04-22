# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **DB as sole runtime read surface** — pattern bodies now live in a new `patterns` table
  (`source_file` PK, `title`, `tags`, `is_universal`, `raw_body`, `content_hash`, `ingested_at`);
  `SessionStart` / `PostCompact` render from the DB instead of re-reading the source markdown.
  Restores the "agents consuming lore need exactly one read surface" contract that PR #33
  inadvertently broke. **Breaking for existing knowledge bases:** schema bumps to v2. After
  upgrading, run `lore ingest --force` once before your next session — `lore` will refuse to start
  otherwise with a version-agnostic advisory. `--force` is a destructive rebuild that re-embeds
  every chunk through Ollama; budget time accordingly. Sandboxed read-only agents (e.g. nono.sh) no
  longer need filesystem access to the patterns directory for the pinned-render path at session
  start. Agents that call write tools (`add_pattern` / `update_pattern` / `append_to_pattern`) still
  need patterns-directory write access because those tools write markdown to disk as the authoring
  surface. **Rollback is not safe-by-default:** the schema probe uses `>=`, so reverting this change
  against a v2 DB silently passes the probe but leaves an orphan `patterns` table on subsequent
  `clear_all` calls. Correct rollback: revert + delete `knowledge.db` + re-run `lore ingest --force`
  under v1. See `docs/architecture.md` for the codified invariant.
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
