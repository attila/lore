# Roadmap

## Up Next

- [ ] Track 2 Observability — opt-in, agent-agnostic per-hook trace logging written as JSONL records
      under `$XDG_STATE_HOME/lore/traces/<session-id>.jsonl` (one file per session), plus a
      `lore trace why <session>` query CLI. Enables data-driven decisions on threshold tuning,
      refactor validation, debug, and continuous dogfooding. Builds on `default_trace_dir()` from
      the etcetera refactor (PR #52) and the three-list RRF pipeline from language detection (PR
      #50). See `docs/brainstorms/2026-05-14-track-2-observability-requirements.md`.

## Future

- [ ] Pre-release UX polish (deferred from edge-case-handling brainstorm) — friendlier
      empty-directory copy beyond the current tier-2 warning, empty-DB search hints, and a
      structured `SlugCollisionError` type (with `existing_path` / `existing_title` fields for
      programmatic downcast) to replace the prose error currently returned by `add_pattern`. Parked
      until a concrete user report or MCP-agent retry loop justifies the additional test cost; see
      Key Decisions in `docs/brainstorms/2026-04-08-edge-case-handling-requirements.md`.
- [ ] Lossy-only HEAD-gate special-case — a single non-UTF-8 filename currently blocks
      `META_LAST_COMMIT` recording, forcing every subsequent `lore ingest` into full mode until the
      file is renamed. The loud warning is the recovery signal, but a large knowledge dir pays a
      permanent full-walk cost. A future special-case could record HEAD when the only entries on
      `result.errors` are lossy-path warnings (recoverable filesystem state, not DB-consistency
      state). Surfaced in slice D code review (REL-02); kept out of slice D scope to respect the
      brainstorm's channel-choice directive. See
      `docs/plans/2026-05-12-001-feat-lossy-path-warning-plan.md`.
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
- [ ] Predicated-universal deduplication behaviour — revisit `src/hook.rs`'s
      `r.is_universal || !seen.contains(&r.id)` filter in `dedup_filter_and_record` once Track 2
      observability data is in. Track 1B deliberately kept the bypass for predicated universals as a
      Key Technical Decision (intent: re-inject on every matching call), but ~45 KB/session of
      repeated injection for `agents/unattended-work.md` has been flagged as a real cost. One-line
      override: `(r.is_universal && r.applies_when_json.is_none()) || !seen.contains(&r.id)`.
      Deferred until observability data quantifies whether per-call reminders change agent
      behaviour; see Key Technical Decisions in
      `docs/plans/2026-05-08-001-feat-sessionstart-respect-applies-when-plan.md`.
- [ ] Code content analysis for query enrichment — extract meaningful terms from `content` /
      `new_string` fields in Edit/Write tool input to improve search relevance
- [ ] Plugin marketplace distribution (Claude Code marketplace or self-hosted)
- [ ] Additional agent integrations (Cursor, opencode) under `integrations/`
- [ ] Install on PATH without building from source (Homebrew tap or similar)
- [ ] Absolute path output in `lore init` MCP config instructions

## Completed

- [x] Extend the shared language table — added 21 new entries (Ruby, Java, C/C++, C#, PHP, Swift,
      Kotlin, Shell, Objective-C, Scala, Elixir, Dart, Lua, Nix, Terraform, Haskell, Clojure, Zig,
      Perl, Groovy) and back-filled the existing six with missing version-pin markers and lockfiles,
      including the asymmetric `package-lock.json` on TypeScript that PR #50 left out. R5 contested
      signals resolved: `.h` shared between `clang` and `cpp` (R5 multi-entry), `.m` single-owner to
      `objectivec`. See `docs/plans/2026-05-18-001-feat-language-table-expansion-plan.md`.
- [x] Language coverage in `lore status` — new `Languages:` line in the CLI status output reports
      per-language source counts (rendered via `LANGUAGES.display_name`) plus an `undeclared`
      bucket, built on the `language_json` column from #50. The same breakdown is exposed to agents
      through the MCP `lore_status` tool's metadata fence as `languages_declared` /
      `languages_undeclared` / `languages_error`. Defence-in-depth hardening: subquery dedup of
      per-source tokens, empty-array rollup into `undeclared`, shared read transaction across the
      two count queries, and unknown-token sanitisation at both ingest and render. See
      `docs/plans/2026-05-15-001-feat-language-in-status-plan.md`.
- [x] Replace hand-rolled XDG resolution with `etcetera` — `default_config_path` and
      `default_database_path` now use `etcetera::base_strategy::Xdg`, making the XDG-everywhere
      macOS posture explicit at the call site rather than implicit in hand-rolled code. Adds
      `default_trace_dir()` returning `$XDG_STATE_HOME/lore/traces` (with
      `$HOME/.local/state/lore/traces` fallback) as quiet infrastructure for the forthcoming Track 2
      Observability work. No observable Linux or macOS behaviour change for the two existing
      helpers. See `docs/plans/2026-05-14-001-refactor-etcetera-xdg-path-resolution-plan.md`.
- [x] Language detection architecture — refactored signal detection around a single shared
      declarative table (`src/engine/languages.rs`) covering extensions, command keywords, marker
      filenames, and directory hints. Word-boundary bash matcher replaces the prior substring
      contains check (no more `bundle install` matching `bun`). Added an optional `language:`
      frontmatter field that drives a structural retrieval gate via a new `language_json` column
      (schema v4, additive). Retrieval now composes three independently-ranked lists fed to RRF:
      FTS-fallback for undeclared patterns, FTS-structural for declared patterns, and
      oversample-and-filter vector. See
      `docs/plans/2026-05-14-001-feat-language-detection-architecture-plan.md`.
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
- [x] Edge case handling — Slice C (no-HEAD progress line on fresh `git init`). `ingest()` now emits
      `No commits yet — HEAD will be recorded after your first commit.` when the knowledge directory
      is a `git init` with zero commits, replacing the misleading
      `No previous ingest recorded — running full ingest` wording for that case only. The other four
      full-mode fallback wordings (non-git, prev-commit-missing, head-resolve-failed,
      git-diff-failed) are unchanged. Discrimination uses `git symbolic-ref --quiet HEAD` plus
      `git rev-parse --verify` on the target via a new `is_unborn_head` helper in `src/git.rs`, so
      other `head_commit` failure modes keep surfacing as warnings. Tier-2 per the CLI behaviour
      ladder. See `docs/plans/2026-05-11-002-feat-no-head-progress-line-plan.md`.
- [x] Edge case handling — Slice E (missing-`git` binary regression test). Integration test in
      `tests/edge_cases.rs` spawns `lore ingest` with `PATH` cleared on the child process only and
      asserts the missing-binary fallback fires the unique progress marker
      `Not a git repository —
      running full ingest` and exits 0. Codifies tier-3 silent
      fallback behaviour per the CLI behaviour ladder; no user-visible behaviour change. See
      `docs/plans/2026-05-11-001-feat-missing-git-regression-test-plan.md`.
- [x] Edge case handling — Slice D (lossy-path warning during directory walk). `walk_md_files` now
      detects `Cow::Owned` from `to_string_lossy()` on relative paths and surfaces non-UTF-8
      filenames as warnings on `IngestResult::errors` rather than indexing them under a
      U+FFFD-substituted source-file key. Wired through `discover_md_files` (full-ingest path) and
      `ReconcileStats.lossy_warnings` (delta-reconcile path). Two accounting fixes ride along:
      `discover_md_files` no longer blames `.loreignore` for lossy exclusions in its progress
      message, and `effective_scan_state` routes all-lossy directories to `FilesystemEmpty` rather
      than `AllIgnored`. R11.9's regression test plus four shadow-path tests pin the contract.
      `cfg(unix)`-gated where `OsStr::from_bytes` is required. Closes the edge-case-handling roadmap
      line entirely. See `docs/plans/2026-05-12-001-feat-lossy-path-warning-plan.md`.
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
- [x] Universal-pattern predicate (`applies_when`) and engine/adapter split (Track 1) —
      universal-tagged patterns may now carry a frontmatter `applies_when` block gating re-injection
      by tool class and Bash command prefix (OR within keys, AND across), with
      `min_relevance_universal` as a per-tier score floor under `[search]` (defaults to inherit
      `min_relevance`). Hook code is reorganised into an agent-agnostic `src/engine/` module
      operating on a `CallContext` plus a Claude-Code-specific `src/hook.rs` adapter, opening the
      door to future Cursor/opencode integrations. Ships alongside a one-way schema bump to v3 via a
      forward-compatible ALTER TABLE migration. See
      `docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`.
- [x] SessionStart pinning deferred for predicated universal patterns — a `universal`-tagged pattern
      that also carries an `applies_when` predicate is conditionally relevant, so pinning its body
      at every SessionStart contradicted that scope. Such patterns are now excluded from the
      `## Pinned conventions` block at SessionStart and from the PostCompact re-emit (shared code
      path), and re-inject on every matching `PreToolUse` call via the predicate path instead.
      Un-predicated universals are unaffected. Carries a small first-tool-call delay for predicated
      patterns. Origin is the post-Track-1 dogfood retrospective captured in
      `docs/solutions/workflow-issues/dogfood-reframes-workstream-2026-05-08.md`. See
      `docs/plans/2026-05-08-001-feat-sessionstart-respect-applies-when-plan.md`.
