---
title: "feat: Track 2 Observability — per-hook decision tracing"
type: feat
status: active
date: 2026-05-15
origin: docs/brainstorms/2026-05-14-track-2-observability-requirements.md
---

# feat: Track 2 Observability — per-hook decision tracing

## Summary

Add opt-in, agent-agnostic per-hook trace logging to lore. Each hook invocation writes a structured
JSONL record to a session-scoped file under `$XDG_STATE_HOME/lore/traces/`; a new nested
`lore trace { why, prune }` clap subcommand reads and maintains them. `SearchResult` gains three
optional pre-fusion score fields (FTS-fallback, FTS-structural, vector) plumbed through
`reciprocal_rank_fusion_n` and surfaced through the existing MCP `lore-metadata` fenced block. Lazy
maintenance compresses files older than seven days and prunes those older than thirty days on a
24-hour throttle. `lore status` and the MCP `lore_status` tool gain a `trace` block when tracing is
enabled. Ships alongside an institutional note documenting the env-var-plus-config-flag coexistence
policy as a reusable convention.

---

## Problem Frame

The Track 2 measurement workstream (200 sessions, 2,208 hooked tool-uses across four falsified
hypotheses) established that lore's current per-call behaviour is mostly its intended search recall
at present defaults — not a regression. Where to take the broader Track 2 work (threshold tuning,
predicate-scope refactor, validate-the-refactor passes, continuous dogfooding) depends on data lore
cannot currently produce.

`LORE_DEBUG=1` emits roughly 70% of the fields a useful trace record would contain, but as ephemeral
stderr text — useless for cross-session analytics or any after-the-fact investigation. The harness
used for the Track 2 measurement reconstructed records from Claude Code's session backups, which is
feasible for one-shot retrospectives but not for ongoing decisions.

Without persistent, structured, queryable traces, every subsequent Track 2 decision is guesswork.
(see origin: `docs/brainstorms/2026-05-14-track-2-observability-requirements.md`)

---

## Requirements

Carried forward from origin (R1-R20) plus two new requirements for the `lore status` / MCP
enrichment that this plan adopts as in-scope (R21, R22).

**Storage and location**

- R1. Hook traces are persisted as JSON Lines (one record per line), one file per session, named by
  session id, stored in the user state directory under the XDG state tier.
- R2. Each record carries an explicit `schema_version` field, enabling forward-compatible evolution.
- R3. The trace schema names events using a canonical taxonomy (PreToolUse, PostToolUse,
  SessionStart, PostCompact). Adapter-specific lifecycle event names map onto this taxonomy at the
  adapter boundary.

**Activation and precedence**

- R4. Tracing is disabled by default. Operators opt in via persistent config flag, per-session env
  var, or both. The env var takes precedence over the config when set.
- R5. When tracing is disabled, no records are written and the hook path incurs no measurable
  additional latency.

**Record contents**

- R6. Each event record captures, at minimum: timestamp, session id, canonical event name, agent
  identifier, call-context fields (tool name, command head, file path, description), extracted
  query, inferred languages (per the post-#50 language detection), candidate list, final injected
  set, search and embedder configuration snapshot, Ollama availability, total duration, and
  per-phase duration breakdown. SessionStart records carry a full configuration snapshot once per
  session; per-event records carry only the search and embedder subset (the only mid-session-tunable
  fields).
- R7. Per-candidate fields include: chunk id, source pattern path, universal-tag flag,
  predicate-presence flag, language-declaration flag, pre-fusion FTS-fallback score, pre-fusion
  FTS-structural score, pre-fusion vector score, post-fusion combined score, predicate outcome,
  threshold pass/fail, and dedup status. Preserving the three pre-fusion component scores requires a
  database-layer change; post-#50 retrieval composes the three lists via `reciprocal_rank_fusion_n`
  and discards per-list scores inside fusion.

**Privacy and redaction**

- R8. Bash command bodies are reduced to the first whitespace-delimited token by default.
  `description` and `file_path` are captured verbatim.
- R9. Operators may opt in to full Bash command capture via a dedicated config flag
  (`[trace] include_full_command`).
- R10. Operators may opt in to transcript-tail capture via a separate dedicated config flag
  (`[trace] include_transcript_tail`).

**Retention and maintenance**

- R11. Trace files are pruned by file modification time with a configurable retention horizon
  (default: thirty days). Files older than a configurable gzip horizon (default: seven days) are
  compressed in place.
- R12. Maintenance (compression then deletion) runs lazily on SessionStart, throttled to at most
  once per 24 hours, with bounded work per maintenance run (cap: 100 files compressed and 100 files
  deleted, hardcoded for v1).
- R13. Operators may invoke `lore trace prune` manually to run an unbounded maintenance pass with no
  throttle and no per-run cap.
- R14. Maintenance-throttle state (last-run timestamp) is tracked in a small state file alongside
  the trace files in the same XDG state tier.
- R15. Write failures, prune failures, and compression failures are silent in normal operation,
  preserving the existing "hook never breaks the agent" contract. Under `LORE_DEBUG=1` they surface
  to stderr alongside existing hook-error diagnostics.

**Query CLI**

- R16. The `lore trace why <session>` command reads trace files for the named session and renders a
  human-readable pretty-print to stdout. The pretty-print shows per record: timestamp, event, tool,
  command head, extracted query, a candidate table with scores and outcomes (injected rows visually
  marked), the final injection set, and total duration with per-phase breakdown.
- R17. The existing global `--json` flag causes `lore trace why` to emit raw JSONL pass-through
  instead of pretty-print.
- R18. `lore trace why --recent N` reports the most recent N records across all sessions.
  `--event <NAME>`, `--tool <NAME>`, and `--agent <NAME>` filter the output. `--agent` is
  forward-compat for future Cursor / opencode adapters sharing the trace directory; today every
  record carries `agent: "claude-code"`, so the filter is a no-op until a second adapter ships.

**Compatibility**

- R19. The trace mechanism preserves the existing "hook never breaks the agent" contract: hooks
  return on the established timeline regardless of trace write success or failure.
- R20. The MCP server (`lore serve`) is unaffected by tracing as an active write path — tracing is a
  hook-context concern. The `SearchResult` extension flows through to `search_patterns` MCP
  responses as forward-compatible enrichment (see Key Technical Decisions).

**Status surfaces (new in plan; not in origin R-IDs)**

- R21. `lore status` surfaces a `Trace:` block when tracing is enabled, reporting: trace directory
  path, file count, total bytes, oldest and newest trace file mtime, and `last_pruned_at` from the
  throttle state file. When tracing is disabled, the block is omitted.
- R22. The MCP `lore_status` tool exposes the same trace-state fields under a `trace` object key,
  matching the existing `empty_knowledge_dir` / `knowledge_dir_status` field-shape convention. The
  `trace` object includes a `capture` sub-object with `command_head_only: bool`,
  `transcript_tail_included: bool`, and a `warnings: []` array of string tokens (e.g.
  `"full_command_body_captured"`, `"transcript_tail_captured"`) that is non-empty when
  privacy-elevated toggles are on. When tracing is disabled, the `trace` key is absent.

**Acceptance Example coverage** (origin AE-IDs mapped to plan units):

- AE1 (env-var override) — U1 + U4
- AE2 (redaction toggle) — U4
- AE3 (retention + throttle) — U5
- AE4 (`--json` pass-through) — U6
- AE5 (silent failure) — U2 + U4

---

## Scope Boundaries

- Live tailing / follow-mode in the query CLI (`tail -f | jq` covers it for v1).
- Stats-aggregation commands such as `lore trace stats` or fire-rate-per-pattern (composable from
  `--json` + `jq` for v1).
- Trace pinning / immutable snapshots (`lore trace pin <session>`).
- Colour output in pretty-print (deferred to v1.1, with `NO_COLOR` env-var respect when added).
- Time-range filter flags (`--from`, `--to`) on `lore trace why` (`jq` handles it for v1).
- Background / async write path (in-process fire-and-forget for v1).
- Trace compaction (rolling multi-session JSONLs into one file) — keeps per-session granularity.
- Heuristic secret-redaction in command arguments (always-imperfect; explicit head-only redaction is
  the safer default).
- Default-on tracing.
- Track 2-B (extending the predicate mechanism to non-universal patterns) — separate workstream.
- Track 1B dedup-bypass override for predicated universals — already deferred to the
  post-observability backlog (`ROADMAP.md` Future section once the ride-along entry lands).

### Deferred to Follow-Up Work

- **Retention rule-hash for eager prune.** Per the
  `filter-changes-in-delta-pipelines-need-bidirectional-reconciliation` learning, when an operator
  shrinks `retain_days`, the next hook should prune immediately rather than wait for the 24-hour
  throttle. Detect by hashing the retention rule alongside `last_pruned_at`. Useful enhancement but
  net-new scope versus the brainstorm; the existing manual `lore trace prune` covers the
  not-immediate case adequately. Defer.
- **`lore trace stats` aggregation command** — composable via `--json` + `jq` for v1; promote when
  real-volume justifies.
- **README CLI mention** — verify whether `README.md` enumerates subcommands; add a one-line mention
  if so; otherwise defer.

---

## Context & Research

### Relevant Code and Patterns

- **Hook pipeline (PreToolUse, 8-step).** `src/hook.rs:171-308` (`handle_pre_tool_use`). The
  pipeline is: `skip_agent` → `to_call_context` → `engine::extract_query` →
  `search_with_threshold_gated` → `expand_to_siblings` → `apply_predicate_filter` →
  `dedup_filter_and_record` → `format_imperative`. Trace-write integration slots between
  `dedup_filter_and_record` (line 281 region) and `format_imperative` (line 300), so the trace
  record reflects what was actually injected. The trace call goes through a new
  `trace::record_pre_tool_use(...)` API that swallows errors silently.

- **Other hook handlers.** `src/hook.rs:128-145` (`handle_session_start`), `src/hook.rs:440-510`
  (`handle_post_tool_use`), `src/hook.rs:420-437` (`handle_post_compact`). Each gets a trace-write
  call at the natural point (after the handler's main decision, before the `Ok(Some(...))` /
  `Ok(None)` return).

- **`SearchResult` and SELECT sites.** `src/database.rs:62-86` is the struct (currently ten fields
  including `language_json: Option<String>` from PR #50). Five SELECT sites build it: `search_fts`
  (`:246`), `search_vector` (`:284`), `search_fts_fallback` (`:353`, via `read_search_row` at
  `:1279-1292`), `search_fts_structural` (`:394`, via the same helper), `chunks_by_sources`
  (`:622`). Adding three `Option<f64>` fields requires touching every SELECT's row mapping plus the
  `read_search_row` helper.

- **`reciprocal_rank_fusion_n` score-loss point.** `src/database.rs:1337-1369`. The function builds
  a `HashMap<String, (SearchResult, f64)>` keyed on `id`. At lines 1346-1348 the per-list score is
  effectively discarded — only the RRF rank contributes; at line 1365 the merged
  `SearchResult.score` is overwritten with the normalised RRF value. The pre-fusion scores still
  exist in each input list's `SearchResult.score` field at fusion entry, so the lowest-touch seam is
  to tag the per-list score onto the appropriate new field on the result before it enters the
  accumulator (see Key Technical Decisions for the seam choice).

- **MCP `lore-metadata` fenced block.** `src/server.rs` (`maybe_append_lore_metadata_fence`,
  `LORE_METADATA_FENCE_TAG`, `extract_lore_metadata_fence`). The `search_patterns` response already
  threads scores through this fence behind `include_metadata: true`. Extend the existing per-row
  shape with the three new `Option<f64>` fields; the
  `search_patterns_response_metadata_pins_hybrid_shape` test gets updated to pin the new shape.

- **`Config` and `SearchConfig` patterns.** `src/config.rs:6-17` (`Config`), `src/config.rs:30-41`
  (`SearchConfig` with `#[serde(default = "default_X")]` per-field pattern). New `TraceConfig`
  follows the `SearchConfig` always-present-with-defaults shape (R4 visibility decision) rather than
  the `GitConfig` optional-section shape.

- **`default_trace_dir`.** Already in place at `src/config.rs:125-132` via the etcetera refactor. No
  further config-path work needed.

- **CLI surface.** `src/main.rs:16-33` is the `Cli` struct with the global `--config` / `--json`
  flags. `src/main.rs:35-136` is the flat `Commands` enum. Nested clap subcommands
  (`lore trace { why, prune }`) introduce a `Trace { #[command(subcommand)] action: TraceAction }`
  variant with its own enum — first nested-subcommand precedent in this codebase. Per-command shape:
  `cmd_search` (`:539-596`) is the closest analogue for the dual pretty-print / `--json` pattern.

- **`lore status` and MCP `lore_status` shape.** `src/main.rs:729-789` (`cmd_status`). The MCP side
  surfaces `empty_knowledge_dir` and `knowledge_dir_status` per the empty-knowledge-dir-validation
  plan; new `trace` object follows the same conditional pattern.

- **Test conventions.** `tests/hook.rs` has `setup_test_env`, `setup_with_universal_pattern`,
  `invoke_session_start`, `run_pre_tool_use_sequence`, and `run_pre_tool_use_capturing_debug`
  (`:1918-1957` — the `LORE_DEBUG=1` subprocess capture pattern). `tests/smoke.rs` has
  `setup_populated_env` for CLI smoke. `FakeEmbedder` in `src/embeddings.rs` is the standard
  Ollama-free test stub.

- **File-I/O invariants.** `tests/invariants.rs:138-191` audits `src/hook.rs`, `src/main.rs`,
  `src/server.rs`, `src/database.rs`, `src/ingest.rs`. A new `src/trace/` module is unmonitored and
  trace writes there do not trip the audit. The plan elects to add `src/trace/writer.rs` and
  `src/trace/maintenance.rs` to the audit list explicitly (see Key Technical Decisions).

### Institutional Learnings

- `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` — failure-mode tiering for trace
  surfaces: tier-1 hard-fail for malformed `[trace]` config; tier-2 warn for `lore trace prune`
  permission errors and malformed lines encountered by `lore trace why`; tier-3 silent for hook-
  context write failures. Resist `--allow-empty` style silencers (see Risks).
- `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md` — split
  `lore trace why` (data → stdout) from `lore trace prune` (action → stderr summary, no stdout).
- `docs/solutions/best-practices/cli-suppress-stderr-in-json-mode-2026-04-03.md` —
  `lore trace why
  --json` suppresses all stderr courtesy text; empty session returns `[]`.
- `docs/solutions/conventions/schema-migration-strategy-2026-05-14.md` — silent-additive preference
  for the JSONL `schema_version` field; readers tolerate unknown fields; bump only when a reader
  genuinely cannot tolerate the older shape. No probe-and-advisory for trace files.
- `docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`
  — corroborates the no-probe choice; an advisory string is an API contract requiring a remedy-
  completion test.
- `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md` —
  precedent for the `last_pruned_at` state file lifecycle. State files have an explicit creator step
  and a `path.exists()` gate at read time.
- `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  — adversarial-cascade audit before merge: list every process that reads `$XDG_STATE_HOME/lore/`,
  confirm none can wipe or overwrite trace files. Dedup tmp files live in `$TMPDIR` so the namespace
  is disjoint; cascade risk is low but pinned by test.
- `docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md` — both
  lazy hook maintenance and manual `lore trace prune` bump `last_pruned_at`; pin with an integration
  test that interleaves the two writers.
- `docs/solutions/best-practices/mcp-metadata-via-fenced-content-block-2026-04-07.md` — three new
  optional fields travel inside the existing `lore-metadata` fenced block; never on
  `result.metadata` (stripped by Claude Code).
- `docs/solutions/best-practices/testing-env-var-reading-code-rust-edition-2024-2026-05-14.md` —
  `temp-env` for `LORE_TRACE` tests (Edition 2024 + `unsafe_code = "deny"` forbids
  `std::env::set_var` in tests).
- `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`
  — applies to retention-rule-change reconciliation; rule-hash eager-prune deferred to follow-up
  (Scope Boundaries).

---

## Key Technical Decisions

- **JSONL over SQLite.** Matches the dominant convention in the agent-tooling ecosystem (Claude
  Code, Codex, LSP servers, OpenTelemetry-local-emit all use JSONL). Portable via `cat` / `jq` /
  `tail -f` without `sqlite3`. Crash-safe by append-only construction. Trivially diffable for
  refactor validation. SQLite advantages accrue at scales beyond lore's expected per-user volume and
  can be added later via a `lore trace import` step if real volume demands it. (see origin)

- **XDG state tier over cache or data.** Trace files are actions-history records that cannot be
  regenerated, ruling out cache. They are not user-owned documents in the same sense as databases or
  projects, so the data tier over-promotes them. State tier matches modern XDG-conformant tools.
  `default_trace_dir()` already in place from PR #52. (see origin)

- **Trace module physical layout: `src/trace/` directory with submodules.** Scope is separable
  (record types, writer, maintenance, query) and `src/engine/` already establishes the
  directory-with-submodules precedent. Submodules: `mod.rs` (re-exports + public API), `record.rs`
  (record types, schema_version, serde), `writer.rs` (append-only JSONL writer, fire-and-forget
  error swallow), `maintenance.rs` (lazy throttled compress + prune, plus the manual prune unbounded
  variant), `query.rs` (file reader, pretty-print, JSON pass-through).

- **`[trace]` always-present in `lore.toml` with `enabled = false` default.** Mirrors the
  `SearchConfig` per-field-default precedent. Discoverable on every fresh `lore init` rather than
  hidden behind documentation. Empty `[trace]` block in the generated config makes the toggle
  visible without requiring operators to know it exists.

- **Env var precedence: `LORE_TRACE` overrides `[trace] enabled`.** Matches the established
  `LORE_DEBUG` convention symmetrically: truthy values `1` / `true` / `yes` enable; falsy values `0`
  / `false` / `no` disable; all are case-sensitive (mirrors `src/debug.rs:11-15`). Any other value,
  including the empty string, is treated as unset — `Config::trace_enabled()` silently falls through
  to `self.trace.enabled`. No malformed-value warning. Documented in the institutional note that
  lands alongside this plan.

- **Opt-in over default-on.** Default-on conflicts with the user-as-operator instinct that always-on
  tracing is obtrusive. Persistent config flag preserves the "set once and forget" property the
  "tune thresholds" use case requires; env var preserves the per-session override path. (see origin)

- **Pre-fusion scores via three `Option<f64>` fields on `SearchResult`, not a sidecar struct.**
  Extending the struct is cleaner than a sidecar — fewer call-site changes, the existing `Serialize`
  derive flows the new fields into `lore search --json` and the MCP `search_patterns` response
  naturally. Field names: `score_fts_fallback`, `score_fts_structural`, `score_vector`. `None` when
  the result did not come from the corresponding list. The seam is at the per-list call sites in
  `search_hybrid_gated` (`src/database.rs:504-523`): each list's `SearchResult` is annotated with
  its own pre-fusion score (already in `r.score` at that point) before being passed into
  `reciprocal_rank_fusion_n`. The two-list legacy `search_hybrid` (`:330-338`) gets the same
  treatment for `score_fts_fallback` + `score_vector` (no structural list in that path). **Closure
  widening:** the `and_modify` branch of the accumulator at `src/database.rs:1346` captures the
  incoming `r` and merges each per-list score field with the existing accumulator entry via
  `existing_r.score_X = existing_r.score_X.or(r.score_X)` — so a chunk appearing in both
  FTS-fallback and vector lists retains both component scores after fusion (the "tag before fusion"
  step alone is insufficient because the second-list `r` is otherwise dropped by `and_modify`).

- **Per-phase timing via widened return type, not a side-effect accumulator.**
  `search_with_threshold_gated` (`src/hook.rs:572-625`) and the legacy two-list shim
  `search_with_threshold` are widened to return `Result<(Vec<SearchResult>, Phases)>` so the
  per-phase numbers flow back to callers explicitly. The three call sites — `handle_pre_tool_use`,
  `handle_post_tool_use`, `cmd_search` — destructure the tuple; the two callers that don't care
  ignore the `Phases` component (`let (results, _) = …`). MCP `search_patterns` handler does the
  same. Rejected alternatives: threaded `&mut Phases` accumulator argument (introduces a side-effect
  parameter that contradicts the existing pure-return shape); re-time at the hook layer by inlining
  `search_with_threshold_gated`'s body (breaks the "all three callers share the same pipeline"
  guarantee documented at `hook.rs:547-548` and creates drift risk).

- **Sanitisation at presentation, not at write.** Industry-standard contextual output encoding:
  store raw bytes; escape only when crossing into a context where escape sequences would execute.
  The trace writer captures `description`, `file_path`, and `command_head` verbatim — JSON encoding
  inherently makes the on-disk file safe for plain-text readers (control characters become ``-style
  escape strings, not raw bytes). The `lore trace why` pretty-print applies `sanitize_for_log` (or
  equivalent, mirroring `src/hook.rs:685`) to those three fields before rendering to a terminal. The
  `--json` pass-through stays raw (JSON re-encoding is inherently safe for downstream consumers like
  `jq`). No length cap on `description` at write time for v1; transcript-tail and command bodies
  inherit their existing caps (see below).

- **Trace file permissions: explicit `0o600` files, `0o700` directory.** `OpenOptions` default
  permissions inherit the process umask, which is typically `0o022` (yielding `0o644` —
  world-readable). Trace files contain command heads, file paths, queries, and (opt-in) full Bash
  commands + transcript tails; world-readable defaults are unsafe on multi-user systems, shared CI
  runners, and containers with shared home volumes. Mirrors the discipline applied to
  `~/.ssh/id_rsa` and other user-private state. Implementation on Unix uses
  `std::os::unix::fs::OpenOptionsExt::mode(0o600)` for the file and `std::fs::set_permissions` with
  `Permissions::from_mode(0o700)` after `create_dir`. Windows best-effort — ACL model differs;
  document the gap.

- **`transcript_tail` capture inherits the existing 32 KB hook-side cap.** When
  `include_transcript_tail = true`, the writer records the already-bounded `cc.transcript_tail`
  (`Option<String>`) which the hook adapter populates via `last_user_message` capped at
  `TRANSCRIPT_TAIL_BYTES = 32_768` (`src/hook.rs:972`). No additional truncation at the trace
  writer; the field is already bounded by the hook's eager read. Documented in
  `docs/configuration.md` so operators can budget per-record sizes.

- **Trace directory is XDG-state-only; not exposed as a `lore.toml` field.** Mirrors the dedup-file
  precedent at `src/hook.rs:780-783` (`std::env::temp_dir()`-resolved, no Config knob): trace files
  are lore-managed session state, not user-owned content. Per-instance isolation and tests that need
  a fresh trace directory use `temp_env::with_vars` to override `XDG_STATE_HOME` for the test
  duration — the established testing-env-var pattern. Rejected adding `trace_dir: PathBuf` to
  `Config`: scope-creeps the config surface; `Config` is for user-owned content (`knowledge_dir`,
  `database`); lore-managed state should not appear there. Industry precedent: `helix-editor`
  (`~/.local/state/helix/helix.log`, no config knob), `rustup` (state at `~/.rustup`, only
  `RUSTUP_HOME` env-var override), `gh` (no log/trace dir config).

- **Trace directory layout stays flat; agent disambiguation at query time.** `--agent <NAME>` filter
  on `lore trace why` (R18) handles future multi-adapter aggregation without needing subdirectories.
  Records already carry an `agent` field; promoting it to a directory level would duplicate
  information, complicate single-session lookups (`lore trace why <session>` becomes a multi-subdir
  scan or requires the agent to be remembered), and break flat-namespace shell-pipe ergonomics.
  Migration cost from flat to subdirs (if ever needed) is bounded; reverse migration is symmetric.

- **MCP `search_patterns` enrichment is intentional, via the existing fence.** The three new
  `SearchResult` fields flow into the `lore-metadata` fenced block per the prior MCP-metadata
  learning. R20 (MCP unaffected by tracing) remains correct — tracing itself runs only in hook
  context — but the cross-cutting struct change deliberately exposes the new fields to MCP consumers
  too. Treated as additive metadata, not breaking. The
  `search_patterns_response_metadata_pins_hybrid_shape` test in `tests/server.rs` (or wherever the
  shape pin lives) gets updated.

- **Two separate redaction toggles (`include_transcript_tail`, `include_full_command`), not one
  unified flag.** The two fields have meaningfully different content (user prompt vs. tool
  argument); granular control fits the privacy model. Default-redact-to-head mirrors the existing
  `predicate suppress:` log discipline that redacts to the first command token specifically to avoid
  `gh auth login --token XXX` leakage. (see origin)

- **30-day retention default, 7-day gzip horizon, both configurable per `[trace]`.** Verified
  empirical baseline at brainstorm time: Claude Code's own session retention is ~60 days locally.
  Lore's own retention is a product decision rather than a mirror; 30 days is the tighter floor.
  Gzip horizon of 7 days balances readable-recent vs. compressed-older without compaction loss of
  per-session granularity. (see origin)

- **Lazy maintenance on SessionStart with 24-hour throttle, bounded work per run, manual escape
  hatch.** Lore is interactive-CLI-shaped, not daemon-shaped. Cron / launchd / Task Scheduler
  require platform-specific setup and produce "tool works only after user wires up scheduling"
  friction. The throttle pattern is small (~15 lines including the state file) and works correctly
  out of the box. `lore trace prune` is the manual unbounded escape hatch for operators preferring
  external orchestration. (see origin)

- **`last_pruned_at` is bumped by both writers.** Both lazy hook maintenance and explicit
  `lore trace prune` bump the timestamp in the state file. Hazard-pinned by an integration test that
  interleaves the two writers (per the out-of-band-writers learning). Rejected: hook-only ownership
  (manual prune wouldn't influence throttle, leading to surprise back-to-back lazy passes) and
  prune-only ownership (manual prune wouldn't help, since the hook would still throttle from its own
  state). (see origin)

- **Cap of 100 files per maintenance run, hardcoded for v1.** Bounded latency at SessionStart;
  worst-case unlink cost stays under ~100ms on cold disk. Configurability deferred until real
  operator friction shows up.

- **Schema versioning: forward-compatible silent-additive, no probe.** Per the
  schema-migration-strategy learning and the compatibility-advisory-reachability learning. Records
  carry `schema_version: 1`. Readers use `#[serde(default)]` on optional fields and ignore unknown
  fields. The integer is bumped only when readers cannot tolerate the older shape, which is not
  anticipated within v1.

- **`flate2` for gzip with the `miniz_oxide` backend.** Pure-Rust backend means no C linkage,
  keeping the bundled-SQLite-only C-deps footprint clean. License is MIT OR Apache-2.0, on the
  cargo-deny allowlist. `just deny` runs post-add to confirm.

- **`SystemTime` + manual RFC3339 formatting, no `time` or `chrono` crate.** Avoids the binary-size
  hit of pulling in a date/time crate. `SystemTime::now()` → `duration_since(UNIX_EPOCH)` rendered
  as `seconds.fractional_seconds` is sufficient; the trace consumer prefers a stable monotonic
  ordering over human-readable formatting, and `lore trace why` pretty-print can format on read.

- **`temp-env` as dev-dep for `LORE_TRACE` / `XDG_STATE_HOME` tests.** Edition 2024 +
  `unsafe_code = "deny"` makes `std::env::set_var` unsafe at the test level; `temp-env::with_vars`
  is the established pattern (already a precedent in the testing-env-var learning landed
  2026-05-14).

- **Institutional note lands alongside this plan.** Single-paragraph convention doc at
  `docs/solutions/conventions/env-var-plus-config-flag-coexistence-2026-05-15.md` (or similar
  date-stamped name following the conventions pattern). Documents the precedence rule (env var wins)
  so future lore toggles inherit it. Counts as part of this PR's Success Criteria.

- **Trace writer module is added to the `tests/invariants.rs` file-I/O audit list.** Even though the
  new module is unmonitored by the existing audit, adding it explicitly preserves the invariant's
  discipline: file-I/O sites in lore source are auditable from a single test. Count budgets:
  `src/trace/writer.rs` (write-side) and `src/trace/maintenance.rs` (read + write + unlink) get
  their expected counts pinned. `src/trace/query.rs` (read-only) joins the conditional reads.

- **Architecture-doc carve-out is updated to name trace files explicitly.** `docs/architecture.md`'s
  session-local-state carve-out gains a third bullet for trace files, keeping the sole-read-surface
  invariant honest about what does and doesn't count as indexed content.

---

## Open Questions

### Resolved During Planning

- **Trace module physical layout** — directory with submodules (see Key Technical Decisions).
- **`[trace]` section visibility in generated `lore.toml`** — always-present (see Key Technical
  Decisions).
- **PR shape** — single monolithic PR, mirroring Track 1's delivery shape.
- **`lore status` / MCP `lore_status` enrichment scope** — included in this PR (R21, R22) for
  agent-native parity and continuous-dogfooding ergonomics. The `lore status` Trace block also
  includes a `Capture:` line and the MCP `trace` object includes a `capture` sub-object with a
  `warnings` array (audit posture for privacy-elevated toggles).
- **MCP enrichment transport** — extend the existing `lore-metadata` fenced block, not
  `result.metadata`.
- **`last_pruned_at` writer ownership** — both writers bump it.
- **Time representation** — `SystemTime` + manual RFC3339, no time/chrono crate.
- **gzip crate** — `flate2` with `miniz_oxide` backend.
- **Env-var test pattern** — `temp-env`.
- **`LORE_TRACE` parsing** — symmetric with `LORE_DEBUG`: truthy `1` / `true` / `yes`, falsy `0` /
  `false` / `no`, case-sensitive, silent fall-through on anything else.
- **Per-phase timing plumbing** — widened return type on `search_with_threshold_gated`
  (`Result<(Vec<SearchResult>, Phases)>`); callers destructure or ignore the `Phases` component.
- **Pre-fusion-score closure widening** — `and_modify` captures the incoming `r` and OR-combines
  each per-list score field into the accumulator entry, not just the existing entry's score.
- **Sanitisation timing** — at presentation (`lore trace why` pretty-print), not at write or
  `--json` pass-through. Industry-standard contextual-output-encoding.
- **Trace file permissions** — `0o600` files, `0o700` directory at creation; mirrors `~/.ssh/id_rsa`
  discipline. Unix-specific via `OpenOptionsExt::mode` and `set_permissions`; Windows best-effort.
- **`transcript_tail` size** — inherits the existing 32 KB cap from the hook's
  `TRANSCRIPT_TAIL_BYTES` constant; no new knob.
- **Trace directory configurability** — XDG-state-only; mirrors dedup-file pattern at
  `src/hook.rs:780-783`. Tests use `temp_env::with_vars` to override `XDG_STATE_HOME`.
- **Multi-adapter discrimination** — flat trace directory + `--agent` query filter; no
  subdirectories by agent.

### Deferred to Implementation

- Exact field names inside the trace record JSON shape (e.g., `event` vs `event_name`, `tool` vs
  `tool_name`) — implementer chooses for consistency with serde-rename conventions already in the
  codebase.
- Exact pretty-print column ordering and ASCII markers in `lore trace why` output — refined during
  implementation by reading actual output.
- Whether `Trace { action: TraceAction }` clap variant or two flat `TraceWhy` / `TracePrune`
  variants reads cleaner once written — both legal, both pass the convention. Default: nested.
- The exact filename of the institutional note in `docs/solutions/conventions/` — proposed
  `env-var-plus-config-flag-coexistence-2026-05-15.md`, finalised at write time.

---

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

### Module boundary after Track 2 Observability

```
┌─────────────────────────────────────────────────────────────────┐
│ src/hook.rs (Claude Code adapter, unchanged audit invariants)   │
│  - handle_pre_tool_use / handle_post_tool_use /                 │
│    handle_session_start / handle_post_compact                   │
│  - existing dedup, predicate, search pipeline                   │
│  - NEW: calls trace::record_*(...) at the natural slot          │
│    (after the handler's decision, before return).               │
│    Errors are swallowed inside the trace module.                │
└──────────────────┬──────────────────────────────────────────────┘
                   │ depends on
                   ▼
┌─────────────────────────────────────────────────────────────────┐
│ src/trace/ (new module)                                         │
│  - mod.rs:        public API + TraceConfig surface              │
│  - record.rs:     TraceRecord + Candidate + Phases + serde     │
│                   schema_version = 1 (forward-compatible)       │
│  - writer.rs:     append-only JSONL writer; fire-and-forget     │
│                   error swallow; LORE_DEBUG visibility          │
│  - maintenance.rs: lazy 24h-throttled compress + prune;         │
│                   per-run caps; `last_pruned_at` state file;    │
│                   unbounded manual variant for `lore trace prune`│
│  - query.rs:      file reader with .gz transparency;            │
│                   pretty-print + raw JSONL pass-through         │
│                   filter helpers (--event, --tool, --recent)    │
└──────────────────┬──────────────────────────────────────────────┘
                   │ uses
                   ▼
┌─────────────────────────────────────────────────────────────────┐
│ src/config.rs    — TraceConfig type, `[trace]` section          │
│                    serde defaults; default_trace_dir exists     │
└─────────────────────────────────────────────────────────────────┘

src/database.rs — SearchResult gains score_fts_fallback /
                  score_fts_structural / score_vector fields.
                  reciprocal_rank_fusion_n and call sites in
                  search_hybrid_gated / search_hybrid tag the
                  per-list scores before fusion overwrites .score.

src/server.rs   — search_patterns response's lore-metadata fence
                  surfaces the three new fields; lore_status MCP
                  tool gains a conditional `trace` object.

src/main.rs     — Cli adds Trace { action: TraceAction } subcommand
                  with Why + Prune variants. cmd_status augmented
                  with TraceStats block.
```

### Per-record JSON shape (sketch)

```jsonc
{
  "schema_version": 1,
  "ts": "2026-05-15T14:23:01.234Z",
  "session_id": "abc-123-...",
  "event": "PreToolUse",
  "agent": "claude-code",
  "call_context": {
    "tool_name": "Bash",
    "command_head": "git",
    "file_path": "src/lib.rs",
    "description": "push to remote",
    "inferred_languages": ["rust"],
  },
  "query": "rust AND (git OR push)",
  "candidates": [
    {
      "chunk_id": "...",
      "source_file": "workflows/git-branch-pr.md",
      "is_universal": true,
      "has_predicate": true,
      "has_language_declaration": false,
      "score_fts_fallback": 0.65,
      "score_fts_structural": null,
      "score_vector": 0.82,
      "score_combined": 0.78,
      "predicate_outcome": "matched",
      "above_threshold": true,
      "deduped": false,
    },
  ],
  "injected": ["chunk_id_1", "chunk_id_2"],
  "config": {
    "hybrid": true,
    "top_k": 5,
    "min_relevance": 0.6,
    "min_relevance_universal": 0.6,
    "embedder_model": "nomic-embed-text",
  },
  "ollama": { "embedding_succeeded": true, "embedding_dims": 768 },
  "duration_ms": 47,
  "phases": {
    "query_extract_ms": 1,
    "search_fts_ms": 3,
    "search_vector_ms": 28,
    "embedding_ms": 12,
    "predicate_filter_ms": 1,
    "dedup_ms": 2,
  },
}
```

SessionStart records carry a fuller `config` snapshot (all sections); PreToolUse / PostToolUse /
PostCompact carry only `[search]` + `embedder.model`.

### `lore status` Trace block (sketch)

When `[trace] include_full_command = false` and `include_transcript_tail = false` (default posture):

```
Trace:
  Directory:   ~/.local/state/lore/traces/
  Sessions:    127
  Total:       3.4 MB
  Oldest:      2026-04-15  (29 days old)
  Newest:      2026-05-15  (today)
  Last pruned: 2026-05-15T03:14:22Z  (5h ago)
  Capture:     default redaction (command head only, transcript tail off)
```

When either privacy-elevated toggle is on:

```
Trace:
  Directory:   ~/.local/state/lore/traces/
  Sessions:    127
  Total:       3.4 MB
  Oldest:      2026-04-15
  Newest:      2026-05-15
  Last pruned: 2026-05-15T03:14:22Z
  Capture:     FULL COMMAND BODY  (privacy-sensitive — [trace] include_full_command = true)
               TRANSCRIPT TAIL     (privacy-sensitive — [trace] include_transcript_tail = true)
```

Block omitted entirely when tracing is disabled.

### MCP `lore_status` `trace` object (sketch)

```jsonc
{
  ...,
  "trace": {
    "directory": "/home/operator/.local/state/lore/traces/",
    "session_count": 127,
    "total_bytes": 3567104,
    "oldest": "2026-04-15T08:12:00Z",
    "newest": "2026-05-15T14:23:01Z",
    "last_pruned_at": "2026-05-15T03:14:22Z",
    "capture": {
      "command_head_only": true,
      "transcript_tail_included": false,
      "warnings": []   // non-empty when toggles are on, e.g. ["full_command_body_captured"]
    }
  }
}
```

Key omitted entirely when tracing is disabled (matches the existing
`empty_knowledge_dir`-conditional pattern).

---

## Implementation Units

### U1. `[trace]` config section + `LORE_TRACE` env-var precedence + institutional note

**Goal:** Add `TraceConfig` to `Config`, wire the `LORE_TRACE` env-var override with documented
precedence, and ship the one-paragraph institutional note documenting the policy for future toggles.

**Requirements:** R4, R5, R22 (the activation contract that everything downstream relies on); also
the brainstorm's Success Criteria for the institutional note.

**Dependencies:** None — `default_trace_dir` already exists.

**Files:**

- Modify: `src/config.rs` (add `TraceConfig`; add `Config::trace_enabled()` accessor that respects
  `LORE_TRACE` env-var override).
- Modify: `src/config.rs` `#[cfg(test)] mod tests` (round-trip + accessor + env-var-override tests
  via `temp-env`).
- Create: `docs/solutions/conventions/env-var-plus-config-flag-coexistence-2026-05-15.md` (the
  institutional note).
- Confirm `Cargo.toml` already declares `temp-env = "0.3.6"` under `[dev-dependencies]` (landed
  alongside the etcetera refactor for the `default_trace_dir` tests); no add needed.

**Approach:**

- `TraceConfig` follows the `SearchConfig` per-field-default pattern. Fields: `enabled: bool`
  (default `false`), `retain_days: u32` (default 30), `gzip_older_than_days: u32` (default 7),
  `include_full_command: bool` (default `false`), `include_transcript_tail: bool` (default `false`).
  `Config` adds `pub trace: TraceConfig` with `#[serde(default)]`.
- `Config::trace_enabled()` accessor reads `LORE_TRACE` env var first, mirroring `LORE_DEBUG`'s
  shape (`src/debug.rs:11-15`) symmetrically. Truthy values `1` / `true` / `yes` (case-sensitive)
  return `true`; falsy values `0` / `false` / `no` (case-sensitive) return `false`. Anything else,
  including the empty string, is treated as unset and falls through silently to
  `self.trace.enabled`. No malformed-value warning — the convention is to fail-soft and rely on the
  operator getting the syntax right (matches `LORE_DEBUG`'s silent ignore behaviour).
- The institutional note documents: when a toggle benefits from both persistent state and
  per-session override, expose both; env var wins; document `LORE_X=0` as the per-session force-off
  form as well.

**Patterns to follow:**

- `SearchConfig` with `default_min_relevance` named functions (`src/config.rs:30-41, 51-53`).
- `min_relevance_universal: Option<f64>` accessor pattern at `src/config.rs:42-48`.
- `temp-env::with_vars` test pattern per the
  `testing-env-var-reading-code-rust-edition-2024-2026-05-14.md` learning.

**Test scenarios:**

- Happy path: `[trace] enabled = true` in TOML round-trips correctly through `Config::load` and
  `Config::save`.
- Happy path: `[trace] enabled = false` with all defaults round-trips (no field bloat in the saved
  TOML).
- Edge case: `[trace]` section absent in TOML deserialises to defaults (`enabled = false`).
- Edge case: `[trace]` section present but empty deserialises to defaults.
- Edge case: unknown field in `[trace]` is rejected (serde strict-mode behaviour mirrors
  `SearchConfig`) — adjust expectation here if existing convention is lenient.
- **Covers AE1 (env-var override).** Given `[trace] enabled = false` and `LORE_TRACE` unset,
  `trace_enabled()` returns `false`. Given `[trace] enabled = false` and `LORE_TRACE=1`,
  `trace_enabled()` returns `true`. Given `[trace] enabled = true` and `LORE_TRACE=0`,
  `trace_enabled()` returns `false`.
- Edge case: unrecognised `LORE_TRACE` value (e.g. `LORE_TRACE=maybe`, `LORE_TRACE=TRUE`,
  `LORE_TRACE=`) silently falls through to `self.trace.enabled`. No stderr warning, no debug log —
  matches `LORE_DEBUG`'s fail-soft behaviour. Verified with `temp_env::with_vars` covering each
  unrecognised form.

**Verification:** `just ci` passes. `cargo test -p lore --lib config` covers the env-var matrix. The
institutional note is dprint-clean and reads as standalone guidance.

---

### U2. `src/trace/` module skeleton + record types + JSONL writer

**Goal:** Establish the new `src/trace/` module with record types, append-only JSONL writer,
fire-and-forget error swallow, and `LORE_DEBUG`-gated diagnostics.

**Requirements:** R1, R2, R3, R6 (per-event record shape, schema versioning), R15 (silent on
failure), R19 (hook contract preserved).

**Dependencies:** U1.

**Files:**

- Create: `src/trace/mod.rs` (public API surface, `TraceRecord` re-export, top-level
  `record_pre_tool_use` / `record_post_tool_use` / `record_session_start` / `record_post_compact`
  fire-and-forget entry points).
- Create: `src/trace/record.rs` (`TraceRecord` enum or struct, `CandidateRecord`, `Phases`,
  `OllamaState`, `ConfigSnapshot`; `schema_version` constant; serde derives).
- Create: `src/trace/writer.rs` (append-only JSONL writer with `OpenOptions::append`; per-call
  open-write-close to keep error surface contained; `LORE_DEBUG`-gated stderr on failure).
- Modify: `src/lib.rs` (add `pub mod trace;`).
- Modify: `tests/invariants.rs` (add `src/trace/writer.rs` to the audit list with the expected
  `OpenOptions` count).

**Approach:**

- `TraceRecord` is a tagged enum keyed on canonical event names, with per-variant struct payloads.
  Schema-version field at the top level. The enum is `#[serde(tag = "event")]` so the JSONL line
  format matches the sketch.
- Writer opens the trace file with
  `OpenOptions::new().create(true).append(true).mode(0o600).open(...)` (Unix-specific
  `OpenOptionsExt::mode`), writes one JSON line plus newline, flushes, closes. No long-lived file
  handle. fd_lock not required for append-only writes at this scale (POSIX guarantees atomicity for
  writes under PIPE_BUF; JSON lines for hook events are well under 64KB; if a future shape exceeds,
  revisit). On Windows the mode is a no-op; document the gap.
- The traces/ directory is created with `std::fs::create_dir_all` followed by
  `std::fs::set_permissions(dir, Permissions::from_mode(0o700))` on Unix. Same Windows caveat.
- The fire-and-forget API surface is `pub fn record_<event>(...) -> ();` — no `Result`. Errors are
  logged via `lore_debug!` only.
- The session-id determines the trace file path: `<default_trace_dir>/<session-id>.jsonl`. Missing
  session id → no write (matches the existing dedup-file handling).
- **Verbatim capture is by design.** The writer stores `description`, `file_path`, and the full Bash
  command body (when `include_full_command = true`) verbatim. JSON encoding makes the on-disk file
  safe for plain-text readers (control characters become ``-style escape strings). Sanitisation is
  a presentation concern handled by `lore trace why` pretty-print (U6), not by the writer.
- **`transcript_tail` capture inherits the existing 32 KB cap.** When
  `include_transcript_tail = true`, the writer records `cc.transcript_tail` which the hook adapter
  has already bounded via `TRANSCRIPT_TAIL_BYTES = 32_768` at `src/hook.rs:972`. No additional
  truncation in the writer.

**Patterns to follow:**

- `src/engine/` module layout for the directory-with-submodules shape.
- `lore_debug!` macro from `src/debug.rs`.
- `dedup_filter_and_record` (`src/hook.rs:826-877`) for the open-write-close discipline and the
  graceful fallback pattern.

**Test scenarios:**

- Happy path: `record_pre_tool_use` with a fully-populated record writes one valid JSON line to the
  expected path. File exists, contains valid JSON parseable back into `TraceRecord`.
- Happy path: two successive `record_*` calls produce two valid JSON lines, in order.
- Edge case: `record_*` with `session_id = None` is a no-op (no file created, no error).
- Edge case: missing parent directory (`$XDG_STATE_HOME/lore/traces/` doesn't exist yet) — the
  writer creates the directory tree, or silently skips if the create fails. Pin the chosen behaviour
  with a test.
- Error path: parent directory unwriteable (mode 0o555) — write fails silently, `LORE_DEBUG=1` emits
  a single diagnostic line on stderr. **Covers AE5 (silent failure).**
- **File permissions:** trace files are created with mode `0o600` on Unix
  (`metadata().permissions().mode() & 0o777 == 0o600`). The `traces/` directory is created with mode
  `0o700`. `cfg(unix)`-gated test.
- Schema-version round-trip: a record written with `schema_version = 1` parses back identically;
  adding a future unknown field to the JSON line still parses (forward-compatible reader).
- Edge case: extremely long `command_head` truncates to the documented cap (e.g., 60 bytes,
  mirroring `PREDICATE_LOG_CMD_HEAD_BYTES` in `src/hook.rs`).
- Verbatim capture: a record with `description` containing ANSI escape codes (e.g. `"\x1b[2J fake"`)
  and `file_path` containing path-traversal-like content (e.g. `"../../../etc/passwd"`) is written
  to disk as a JSON-encoded literal (`[2J fake`) — parsing the JSON back round-trips the original
  byte sequence. No write-time mutation. (Sanitisation on read is exercised in U6.)
- Invariant test: `src/trace/writer.rs` file-I/O count matches the expected number in
  `tests/invariants.rs`.

**Verification:** `just ci` passes. Unit tests confirm the writer's invariants. No hook integration
yet — that lands in U4.

---

### U3. `SearchResult` extension for pre-fusion scores + RRF plumbing + MCP fence enrichment

**Goal:** Extend `SearchResult` with three optional pre-fusion score fields; preserve per-list
scores through `reciprocal_rank_fusion_n`; surface the new fields via the existing `search_patterns`
MCP `lore-metadata` fence.

**Requirements:** R7 (per-candidate pre-fusion scores), R20 (MCP enrichment as forward-compatible
side effect of the struct change).

**Dependencies:** None (parallel to U2).

**Files:**

- Modify: `src/database.rs`:
  - `SearchResult` (`:62-86`): add `pub score_fts_fallback: Option<f64>`,
    `pub score_fts_structural: Option<f64>`, `pub score_vector: Option<f64>`.
  - All five SELECT sites (`search_fts:246`, `search_vector:284`, `search_fts_fallback:353`,
    `search_fts_structural:394`, `chunks_by_sources:622`): default the new fields to `None` in row
    mapping (they're populated post-SELECT at the search-hybrid call sites).
  - `read_search_row` helper (`:1279-1292`): same `None` defaults.
  - `search_hybrid` (`:330-338`) and `search_hybrid_gated` (`:504-523`): per-list, before passing
    into `reciprocal_rank_fusion_n`, populate the appropriate score field on each `SearchResult`
    based on which list it came from.
  - `reciprocal_rank_fusion_n` (`:1337-1369`): when merging duplicate ids from multiple lists,
    OR-combine the per-list score fields (an id appearing in both FTS-fallback and vector lists
    keeps both component scores).
- Modify: `src/database.rs` `#[cfg(test)] mod tests`: extend existing RRF tests + add new pre-fusion
  preservation tests.
- Modify: `src/server.rs` `maybe_append_lore_metadata_fence` per-row JSON shape: include the three
  new fields conditionally (omit when all are `None`, include when any is `Some`).
- Modify: `src/server.rs` in-source `#[cfg(test)] mod tests` block, where
  `search_patterns_response_metadata_pins_hybrid_shape` and its siblings
  (`search_patterns_response_metadata_empty_results`,
  `search_patterns_response_metadata_fts_only_when_hybrid_disabled`) pin the metadata shape: update
  each to assert on the new three-field schema.

**Approach:**

- The per-list score is already in each input list's `SearchResult.score` field at fusion entry.
  Before the result enters the accumulator, clone its current `.score` into the appropriate new
  field on the row (e.g. `r.score_fts_fallback = Some(r.score)` when iterating the `fts_fallback`
  list). The accumulator's `or_insert_with` branch carries the tagged row into the entry unchanged.
- **Closure widening for cross-list collisions.** The `and_modify` closure must capture the incoming
  `r` and OR-combine each per-list score field with the existing accumulator entry's fields:
  `existing_r.score_fts_fallback = existing_r.score_fts_fallback.or(r.score_fts_fallback)`, same for
  `score_fts_structural` and `score_vector`. Without this widening, a chunk that appears in both
  FTS-fallback and vector lists would only retain the field from whichever list saw it first; the
  second-list `r` would otherwise be dropped by `and_modify`'s current shape.
- The two-list legacy `search_hybrid` populates `score_fts_fallback` + `score_vector` only (no
  structural list in that path); `search_hybrid_gated` populates all three when all three lists
  return results.
- The MCP fence shape is conditional: if any of the three new fields is `Some`, emit them; if all
  three are `None` (FTS-only path with no embedding), omit them entirely to keep the response shape
  minimal for callers that don't care.

**Patterns to follow:**

- `is_universal` and `applies_when_json` plumbing (`src/database.rs:62-86`, all SELECT sites) as the
  column-addition template.
- Existing `reciprocal_rank_fusion_n` (`:1337-1369`) accumulator structure.
- `maybe_append_lore_metadata_fence` / `extract_lore_metadata_fence` per the
  `mcp-metadata-via-fenced-content-block-2026-04-07.md` learning.

**Test scenarios:**

- Happy path: three-list `search_hybrid_gated` populates all three score fields on returned results;
  the post-RRF `.score` is the normalised combined value.
- Happy path: two-list `search_hybrid` populates `score_fts_fallback` + `score_vector`; the
  structural field is `None`.
- Happy path: FTS-only fallback path (no embedding) populates `score_fts_fallback` only; vector and
  structural are `None`.
- Edge case: chunk appearing in both FTS-fallback and vector lists keeps both component scores after
  fusion. Specifically: the chunk's `SearchResult` after fusion has BOTH
  `score_fts_fallback = Some(<bm25-value>)` AND `score_vector = Some(<distance-value>)`. Pins the
  closure-widening behaviour in `reciprocal_rank_fusion_n`.
- Edge case: chunk appearing in all three lists (FTS-fallback, FTS-structural, vector) keeps all
  three component scores after fusion.
- Edge case: chunk appearing only in structural list keeps `score_fts_structural` only.
- Edge case: post-RRF sort order is unchanged by the new field additions (regression pin on existing
  RRF tests).
- Integration: MCP `search_patterns` with `include_metadata: true` emits the new fields inside the
  `lore-metadata` fence when present; the existing shape pin test gets updated.
- Integration: MCP `search_patterns` with `include_metadata: false` (default) does not emit the
  fence at all — no behavioural change.
- Edge case: the `chunks_by_sources` helper (used by sibling expansion) returns rows with all three
  new fields as `None` (it doesn't run scoring), which is the documented expectation.

**Verification:** `just ci` passes. Pre-existing RRF tests continue to pass. New tests pin the
component-score preservation contract. MCP response shape test re-pins the new fields.

---

### U4. Hook integration — wire trace writes into the four canonical events

**Goal:** Call `trace::record_*(...)` at the natural slot inside each of the four hook event
handlers in `src/hook.rs`, gated by `Config::trace_enabled()`. Each call is fire-and-forget; the
hook's existing semantics are preserved.

**Requirements:** R3 (canonical event taxonomy), R4 (opt-in gate), R6 (record contents), R19 (hook
contract preserved).

**Dependencies:** U1, U2, U3.

**Files:**

- Modify: `src/hook.rs`:
  - `handle_pre_tool_use` (`:171-308`): after `dedup_filter_and_record` and before
    `format_imperative`, build the record from the data already at hand and call
    `trace::record_pre_tool_use(...)`. Gate behind `Config::trace_enabled()`.
  - `handle_post_tool_use` (`:440-510`): after the search runs and before the response is formatted,
    call `trace::record_post_tool_use(...)`. Same gate.
  - `handle_session_start` (`:128-145`): after `format_session_context`, before returning, call
    `trace::record_session_start(...)`. Same gate.
  - `handle_post_compact` (`:420-437`): symmetric to session-start, call
    `trace::record_post_compact(...)`. Same gate.
  - Update the `handle_pre_tool_use` docstring (`:148-170`) to bump the documented eight-step
    pipeline to nine steps, adding the trace-write slot between dedup and `format_imperative`.
- Modify: `tests/hook.rs`: integration tests for each event's trace-write outcome.

**Approach:**

- Each handler's existing variables (CallContext, query, seeds, expanded, after_predicate, combined,
  dedup_path, duration timers) are already present at the trace-write slot. The record is built from
  those values. The `trace::record_*` API takes a `&Config` + an event-specific payload struct; no
  `Result` returned.
- **Per-phase timing flows back via a widened return type.** `search_with_threshold_gated` changes
  from `Result<Vec<SearchResult>>` to `Result<(Vec<SearchResult>, Phases)>`, where `Phases` is a
  struct of `Option<u64>` ms-counts: `query_extract_ms`, `search_fts_ms`, `search_vector_ms`,
  `embedding_ms`, `predicate_filter_ms`, `dedup_ms`. The function populates each field at the
  natural phase boundary using `std::time::Instant`. The legacy two-list shim
  `search_with_threshold` widens identically. Three current callers (`handle_pre_tool_use`,
  `handle_post_tool_use`, `cmd_search`) update to destructure the tuple; `cmd_search` and
  `handle_post_tool_use` ignore the `Phases` component with `let (results, _) = …`. The MCP
  `search_patterns` handler does the same. Rejected alternatives are recorded in Key Technical
  Decisions.
- Total duration is `start.elapsed()` at the trace-write site (measured at hook scope, not search
  scope).
- `Config::trace_enabled()` early-returns silently when tracing is off; the cost is one boolean
  check.
- `Config::trace_enabled()` early-returns silently when tracing is off; the cost is one boolean
  check.

**Patterns to follow:**

- Existing `eprintln!` + `lore_debug!` pair at hook.rs error sites (`:137`, `:273`, `:429`).
- `dedup_filter_and_record` (`:826-877`) for the "open-write-close, swallow errors" discipline the
  trace writer mirrors.
- `handle_pre_tool_use`'s eight-step docstring (`:148-170`) as the in-source documentation pattern;
  this unit adds step 9 (trace-write) to the docstring.

**Test scenarios:**

- Happy path: PreToolUse with tracing enabled writes one JSON line to the session's trace file
  containing all R6 fields including the per-phase breakdown.
- Happy path: SessionStart with tracing enabled writes one record with the full config snapshot.
- Happy path: PostCompact with tracing enabled writes one record; the existing dedup-truncation
  side-effect is unaffected.
- Happy path: PostToolUse with tracing enabled writes one record when there's a Bash error; no
  record on success (matches the existing skip-on-success semantics).
- **Covers AE1 (env-var override).** With `[trace] enabled = true` and `LORE_TRACE=0`, hook events
  produce no trace records.
- **Covers AE2 (redaction toggle).** Given `[trace] include_full_command = false` (default) and a
  Bash hook with command `git push origin main`, the trace record's `command_head` field is `"git"`
  and the full command body is absent. Given `[trace] include_full_command = true` and the same
  input, the record contains the full command body.
- **Covers R10 (transcript-tail toggle).** Given `[trace] include_transcript_tail = false`
  (default), the trace record contains no `transcript_tail` field. Given
  `include_transcript_tail = true`, the field is present and contains the eager transcript-tail read
  from `to_call_context`, bounded by the existing 32 KB `TRANSCRIPT_TAIL_BYTES` cap.
- **Covers AE5 (silent failure under read-only trace dir).** With tracing enabled and the trace
  directory read-only, the hook still returns its normal additionalContext payload to the agent.
  Stderr is silent unless `LORE_DEBUG=1` is set.
- Integration: `LORE_DEBUG=1` subprocess test confirms a `lore trace:` diagnostic line on stderr
  when the trace write fails.
- Integration: the trace record's `injected` set matches the `combined` final vector that drives
  `format_imperative`'s output (cross-check between the trace and the actual injection).
- Regression: existing hook tests pass unchanged when tracing is disabled (no behavioural change).

**Verification:** `just ci` passes. `tests/hook.rs` integration tests confirm the trace shape and
the hook-contract preservation. Manual dogfooding: a fresh session in this repo with `LORE_TRACE=1`
produces well-formed JSONL files under `~/.local/state/lore/traces/`.

---

### U5. Lazy maintenance (throttle, retention, gzip) + manual `lore trace prune`

**Goal:** Add the maintenance pipeline (compress then prune) that runs lazily on SessionStart with a
24-hour throttle and bounded work caps; expose a `lore trace prune` subcommand for unbounded manual
runs.

**Requirements:** R11, R12, R13, R14.

**Dependencies:** U1, U2.

**Files:**

- Create: `src/trace/maintenance.rs` (lazy + unbounded variants, throttle state file management).
- Modify: `src/trace/mod.rs` (re-export the public maintenance API).
- Modify: `src/hook.rs` `handle_session_start` (call lazy maintenance after the SessionStart trace
  write).
- Modify: `src/main.rs` (add `lore trace prune` clap subcommand and `cmd_trace_prune` handler).
- Modify: `tests/invariants.rs` (add `src/trace/maintenance.rs` to the audit list).
- Modify: `tests/smoke.rs` (CLI smoke test for `lore trace prune`).

**Approach:**

- The throttle state file at `<trace_dir>/.last_pruned_at` contains a single RFC3339 timestamp. Lazy
  maintenance reads it; if less than 24h ago, no-op. Otherwise compress then prune (capped at 100
  each), then update the file. The manual `lore trace prune` skips the throttle check, runs
  unbounded, and also bumps the timestamp.
- Compression: gzip files older than `gzip_older_than_days` to `<filename>.jsonl.gz`. Use
  `flate2::GzEncoder` with default compression. Skip files that are already gzipped.
- Pruning: unlink files (gzipped or not) older than `retain_days`. The retention check is on the
  source file's `mtime`, not the gzip operation time.
- Stderr discipline: `lore trace prune` emits progress to stderr per the cli-data-stdout convention;
  stdout stays empty (it's an action command). Lazy maintenance is silent (no stderr unless
  `LORE_DEBUG=1`).
- Failure handling: permission errors on a per-file basis warn and continue (tier-2 per the CLI
  behaviour ladder); fail-fast on directory-level errors only when the operation is impossible.

**Patterns to follow:**

- `dedup_file_path` (`src/hook.rs:780-783`) for the state-file naming and FNV-1a hashing if needed.
- `reset_dedup` (`src/hook.rs:813-824`) for the create-or-truncate-with-lock pattern (the throttle
  state file does not need fd_lock — single-writer SessionStart and single-writer manual prune are
  mutually exclusive in practice; if locking later becomes needed, add it).
- `cmd_ingest`'s stderr-progress pattern (`src/main.rs:317-384`) for `lore trace prune`'s
  user-facing output.

**Test scenarios:**

- Happy path: lazy maintenance with `last_pruned_at` >24h ago and trace files spanning >30 days
  compresses files in the 7-30 day window and deletes files >30 days.
- Happy path: lazy maintenance with `last_pruned_at` <24h ago is a no-op (no files touched).
- Edge case: trace files exist but no `last_pruned_at` file (first lazy run) — runs maintenance,
  creates the file.
- Edge case: `last_pruned_at` file exists but is malformed — log via `LORE_DEBUG`, treat as if
  unset, run maintenance, overwrite the file.
- Edge case: cap of 100 honoured. Given 200 stale files, one lazy run compresses/deletes 100; the
  next run (24h later, after the throttle elapses) handles the rest.
- **Covers AE3.** Given trace files exist spanning 90 days and `retain_days = 30`, when SessionStart
  fires and the throttle permits, files older than 30 days are deleted (capped at 100) and files
  older than 7 days are gzipped (capped at 100). Given `last_pruned_at` is <24h ago, when
  SessionStart fires, maintenance is skipped.
- Edge case: `lore trace prune` runs unbounded even when `last_pruned_at` is recent, deletes/
  compresses all eligible files in one pass, bumps `last_pruned_at` to now.
- Hazard pin (composition cascade): manual `lore trace prune` followed by an immediate SessionStart
  finds nothing to do (no second prune in 24h), respecting the shared `last_pruned_at` checkpoint.
- Error path: permission-denied on one file warns to stderr and continues with the remaining files;
  exit code 0 from `lore trace prune` (tier-2).
- Error path: trace directory does not exist at all — `lore trace prune` warns and exits 0; lazy
  maintenance silently no-ops.
- Integration: `lore trace prune` stdout is empty under all paths (action command).

**Verification:** `just ci` passes. Smoke test confirms manual prune semantics. Integration with the
hook's SessionStart confirms the lazy + manual writers share the throttle state correctly.

---

### U6. `lore trace why` query CLI

**Goal:** Add the nested `lore trace why <session>` subcommand that reads trace files, applies
filters, and emits either pretty-print (default) or raw JSONL pass-through (`--json`).

**Requirements:** R16, R17, R18.

**Dependencies:** U2 (the writer's output is the reader's input).

**Files:**

- Create: `src/trace/query.rs` (file reader with `.gz` transparency, filter helpers, pretty-print
  renderer).
- Modify: `src/trace/mod.rs` (re-export the query API).
- Modify: `src/main.rs` (nested `Trace { action: TraceAction }` subcommand;
  `TraceAction::Why { ...
  }` and `TraceAction::Prune` variants; `cmd_trace_why` handler that reads
  the global `--json` flag for output mode).
- Modify: `tests/smoke.rs` (smoke tests for `lore trace why` + filters + `--json`).

**Approach:**

- `lore trace why <session-id>` reads `<default_trace_dir>/<session-id>.jsonl` (or `.jsonl.gz`,
  auto-decompressed via `flate2::GzDecoder`). Each line is one record.
- `--recent N`: walk the trace directory by mtime newest-first, collect records across all sessions
  until N records gathered.
- `--event <NAME>`: filter records where `event == NAME` (canonical event names only).
- `--tool <NAME>`: filter records where `call_context.tool_name == NAME`.
- `--agent <NAME>`: filter records where `agent == NAME`. Today every record has
  `agent: "claude-code"`; the filter is forward-compat for future Cursor / opencode adapters.
- Pretty-print: one block per record, key fields highlighted (timestamp, event, tool, command head,
  query, candidate table with injected rows marked, duration with phase breakdown).
- **Sanitisation at pretty-print only.** Apply `sanitize_for_log` (or equivalent — see
  `src/hook.rs:685`) to `description`, `file_path`, and `command_head` before emitting each record's
  pretty-printed lines. Mirrors the existing log-sanitisation discipline for semi-trusted data
  flowing into a terminal. `--json` pass-through stays raw — JSON encoding is inherently safe for
  downstream consumers (`jq`, scripts) since the on-disk bytes are JSON-encoded escape strings, not
  raw control characters.
- `--json` mode: emit raw JSONL pass-through — one record per line, stripped of any decompression.
  Per the `cli-suppress-stderr-in-json-mode-2026-04-03.md` learning, stderr stays silent on
  empty/missing session under `--json`; pretty-print mode may emit a tier-2 "no records found"
  stderr warning.
- The tagline (`#[command(after_help)]`-style) is "Explain lore's injection decisions for a
  session."

**Patterns to follow:**

- `cmd_search` (`src/main.rs:539-596`) for the `--json` branch pattern.
- Established nested-subcommand pattern (none yet in this codebase — this unit introduces it).
- `LORE_METADATA_FENCE_TAG` and `extract_lore_metadata_fence` for any future reader needing to parse
  the fence (not applicable to v1 reader, but documented in the institutional notes).

**Test scenarios:**

- Happy path: `lore trace why <session>` on a populated session emits pretty-print on stdout with
  one block per record. Exit code 0.
- **Covers AE4 (--json mode).** `lore trace why <session> --json` emits raw JSONL on stdout
  identical to the file content (modulo gzip decompression). Stderr is empty.
- Happy path: `lore trace why --recent 10` returns the most recent 10 records across all sessions.
- Happy path: `lore trace why <session> --event PreToolUse` filters to PreToolUse records only.
- Happy path: `lore trace why <session> --tool Bash` filters to Bash records only.
- Happy path: `lore trace why <session> --agent claude-code` returns all records (today's only
  adapter). With a synthesised second-adapter record in the same session file, `--agent <other>`
  filters that one in. Forward-compat smoke test.
- Happy path: combined filters compose (`--event PreToolUse --tool Bash --agent claude-code`).
- **Sanitisation at pretty-print.** Given a trace record with `description` containing ANSI escape
  codes (`"\x1b[2Jevil"`) and `file_path` containing newlines (`"src/foo\nbar.rs"`), pretty-print
  emits both fields with control characters escaped (e.g. `"\u{1b}[2Jevil"` rendered as visible
  escape sequence, no terminal reset). With `--json`, the same record passes through as raw JSONL
  (the encoded escape strings are preserved).
- Edge case: nonexistent session id — pretty-print mode prints "No traces found for session
  <id>" to stderr, exits 0. `--json` mode emits `[]` on stdout, stderr silent.
- Edge case: empty trace file — pretty-print mode prints "Session <id> has no records" to stderr,
  exits 0. `--json` mode emits `[]`.
- Edge case: gzipped session file — auto-decompresses transparently; output is identical to
  non-gzipped equivalent.
- Edge case: malformed JSON line in a trace file — tier-2 warn to stderr, skip line, continue with
  remaining lines.
- Edge case: `--recent N` with N > total records — returns all records, exit 0.

**Verification:** `just ci` passes. Smoke tests cover the pretty-print and `--json` output shapes
plus filters. Manual: `lore trace why` after dogfooding shows readable output.

---

### U7. `lore status` and MCP `lore_status` trace-state enrichment

**Goal:** Surface trace state through `lore status` and the MCP `lore_status` tool when tracing is
enabled, matching the existing `empty_knowledge_dir` / `knowledge_dir_status` conditional-field
pattern.

**Requirements:** R21, R22.

**Dependencies:** U2 (the trace dir + state file have to exist for the stats to be meaningful), U5
(`last_pruned_at` becomes a field).

**Files:**

- Modify: `src/main.rs` `cmd_status` (`:729-789`): add a `Trace:` block when tracing is enabled.
- Modify: `src/server.rs` `lore_status` tool handler: add a `trace` object key when tracing is
  enabled.
- Create: `src/trace/stats.rs` (a `TraceStats` struct: directory path, file count, total bytes,
  oldest mtime, newest mtime, `last_pruned_at`) plus a builder reading from disk.
- Modify: `src/trace/mod.rs` (re-export `TraceStats`).
- Modify: `tests/smoke.rs` (smoke test for `lore status` Trace block — present when enabled, absent
  when disabled).
- Modify: existing MCP integration test file for `lore_status` (find at planning time; likely
  `tests/server.rs` or `tests/mcp.rs`).

**Approach:**

- `TraceStats::compute(config)` reads the trace directory: walks the files (excluding
  `.last_pruned_at`), tallies count + total bytes, finds oldest + newest mtime, reads
  `.last_pruned_at` if present. Also reads the privacy-toggle state from `config.trace` to populate
  the `Capture` summary.
- `cmd_status` augments its output with a `Trace:` block when `Config::trace_enabled()` is true and
  the trace directory exists. Block is human-formatted (path, sessions, total, oldest, newest, last
  pruned, capture posture). The `Capture:` line summarises privacy toggles: shows
  `default redaction (command head only, transcript tail off)` when both toggles are at default, or
  surfaces each elevated toggle on its own line with a `privacy-sensitive` marker.
- MCP `lore_status` handler adds a `trace` object alongside the existing fields when tracing is
  enabled. The object includes the stat fields plus a nested `capture` sub-object:
  `{ "command_head_only": bool, "transcript_tail_included": bool, "warnings": [<string>] }`. The
  `warnings` array is non-empty when privacy-elevated toggles are on, surfacing tokens like
  `"full_command_body_captured"` and `"transcript_tail_captured"`.

**Patterns to follow:**

- `cmd_status` existing scan-state and empty-knowledge-dir presentation (`src/main.rs:729-789`).
- MCP `lore_status` existing `empty_knowledge_dir` / `knowledge_dir_status` conditional-field
  pattern.
- `walkdir::WalkDir` for the trace-directory traversal (already a dep).

**Test scenarios:**

- Happy path: tracing enabled + populated trace dir → `lore status` includes Trace block with
  accurate counts; MCP `lore_status` includes `trace` object.
- Happy path: tracing disabled → no Trace block in CLI output; no `trace` key in MCP response.
- Edge case: tracing enabled but trace dir empty → Trace block shows 0 sessions, total 0, oldest /
  newest as `None`; MCP response includes the `trace` object with those nulls.
- Edge case: tracing enabled but trace dir doesn't exist → same as empty (graceful).
- Edge case: `.last_pruned_at` malformed → field shows as `unknown` in CLI output, `null` in MCP.
- **Capture posture — default redaction.** Given `[trace] include_full_command = false` and
  `include_transcript_tail = false`, `lore status` Trace block shows
  `Capture: default redaction (command head only, transcript tail off)`. MCP `lore_status`
  `trace.capture.warnings` is `[]`; `command_head_only = true`, `transcript_tail_included = false`.
- **Capture posture — full command on.** Given `include_full_command = true`, `lore status` shows
  the `FULL COMMAND BODY (privacy-sensitive — ...)` line. MCP `trace.capture.warnings` contains
  `"full_command_body_captured"`.
- **Capture posture — transcript tail on.** Given `include_transcript_tail = true`, `lore status`
  shows the `TRANSCRIPT TAIL (privacy-sensitive — ...)` line. MCP `trace.capture.warnings` contains
  `"transcript_tail_captured"`.
- **Capture posture — both on.** Both lines visible in CLI; both warning tokens in MCP `warnings`
  array.
- Regression: existing `lore status` output is unchanged when tracing is disabled.

**Verification:** `just ci` passes. CLI smoke + MCP integration tests confirm the conditional
fields. Manual: `lore status` shows the Trace block in a fresh dogfood session.

---

### U8. Documentation updates

**Goal:** Update `docs/configuration.md`, `docs/architecture.md`, and
`docs/hook-pipeline-reference.md` for the new surfaces, plus the CHANGELOG entry.

**Requirements:** Repo conventions (documentation-multi-surface-consistency, doc terminology
standards, CHANGELOG style); plus the brainstorm Success Criterion on the institutional note (which
landed in U1).

**Dependencies:** All behavioural units (U1-U7) so the docs describe shipped behaviour.

**Files:**

- Modify: `docs/configuration.md` (add `[trace]` section table; document `LORE_TRACE` env var;
  reference `XDG_STATE_HOME` and `default_trace_dir` path resolution).
- Modify: `docs/architecture.md` (add trace files to the session-local-state carve-out list in the
  sole-read-surface invariant section).
- Modify: `docs/hook-pipeline-reference.md` (brief callout in the PreToolUse pipeline section noting
  where trace-write happens; mention the other three event handlers similarly).
- Modify: `CHANGELOG.md` (one assertive-voice sentence ending in `(#N)` per the repo convention,
  placed in `[Unreleased]` under `### Added`).
- Optional: `README.md` if it enumerates CLI subcommands — verify at write time.

**Approach:**

- `docs/configuration.md` `[trace]` entry follows the existing `[search]` row shape: table column
  per option with type, default, description. Includes:
  - The 30-day default `retain_days` and 7-day `gzip_older_than_days`, plus the fact that setting
    either to `0` disables that phase.
  - The privacy posture: redaction defaults; what `include_full_command = true` and
    `include_transcript_tail = true` each capture; the existing 32 KB cap on `transcript_tail` from
    `TRANSCRIPT_TAIL_BYTES` so operators can budget per-record sizes.
  - The `LORE_TRACE` env var with the symmetric `LORE_DEBUG`-style truthy/falsy parsing
    (`1`/`true`/`yes` vs. `0`/`false`/`no`, case-sensitive, silent fall-through).
  - Steady-state disk footprint estimate (~30-60 MB post-gzip at default knobs for a heavy operator)
    so users can plan retention.
  - The XDG-state-tier path, with `$XDG_STATE_HOME` as the env-var override (no `lore.toml` knob for
    the directory, mirroring dedup-file precedent).
- `docs/architecture.md` carve-out: append a bullet "Session-local trace files
  (`$XDG_STATE_HOME/lore/traces/*.jsonl[.gz]`) and the throttle state file (`.last_pruned_at`)" to
  the existing carve-out list.
- `docs/hook-pipeline-reference.md`: add one paragraph after the existing PreToolUse pipeline
  description noting that step 9 (post-dedup) is a fire-and-forget trace write when
  `Config::trace_enabled()` is true; same paragraph notes the other three handlers' trace-write
  slots.
- `CHANGELOG.md` entry (assertive voice): "Add opt-in per-hook trace logging under
  `$XDG_STATE_HOME/lore/traces/` with `lore trace why` query CLI and `lore trace prune` maintenance
  (#N)."

**Patterns to follow:**

- `docs/configuration.md` `[search]` table (lines TBD at write time).
- `docs/architecture.md` carve-out list at the bottom of the sole-read-surface invariant section.
- CHANGELOG style per memory `feedback_changelog_entries.md`: one assertive-voice sentence ending in
  `(#N)`, detail goes in the PR body.

**Test scenarios:** _Test expectation: none — documentation-only changes. Verification is via
`dprint check` (run by `just ci`) plus a manual read of the doc against shipped behaviour._

**Verification:** `just ci` passes (includes `dprint check`). Manual review confirms each doc
surface reads as standalone guidance and cross-references back to the right code or other docs.

---

## System-Wide Impact

- **Interaction graph.** New `src/trace/` module is consumed by `src/hook.rs` (write path),
  `src/main.rs` (query + prune commands + status block), and `src/server.rs` (MCP status). The
  `SearchResult` extension propagates into `src/database.rs` (RRF + SELECT sites), `src/server.rs`
  (MCP fence enrichment), and `src/hook.rs` indirectly (the trace record consumes the extended
  fields). No new external dependencies on the database layer.
- **Error propagation.** Trace writes are fire-and-forget. Lazy maintenance failures degrade to a
  silent skip with `LORE_DEBUG`-gated diagnostics. Manual `lore trace prune` reports per-file errors
  to stderr but exits 0 on partial success (tier-2).
- **State lifecycle.** Trace files live until `retain_days` mtime expiry; the `.last_pruned_at`
  state file is updated by both lazy and manual maintenance. No conflicts with existing dedup state
  (lives in `$TMPDIR`) or knowledge.db (sole-read-surface invariant preserved).
- **API surface parity.** CLI `lore status` and MCP `lore_status` add symmetric trace-state fields.
  The MCP `search_patterns` response gains optional pre-fusion score fields inside the existing
  `lore-metadata` fence — additive only.
- **Unchanged invariants.** `tests/invariants.rs` audit unchanged on `src/hook.rs`, `src/main.rs`,
  `src/server.rs`, `src/database.rs`, `src/ingest.rs`. New `src/trace/writer.rs` and
  `src/trace/maintenance.rs` are added to the audit list with their expected file-I/O counts pinned.
  The architecture.md sole-read-surface invariant is preserved (traces are session-local state, not
  indexed content).

---

## Risks & Dependencies

| Risk                                                                                                      | Mitigation                                                                                                                                                                                                           |
| --------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Trace write fails silently and operators can't tell tracing is broken                                     | `LORE_DEBUG=1` surfaces every write failure with a clear prefix; `lore status` Trace block shows file counts so an unchanging count post-session is the obvious tell                                                 |
| `SearchResult` extension breaks an MCP consumer that pinned the old response shape                        | U3 updates `search_patterns_response_metadata_pins_hybrid_shape` test; consumers iterate optional fields per the additive-only contract                                                                              |
| Per-list score plumbing through `reciprocal_rank_fusion_n` has subtle bugs around id-collision OR-combine | U3 has explicit edge-case tests for id-appearing-in-multiple-lists; the rrf_collision regression suite from PR #50 stays green                                                                                       |
| Lazy maintenance on cold disk produces noticeable SessionStart lag at the 100-file cap                    | Cap chosen to keep worst-case unlink cost under ~100ms; tested cold-cache scenarios; `lore trace prune` manual mode for users who want to drain a backlog faster                                                     |
| Both writers (hook + manual) corrupt `.last_pruned_at` on race                                            | Manual `lore trace prune` and lazy SessionStart maintenance are mutually exclusive in practice (one operator); a single-line atomic write (`std::fs::write`) is sufficient; if true concurrency emerges, add fd_lock |
| `[trace] include_full_command = true` accidentally enabled in CI logs leaks secrets                       | Default is `false`; documented privacy warning in `docs/configuration.md`; the `predicate suppress:` precedent at `src/hook.rs` line cited                                                                           |
| `temp-env` is unfamiliar to the repo; tests fragile                                                       | Established precedent in the testing-env-var learning landed 2026-05-14; pattern is `temp_env::with_vars(...)` only                                                                                                  |
| Forward-compatible JSONL schema bites on a future breaking change                                         | `schema_version: 1` is the integer; bumps are explicit and tied to a reader update in the same PR per the schema-migration-strategy convention                                                                       |

---

## Documentation / Operational Notes

- Operators enable tracing by setting `[trace] enabled = true` in `lore.toml` or `LORE_TRACE=1` in
  the environment. Disabled by default; no behaviour change for users who don't opt in.
- `lore trace prune` is the manual maintenance escape hatch — wire it into cron / launchd /
  systemd-timer if external scheduling is preferred over the lazy SessionStart path.
- After this plan merges, `ROADMAP.md` gains the Track 2 Observability completion line in
  `## Completed` (with link to this plan), and the existing `Up Next` entry moves to `## Completed`.
  Done as part of the final commit.
- Track 1B dedup-bypass override remains queued in `ROADMAP.md` Future once that ride-along lands;
  revisiting that decision is the natural next observability-data-driven task.

---

## Sources & References

- **Origin document:**
  [`docs/brainstorms/2026-05-14-track-2-observability-requirements.md`](../brainstorms/2026-05-14-track-2-observability-requirements.md)
- **Path-resolution prerequisite (shipped):**
  [`docs/plans/2026-05-14-001-refactor-etcetera-xdg-path-resolution-plan.md`](2026-05-14-001-refactor-etcetera-xdg-path-resolution-plan.md)
- **Closest functional analogue (instrument hook + add query CLI):**
  [`docs/plans/2026-04-03-002-feat-debug-logging-json-output-plan.md`](2026-04-03-002-feat-debug-logging-json-output-plan.md)
- **Structural template (most recent comparable plan):**
  [`docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`](2026-05-07-001-feat-universal-pattern-predicate-plan.md)
- **Follow-up template (smaller pattern):**
  [`docs/plans/2026-05-08-001-feat-sessionstart-respect-applies-when-plan.md`](2026-05-08-001-feat-sessionstart-respect-applies-when-plan.md)
- **Language detection precedent (introduces three-list RRF):**
  [`docs/plans/2026-05-14-001-feat-language-detection-architecture-plan.md`](2026-05-14-001-feat-language-detection-architecture-plan.md)
- Institutional learnings cited under Context & Research above, individually linked.
