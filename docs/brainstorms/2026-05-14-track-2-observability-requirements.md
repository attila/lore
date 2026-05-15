---
date: 2026-05-14
topic: track-2-observability
---

# Track 2 Observability — per-hook decision tracing

## Summary

Add opt-in, agent-agnostic per-hook trace logging to lore. Each hook invocation writes a structured
JSONL record to the user state directory; a new `lore trace why` command renders those records for
inspection. Bounded retention with configurable compression, scoped redaction, and a
separately-landed path-resolution prerequisite.

---

## Problem Frame

The completed Track 2 measurement workstream (200 sessions, 2,208 hooked tool-uses across four
falsified hypotheses) established that lore's current per-call behaviour is mostly its _intended_
search recall at present defaults — not a regression. Where to take the broader Track 2 work
(threshold tuning, predicate-scope refactor, validate-the-refactor passes, continuous dogfooding)
depends on data lore cannot currently produce.

`LORE_DEBUG=1` emits roughly 70% of the fields a useful trace record would contain, but as ephemeral
stderr text — useless for cross-session analytics or any after-the-fact investigation. The harness
used for the Track 2 measurement reconstructed records from Claude Code's session backups, which is
feasible for one-shot retrospectives but not for ongoing decisions.

Without persistent, structured, queryable traces, every subsequent Track 2 decision is guesswork.

---

## Actors

- A1. **Operator** — single-author of lore; runs lore via Claude Code today and may run via other
  agents in future. Dual-hatted as pattern author and trace consumer.
- A2. **Hook adapter** — per-agent translation code that maps each agent's lifecycle into canonical
  hook event names. Claude Code adapter exists today; future Cursor / opencode adapters write to the
  same schema.

---

## Key Flows

- F1. **Trace write**
  - **Trigger:** A2 invokes a canonical hook event while tracing is enabled
  - **Actors:** A2
  - **Steps:** hook handler builds CallContext → executes its normal decision pipeline (search,
    predicate, dedup) → fire-and-forget write of a structured record to the session's trace file →
    hook returns its output to the agent
  - **Outcome:** structured record persisted; the hook returns within its established timeline
    regardless of write success or failure
  - **Covered by:** R1, R4, R6, R7, R8, R19
- F2. **Trace query**
  - **Trigger:** A1 runs `lore trace why <session>` (or `lore trace why --recent N`)
  - **Actors:** A1
  - **Steps:** command opens the session's trace file → decompresses gzipped sections transparently
    → applies any filters → emits pretty-print (default) or raw JSONL (`--json`)
  - **Outcome:** A1 sees per-call decisions including candidates, scores (pre-fusion + combined),
    predicate outcomes, dedup status, final injected set, and timing breakdown
  - **Covered by:** R16, R17, R18
- F3. **Trace maintenance**
  - **Trigger:** SessionStart hook event (lazy path) or A1 running `lore trace prune` (explicit
    path)
  - **Actors:** A1 (sometimes implicitly via SessionStart)
  - **Steps:** check throttle state (skip if last maintenance was within 24h, lazy path only) →
    compress files older than the gzip horizon, capped → delete files older than the retention
    horizon, capped
  - **Outcome:** trace directory size stays bounded; lazy maintenance never produces noticeable
    SessionStart lag
  - **Covered by:** R11, R12, R13, R14

---

## Requirements

**Storage and location**

- R1. Hook traces are persisted as JSON Lines (one record per line), one file per session, named by
  session id, stored in the user state directory under the XDG state tier.
- R2. Each record carries an explicit `schema_version` field, enabling forward-compatible evolution.
- R3. The trace schema names events using a canonical taxonomy (PreToolUse, PostToolUse,
  SessionStart, PostCompact). Adapter-specific lifecycle event names are mapped onto this taxonomy
  at the adapter boundary.

**Activation and precedence**

- R4. Tracing is disabled by default. Operators opt in via persistent config flag, per-session env
  var, or both. The env var takes precedence over the config when set.
- R5. When tracing is disabled, no records are written and the hook path incurs no measurable
  additional latency.

**Record contents**

- R6. Each event record captures, at minimum: timestamp, session id, canonical event name, agent
  identifier, call-context fields (tool name, command head, file path, description), extracted
  query, inferred languages (per the post-#50 language detection — drives which retrieval lists are
  populated), candidate list, final injected set, search and embedder configuration snapshot, Ollama
  availability, total duration, and per-phase duration breakdown. SessionStart records carry a full
  configuration snapshot once per session; per-event records carry only the search and embedder
  subset (the only mid-session-tunable fields).
- R7. Per-candidate fields include: chunk id, source pattern path, universal-tag flag,
  predicate-presence flag, language-declaration flag (declared vs. undeclared per the post-#50
  `language_json` column), pre-fusion FTS-fallback score, pre-fusion FTS-structural score,
  pre-fusion vector score, post-fusion combined score, predicate outcome, threshold pass/fail, and
  dedup status. Preserving the three pre-fusion component scores requires a database-layer change;
  post-#50 retrieval composes the three lists via `reciprocal_rank_fusion_n` and discards per-list
  scores inside fusion.

**Privacy and redaction**

- R8. Bash command bodies are reduced to the first whitespace-delimited token by default.
  `description` and `file_path` are captured verbatim.
- R9. Operators may opt in to full Bash command capture via a dedicated config flag.
- R10. Operators may opt in to transcript-tail capture via a separate dedicated config flag.

**Retention and maintenance**

- R11. Trace files are pruned by file modification time with a configurable retention horizon
  (default: 30 days). Files older than a configurable gzip horizon (default: 7 days) are compressed
  in place.
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
  `--event <NAME>` and `--tool <NAME>` filter the output.

**Compatibility**

- R19. The trace mechanism preserves the existing "hook never breaks the agent" contract: hooks
  return on the established timeline regardless of trace write success or failure.
- R20. The MCP server (`lore serve`) is unaffected by tracing. Tracing is a hook-context concern;
  MCP runs in a separate context that does not invoke hook handlers.

---

## Acceptance Examples

- AE1. **Covers R4.** Given `[trace] enabled = false` in `lore.toml` and `LORE_TRACE` unset, when a
  hook event fires, no trace file is created and no record is written. Given
  `[trace] enabled = true` in `lore.toml` and `LORE_TRACE=0` set in the environment, when a hook
  event fires, no record is written — the env var overrides the config.
- AE2. **Covers R8, R9.** Given `[trace] include_full_command = false` (default) and a Bash hook
  records the command `git push origin main`, the resulting trace record's `command_head` field is
  `"git"` and the full command body is absent from the record. Given
  `[trace] include_full_command = true`, the resulting record contains the full command body.
- AE3. **Covers R11, R12.** Given trace files exist spanning the last 90 days and
  `[trace] retain_days = 30`, when SessionStart fires and the maintenance throttle permits, files
  older than 30 days are scheduled for deletion (capped at 100 per run) and files older than 7 days
  are gzipped (capped at 100 per run). Given the throttle state file shows maintenance ran less than
  24 hours ago, when SessionStart fires, maintenance is skipped this round.
- AE4. **Covers R16, R17.** Given a trace file exists for session `abc-...`, when
  `lore trace why abc-...` runs without `--json`, stdout contains a human-readable pretty-print with
  per-record sections. When `lore trace why abc-... --json` runs, stdout contains raw JSONL — one
  record per line, identical to the trace file's content modulo any gzip decompression.
- AE5. **Covers R15, R19.** Given the trace file path is read-only or otherwise unwritable, when a
  hook event fires while tracing is enabled, the hook returns its normal output to the agent without
  delay or error. Stderr is silent unless `LORE_DEBUG=1` is set; under `LORE_DEBUG=1` a
  `lore trace: ...` diagnostic appears.

---

## Success Criteria

- An operator running `lore trace why <recent-session>` after enabling tracing can answer "which
  patterns fired on which calls, with what scores, and why some were suppressed" without
  cross-referencing Claude Code's session JSONL or any other source.
- The four documented Track 2 use cases (debug bad injections, tune thresholds, validate refactor,
  continuous dogfooding) become data-driven exercises grounded in trace records rather than
  guesswork or replay-archaeology.
- No production hook execution is broken by the trace mechanism: write failures, full disks,
  permission errors, and similar conditions degrade gracefully without affecting agent behaviour.
- The trace schema is genuinely agent-agnostic: a future Cursor or opencode adapter writes records
  of the same shape, and `lore trace why` reads them without per-agent special-casing.
- The plan ships alongside a one-paragraph institutional note in `docs/solutions/conventions/`
  documenting the env-var-plus-config-flag coexistence policy as a reusable convention for future
  lore toggles.

---

## Scope Boundaries

- Live tailing / follow-mode in the query CLI (`tail -f | jq` covers it for v1)
- Stats-aggregation commands such as `lore trace stats` or fire-rate-per-pattern (composable from
  `--json` + `jq` for v1)
- Trace pinning / immutable snapshots (`lore trace pin <session>`)
- Colour output in pretty-print (deferred to v1.1, with `NO_COLOR` env-var respect when added)
- Time-range filter flags (`--from`, `--to`) on `lore trace why` (`jq` handles it for v1)
- Background / async write path (in-process fire-and-forget for v1)
- Trace compaction (rolling multi-session JSONLs into one file) — keeps per-session granularity
- Heuristic secret-redaction in command arguments (always-imperfect; explicit head-only redaction is
  the safer default)
- Default-on tracing (explicit user-as-operator decision)
- Track 2-B (extending the predicate mechanism to non-universal patterns) — separate workstream
- The Track 1B dedup-bypass override for predicated universals — already deferred to the
  post-observability backlog (`ROADMAP.md` Future section)

---

## Key Decisions

- **JSONL over SQLite.** Matches the dominant convention in the agent-tooling ecosystem (Claude
  Code, Codex, LSP servers, OpenTelemetry-local-emit all use JSONL). Portable via `cat` / `jq` /
  `tail -f` without requiring the `sqlite3` CLI. Crash-safe by append-only construction. Trivially
  diffable for refactor validation. SQLite advantages (aggregate queries, joins, indexing) accrue
  only at scales beyond lore's expected per-user data volume and can be added later via a
  `lore trace import` step if real volume demands it.
- **XDG state tier over cache or data.** Trace files are actions-history records that cannot be
  regenerated, ruling out cache (some environments evict cache, including the sandboxed environment
  lore is sometimes operated from). They are not user-owned documents in the same sense as databases
  or projects, so the data tier over-promotes them. State tier matches modern XDG-conformant tools
  (Helix, Nushell).
- **XDG-everywhere posture on macOS, not platform-native.** Lore's user persona is terminal-fluent
  and expects dotfile-portable paths across machines. Matches lore's existing hand-rolled posture.
  The path-resolution prerequisite makes this posture explicit at the call site rather than
  implicit-by-hand-rolling.
- **Opt-in over default-on.** Default-on conflicts with the user-as-operator instinct that always-on
  tracing is obtrusive. Persistent config flag preserves the "set once and forget" property the
  "tune thresholds" use case requires; env var preserves the per-session override path.
- **Pre-fusion scores preserved per candidate for all three retrieval lists.** Post-#50 retrieval
  composes three independently-ranked lists fed to RRF: FTS-fallback for undeclared patterns,
  FTS-structural for declared patterns matching inferred languages, and oversample-and-filter
  vector. Threshold tuning and "why did this rank where it did" debugging require knowing which list
  produced signal for a given candidate. Post-fusion combined score alone makes those analyses
  guesswork. The implementation cost (preserving component scores through `reciprocal_rank_fusion_n`
  in the database layer) is real but bounded.
- **Two separate redaction toggles, not one unified flag.** `include_transcript_tail` and
  `include_full_command` are independent because the two fields have meaningfully different content
  (user prompt vs. tool argument); granular control fits the privacy model. The
  default-redact-to-head posture mirrors lore's existing `predicate suppress:` log discipline, which
  redacts to the first command token specifically because the `gh auth login --token XXX` leakage
  case was a known concern in prior code.
- **30-day retention default.** Bounds disk footprint while preserving sample size for the
  threshold-tuning use case. Empirical baseline check: Claude Code's own session retention is ~60
  days locally (verified 2026-05-14: oldest 2026-03-16, newest 2026-05-14, 373 files). Lore's own
  retention is a product decision rather than a mirror; 30 days is the tighter floor.
- **Lazy maintenance on SessionStart, not external scheduler.** Lore is interactive-CLI-shaped, not
  daemon-shaped. Cron / launchd / Task Scheduler all require platform-specific setup and produce
  "tool works only after user wires up scheduling" friction we deliberately avoid. The throttle
  pattern is small and works correctly out of the box on all platforms. The `lore trace prune`
  manual escape hatch preserves the external-scheduler workflow for operators who prefer it.
- **Plan ships a one-paragraph institutional note on env-var-plus-config-flag coexistence.** Track 2
  Observability is the first lore feature where both an env var (`LORE_TRACE`) and a config flag
  (`[trace] enabled`) toggle the same setting. The plan documents the precedence rule (env var wins)
  as a reusable convention so future toggles inherit the pattern without re-deciding.
- **`SearchResult` extension for pre-fusion score capture.** Extend `SearchResult` in
  `src/database.rs` with three `Option<f64>` fields for pre-fusion FTS-fallback, FTS-structural, and
  vector scores rather than introducing a sidecar struct. Cleaner integration with the existing
  search pipeline; fewer call-site changes. `None` when the result did not come from the
  corresponding list, `Some` when it did. The per-list scores are currently discarded inside
  `reciprocal_rank_fusion_n`; the refactor preserves them on each list's `SearchResult`s before
  fusion.
- **MCP `search_patterns` gains the new score fields as intentional forward-compatible enrichment.**
  The three new `SearchResult` fields above flow naturally into the MCP `search_patterns` response
  (via the existing fenced `lore-metadata` block pattern). R20's "MCP unaffected by tracing" remains
  correct in scope — tracing itself runs only in hook context — but the cross-cutting struct change
  deliberately exposes the new fields to MCP consumers too. Treated as additive metadata, not
  breaking. The existing `search_patterns_response_metadata_pins_hybrid_shape` test gets updated to
  pin the new fields.
- **`last_pruned_at` throttle state is bumped by both writers.** Both the lazy hook-driven
  maintenance pass (on SessionStart, throttled to ≤ once per 24h) and the explicit
  `lore trace prune` command bump the `last_pruned_at` timestamp in the state file. Hazard-pinned by
  an integration test that interleaves the two writers (per the
  out-of-band-writers-bypass-delta-checkpoint learning). Rejected alternatives: hook-only ownership
  (manual prune wouldn't influence throttle, leading to surprise back-to-back lazy passes),
  prune-only ownership (manual prune wouldn't help, since the hook would still throttle from its own
  state).

---

## Dependencies / Assumptions

- **Path-resolution refactor — shipped 2026-05-14 via PR #52.** Lore's hand-rolled XDG resolution
  was replaced with the `etcetera` crate using the `Xdg` strategy. `default_config_path` and
  `default_database_path` were retrofitted; `default_trace_dir()` was added returning
  `$XDG_STATE_HOME/lore/traces` (with `$HOME/.local/state/lore/traces` fallback). Track 2
  Observability builds on this helper directly. See
  `docs/plans/2026-05-14-001-refactor-etcetera-xdg-path-resolution-plan.md`.
- **Language detection three-list RRF — shipped 2026-05-14 via PR #50.** Retrieval now composes
  three independently-ranked lists into `reciprocal_rank_fusion_n` (FTS-fallback for undeclared
  patterns, FTS-structural for declared patterns matching inferred languages, oversample-and-filter
  vector). The trace record schema reflects this: three pre-fusion scores per candidate (R7) and an
  `inferred_languages` field per call (R6). See
  `docs/plans/2026-05-14-001-feat-language-detection-architecture-plan.md`.
- The four canonical hook event names (PreToolUse, PostToolUse, SessionStart, PostCompact) —
  currently Claude Code's terms — are adopted as lore's canonical taxonomy. Future agent adapters
  (Cursor, opencode) map their lifecycle events onto these names rather than introduce parallel
  naming.
- A new `[trace]` config section is added to `lore.toml` alongside the existing `[search]` and
  `[git]` sections.
- The existing global `--json` flag (shipped in the 2026-04-03-002 plan) is reused as-is for
  `lore trace why`; no breaking change to its handling for `lore search` / `lore list`.
