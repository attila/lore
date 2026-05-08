---
title: "feat: Universal-pattern predicate (`applies_when`) and engine/adapter split"
type: feat
status: completed
date: 2026-05-07
completed: 2026-05-08
origin: docs/brainstorms/2026-05-07-universal-pattern-predicate-requirements.md
---

# feat: Universal-pattern predicate (`applies_when`) and engine/adapter split

## Summary

Add an optional `applies_when` predicate to universal-tagged patterns that gates re-injection by
tool class and Bash command prefix; persist it as a JSON column on chunks and patterns (schema bump
v3 with forward-compatible ALTER TABLE migration on first open — no `lore ingest --force` required);
extend the hand-rolled frontmatter parser for the nested mapping. Reorganise hook code into an
agent-agnostic engine (predicate evaluator, smart-prefix matcher, query extraction, pure-string
helpers) operating on a minimal `CallContext`, and a Claude-Code-specific adapter in `src/hook.rs`
that owns `HookInput` deserialisation and the `HookInput → CallContext` conversion (with eager
transcript-tail read). Ship `min_relevance_universal` as an optional config field that inherits from
`min_relevance` when unset. Audit the three MCP single-file write paths so the new column
round-trips.

---

## Problem Frame

`workflows/git-branch-pr.md` is tagged `universal`, bypasses the read-side dedup at
`src/hook.rs:700`, and re-injects on every `PreToolUse:Bash` call — including `ls`, `wc -l`, and
`grep` where its content has no relevance. Pattern authors today face a binary choice: tag
`universal` and accept the noise, or de-universalise and lose always-on visibility. Full motivation
in origin doc.

---

## Requirements

Carried forward from origin (R1-R11). Track 1 implements all eleven plus the engine/adapter split
captured in Key Technical Decisions.

- R1. Optional `applies_when` block on universal-tagged patterns gates `PreToolUse` re-injection.
- R2. Predicate supports `applies_when.tools` (tool-class allowlist) and
  `applies_when.bash_command_starts_with` (Bash command prefix allowlist).
- R3. Predicate semantics: OR within each list, AND across keys.
- R4. `applies_when.tools` matches when current tool name is in the list.
- R5. `applies_when.bash_command_starts_with` matches when call is Bash AND command starts with a
  listed token (after walking past `sudo` and `env KEY=VAL` wrappers).
- R6. `min_relevance_universal` config knob with default = current `min_relevance`.
- R7. `applies_when` namespace reserved for Track 2-B extensions (`languages`, `environments`).
- R8. Predicate evaluator runs only for universal-tagged chunks in Track 1; namespace parses on any
  pattern.
- R9. Malformed `applies_when` at ingest → skip-with-warning; pattern fires as if absent.
- R10. Predicate-level suppression logged via `LORE_DEBUG` (pattern, tool, command head).
- R11. Existing universal patterns without `applies_when` continue firing on every relevant call;
  pattern-side migration is follow-up work.

**Origin acceptance examples:** AE1-AE5 covered by U2, U3, U5, U7; AE6 covered by U6 (see test
scenarios).

---

## Scope Boundaries

- Regex predicates (`bash_command_match`) — not implemented.
- Path-glob, language, and environment predicates for non-universal patterns — Track 2-B.
- Score instrumentation, fire-rate aggregation — Track 2.
- Tuned value of `min_relevance_universal` — Track 2 informs.
- Nested-quote / escaped-quote handling inside `bash -c` (e.g. `bash -c "echo \"git status\""`),
  recursive wrapper-stripping inside the `bash -c` quoted body (e.g. `bash -c "sudo git status"` —
  the inner `sudo` is not unwrapped), and quoted `KEY=VAL` with internal spaces (e.g.
  `env "A=value with spaces" git status`) — remaining documented Track 1 limitations. The common
  cases (`sudo -u USER`, `env -u VAR`, `env -i`, multiple KEY=VAL, sudo with short flags, leading
  whitespace, nested env wrappers, `bash -c "..."` and `sh -c '...'` quoted-command extraction) are
  implemented in U3.
- Per-pattern threshold tuning in frontmatter — config-level only.
- Pattern-side migration of `workflows/git-branch-pr.md` and `agents/unattended-work.md` — Track 1's
  smoke test uses fixture patterns; pattern repo migration is a follow-up.
- Per-section predicates inside a pattern — `applies_when` is whole-file; every chunk of a pattern
  carries its frontmatter's predicate.
- New CLI subcommand for predicate debugging — future tooling work.

### Deferred to Follow-Up Work

(none — `extract_query`'s migration to `CallContext` is in scope per B+ decision)

---

## Context & Research

### Relevant Code and Patterns

- **PreToolUse pipeline.** `src/hook.rs:152-249` (`handle_pre_tool_use`): seeds → expand → dedup →
  inject. Predicate filter slots between expand (line 183) and dedup (line 202).
- **Dedup bypass for universal chunks.** `src/hook.rs:700`
  (`r.is_universal || !seen.contains(&r.id)`). Predicate suppression does NOT touch dedup state —
  suppressed chunks must not be recorded as seen.
- **Frontmatter parsing (hand-rolled, no YAML library).** `src/chunking.rs:287-331`
  (`parse_frontmatter_tag_list`); `src/chunking.rs:265-282` (`frontmatter_near_miss_tags`);
  `src/chunking.rs:340-365` (`extract_frontmatter`, `strip_frontmatter`). Currently only handles
  `tags: [...]` inline-list and block-list. Extending to a nested mapping (`applies_when:` with
  indented children) is the parser delta.
- **Chunk and pattern row schemas.** `src/database.rs:558-569` (`CHUNKS_DDL`),
  `src/database.rs:595-604` (`PATTERNS_DDL`). `is_universal INTEGER NOT NULL DEFAULT 0` is the
  template for the new column. `SCHEMA_VERSION` constant at `src/database.rs:552`; compatibility
  probe at `src/database.rs:629-667` already produces the "Run `lore ingest --force`" advisory.
- **`SearchResult` round-trip.** `src/database.rs:62-75`. Three SELECT sites build the struct: FTS
  (`src/database.rs:230-256`), vector (`src/database.rs:261-291`), `chunks_by_sources`
  (`src/database.rs:405-430`). All three need the new column.
- **MCP single-file write paths.** `src/server.rs` exposes `add_pattern`, `update_pattern`,
  `append_to_pattern`. They each reach the private helper `index_single_file` (`src/ingest.rs:1237`)
  directly — `ingest_single_file` is a separate wrapper used only by the CLI `--file` path. Track
  1's column plumbing must land inside `index_single_file` (the common funnel for both CLI
  single-file ingest and the three MCP write tools) so the new column round-trips through every
  authoring surface.
- **Hook test conventions.** `tests/hook.rs:962-981` (`setup_with_universal_pattern` fixture);
  `tests/hook.rs:1278-1319` (`run_pre_tool_use_sequence` helper); `tests/hook.rs:1415-1457`
  (canonical "must NOT be in output" pattern). Unit tests for pure helpers go in `src/hook.rs`
  `#[cfg(test)] mod tests` (currently around `src/hook.rs:1048+`); after the engine split, those
  tests follow the helpers into the engine module.
- **Config loading.** `src/config.rs:5-16` (`Config`), `src/config.rs:29-35` (`SearchConfig`).
  `min_relevance: f64` lives under `[search]` with serde defaults at `src/config.rs:37-39` and
  `src/config.rs:48-69`. Use `Option<f64>` + accessor for `min_relevance_universal` to encode
  "absent → inherit" cleanly.
- **Logging conventions.** `lore_debug!` at `src/debug.rs:33-40`. Existing trace shapes: `expand:`
  (`src/hook.rs:184`), `dedup:` (`src/hook.rs:205`), `injecting:` (`src/hook.rs:233`).
  `sanitize_for_log` at `src/hook.rs:520` is the canonical control-character escape — reuse for
  predicate-suppression lines.
- **Closest existing predicate analogues.** `skip_agent` (`src/hook.rs:779-781`) for tool-class
  short-circuits; `language_from_bash` (`src/hook.rs:808-824`) for token-table matching against Bash
  commands (no smart-prefix today, but the table-driven shape is right).
- **Architecture invariant.** `docs/architecture.md` + `tests/invariants.rs` static-grep: no
  `fs::read*` / `File::open` in runtime modules outside the documented allow-list. Engine module
  must stay disk-I/O-free; adapter retains all transcript-read responsibility.

### Institutional Learnings

- `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md` — the
  dedup-bypass-for-universal rationale. Predicate suppression must NOT record chunk into dedup file
  (a later predicate-passing call would otherwise be silently skipped).
- `docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`
  — schema-bump pattern. The `lore ingest --force` advisory must be reachable (i.e., not blocked by
  the probe itself). Includes the "remedy-completion" regression test pattern.
- `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  — hazard template. Sibling expansion is a pre-existing reconciliation pass; `applies_when` is
  whole-file (frontmatter-derived) so siblings share the predicate uniformly. Pin with a test.
- `docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md` — MCP
  write-path audit requirement. Each of `add_pattern`, `update_pattern`, `append_to_pattern` must
  carry `applies_when_json` end-to-end through `ingest_single_file`.
- `docs/solutions/best-practices/short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md`
  — rationale for predicate-as-primary-primitive. Pattern body size confounds FTS scoring; threshold
  tuning alone is structurally insufficient for universal-pattern over-firing.
- `docs/solutions/database-issues/fts5-query-construction-for-hook-based-search-2026-04-02.md` —
  predicate persistence on `chunks` (not sidecar) so sibling-expansion via `chunks_by_sources`
  preserves it without a join.
- `docs/solutions/best-practices/coverage-check-query-source-must-simulate-hook-not-llm-2026-04-08.md`
  — `evaluate_applies_when(&AppliesWhen, &CallContext) -> bool` pure and reusable so future
  coverage-check work can simulate predicate behaviour.
- `docs/solutions/integration-issues/additional-context-timing-in-pretooluse-hooks-2026-04-02.md` —
  one-tool-call lag between injection and Claude using a convention. Tests for predicate-driven
  re-fires that assert behavioural compliance must compose at least two calls.
- `docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md` —
  smart-prefix matcher must operate on raw `tool_input.command`, never on FTS-cleaned terms (so `gh`
  survives the 3-char filter that would otherwise drop it).
- `docs/plans/2026-04-20-001-feat-universal-patterns-plan.md` — prior schema-bump precedent (the
  v1→v2 migration that added `is_universal`).

---

## Key Technical Decisions

- **Persist `applies_when` as a JSON `TEXT NULL` column on `chunks` AND `patterns`.** Mirrors how
  `is_universal` is laid out (dedicated column on both tables). A column on `chunks` lets
  `chunks_by_sources` propagate predicate state without a left-join. Sidecar table rejected: breaks
  parity, forces a join.
- **Schema bump to `SCHEMA_VERSION = 3` with forward-compatible ALTER TABLE migration.** The v2→v3
  bump is purely additive (a nullable `applies_when_json` column with no default; NULL means "no
  predicate" which is the current pre-Track-1 behaviour). The schema-compatibility probe at
  `src/database.rs:629-667` gains a v2→v3 branch that runs
  `ALTER TABLE chunks ADD
  COLUMN applies_when_json TEXT` and the equivalent for `patterns`, sets
  `PRAGMA user_version = 3`, and continues without bailing. No `lore ingest --force` is required for
  this bump — existing populated DBs migrate silently on next open. Deliberate departure from the
  prior precedent: v1→v2 (`is_universal`) required full re-ingest because new behaviour depended on
  populated columns; v2→v3 has no such dependency. The hard-bail path remains reserved for
  non-additive future bumps.
- **Smart-prefix matcher walks past at most one `sudo` and one `env KEY=VAL` wrapper.** Multi-env,
  `env -u`, `env -i`, `sudo -u user`, quoted values are documented Track 1 limitations. Matcher
  operates on raw `tool_input.command`, never through FTS term-cleaning.
- **Hand-roll the `applies_when` frontmatter parser as a sibling of the existing `tags` parser in
  `src/chunking.rs`.** No new YAML dependency. Avoids `serde_yaml` (binary-size budget concern, per
  parked memory) and keeps parser idiom consistent.
- **`min_relevance_universal: Option<f64>` with effective-value accessor.** Falls back to
  `min_relevance` when unset. Config-level (under `[search]`) — no precedent for per-pattern
  numerical knobs in frontmatter.
- **Engine/adapter split: predicate evaluator, smart-prefix matcher, `extract_query`, and pure-
  string helpers (`language_from_bash`, `language_from_extension`, `filename_terms`, `clean_terms`,
  `split_into_words`, `truncate_str`) live in a new agent-agnostic engine module taking a minimal
  `CallContext`. `src/hook.rs` becomes a Claude-Code adapter that owns `HookInput` deserialisation,
  the `HookInput → CallContext` conversion, the eager transcript- tail read with `$HOME` validation,
  and the PreToolUse/SessionStart/PostCompact/PostToolUse handlers.** Future Cursor/opencode
  integration writes its own adapter; engine module unchanged.
- **`CallContext` carries pre-extracted strings**: `tool_name`, `command`, `file_path`,
  `description`, `transcript_tail`. Adapter eagerly populates all five. Engine never reads disk;
  `tests/invariants.rs` allow-list unchanged.
- **Predicate suppression does NOT record into dedup file.** Suppressed chunks bypass both the read
  and write side of `dedup_filter_and_record`. A later predicate-passing call still fires.
- **Skip-with-warning on malformed `applies_when` at ingest.** Per-file warning via the existing
  `on_progress` channel naming the offending key. Pattern is ingested with
  `applies_when_json =
  NULL` and fires as if no predicate were set — equivalent to current
  pre-Track-1 behaviour.
- **Predicate logging shape**: one aggregate `predicate:` line per call (matching the `expand:` /
  `dedup:` family), one detailed `predicate suppress:` line per dropped chunk naming the source,
  tool, and command head (truncated). Both gated by `LORE_DEBUG`. Source/command fragments passed
  through `sanitize_for_log`.

---

## Open Questions

### Resolved During Planning

- **Persistence shape on the chunk row** → JSON column on `chunks` AND `patterns`, not sidecar.
- **Sudo/env tokenisation rule** → tokenise raw command on whitespace; walk past at most one `sudo`
  and one `env KEY=VAL` (single token of shape `[A-Z_][A-Z0-9_]*=...`); complex variants are
  documented limitations.
- **Threshold knob location** → `[search].min_relevance_universal: Option<f64>` with an
  `effective_min_relevance_universal()` accessor falling back to `min_relevance`.
- **`CallContext` field types** → owned `String` (one allocation per field; one call per
  `PreToolUse`; borrowed lifetimes would add complexity for one-call construction). Default unless
  profiling motivates a change.

### Deferred to Implementation

- Final engine-module layout (single `src/engine.rs` file vs `src/engine/` directory with submodules
  `call_context.rs`, `predicate.rs`, `query.rs`, `text.rs`). Recommend starting with the directory
  layout from day one — the file count justifies it — but the implementer may collapse to a single
  file if it reads cleaner.
- Naming of the predicate-suppression aggregate log line. Current proposal:
  `predicate: N before
  -> M after (K suppressed)`. Implementer may adjust prefix to match
  `expand:` / `dedup:` exactly.
- Whether `extract_query`'s transcript-tail read deserves a feature gate / fast-path now that it's
  eager. Default: no — measure if needed; the read is bounded at 32KB.

---

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

### Module boundary after Track 1

```
┌─────────────────────────────────────────────────────────────┐
│ src/hook.rs (Claude Code adapter)                           │
│  - HookInput deserialisation (Claude Code stdin event JSON) │
│  - HookInput::to_call_context()                             │
│      eagerly reads transcript tail with $HOME validation    │
│  - validate_transcript_path, last_user_message              │
│      (filesystem I/O — stays here, allow-listed already)    │
│  - PreToolUse / SessionStart / PostCompact / PostToolUse    │
│  - search_with_threshold (uses Config + DB)                 │
│  - dedup_filter_and_record (universal bypass intact)        │
│  - format_imperative                                        │
└──────────────────┬──────────────────────────────────────────┘
                   │ depends on (calls into)
                   ▼
┌─────────────────────────────────────────────────────────────┐
│ src/engine/ (or src/engine.rs) — agent-agnostic engine      │
│  - CallContext { tool_name, command, file_path,             │
│                  description, transcript_tail }             │
│  - AppliesWhen { tools, bash_command_starts_with }          │
│  - evaluate_applies_when(&AW, &CC) -> bool                  │
│  - command_matches_with_wrappers(...)  // sudo/env stripper │
│  - extract_query(&CC) -> Option<String>                     │
│  - language_from_bash, language_from_extension              │
│  - filename_terms, clean_terms, split_into_words            │
│  - truncate_str                                             │
│  - NO disk I/O (tests/invariants.rs invariant preserved)    │
│  - NO HookInput import                                      │
└─────────────────────────────────────────────────────────────┘

Future: src/cursor_adapter.rs / src/opencode_adapter.rs deserialise their
own input format, build a CallContext, and call the same engine functions.
```

### PreToolUse data flow after Track 1

```
HookInput (from Claude Code stdin)
   │
   ▼
[adapter] HookInput::to_call_context()
   │  (eager transcript-tail read happens here, with $HOME validation)
   ▼
CallContext
   │
   ├──► [engine] extract_query(&CallContext) ──► query string
   │                                                │
   │                                                ▼
   │                                  search_with_threshold ──► seeds (DB)
   │                                                │
   │                                                ▼
   │                                          expand_to_siblings ──► expanded
   │                                                │
   │                                                ▼
   │      [engine] evaluate_applies_when(&AW, &CC)
   │              for each is_universal chunk
   │                       │
   │                       ▼
   │              kept | suppressed
   │                  (suppressions: lore_debug!("predicate suppress: ..."))
   │                  (suppressed chunks DO NOT enter dedup_filter_and_record)
   │                       │
   │                       ▼
   │              dedup_filter_and_record (universal bypass on read side
   │                                       intact for non-suppressed universals)
   │                       │
   │                       ▼
   └──► format_imperative + emit additionalContext
```

### `applies_when` frontmatter shape

```yaml
---
title: Git Branch and PR Workflow
tags:
  - workflow
  - universal
applies_when:
  tools: [Bash]
  bash_command_starts_with: [git, gh]
---
```

Both keys optional; `applies_when` itself optional. Empty lists are valid syntax but match nothing
(zero-element allowlist — documented).

---

## Implementation Units

- U1. **Schema migration: `applies_when_json` column on `chunks` and `patterns`**

**Goal:** Add `applies_when_json TEXT NULL` to both tables; bump `SCHEMA_VERSION` to 3; ensure the
existing compatibility-advisory + `lore ingest --force` remedy round-trip works.

**Requirements:** R1 (persistence shape), R8 (parsing accepts on any pattern).

**Dependencies:** None.

**Files:**

- Modify: `src/database.rs` (`CHUNKS_DDL`, `PATTERNS_DDL` constants, `SCHEMA_VERSION` bump,
  `insert_chunk_in_tx`, `upsert_pattern_in_tx`, the three `SearchResult` SELECT sites: FTS, vector,
  `chunks_by_sources`)
- Test: `tests/database.rs` (or wherever schema tests live; if absent, add inline `#[cfg(test)]`
  block in `src/database.rs`)

**Approach:**

- Preflight: confirm `SCHEMA_VERSION` in `src/database.rs:552` is currently `2`
  (`git grep -n "SCHEMA_VERSION" src/database.rs`). If a concurrent PR has already bumped to `3`,
  coordinate or bump this work to `4`. Plan downstream assumes the bump is `2 → 3`.
- Add `applies_when_json TEXT` (nullable, no `DEFAULT`) to `CHUNKS_DDL` and `PATTERNS_DDL` after
  `is_universal`. Update column-index comments.
- Bump `SCHEMA_VERSION` from 2 to 3.
- **Forward-compatible migration via ALTER TABLE on first open.** Extend the schema- compatibility
  probe at `src/database.rs:629-667` with a v2→v3 branch that runs
  `ALTER TABLE chunks ADD COLUMN applies_when_json TEXT` and the equivalent for `patterns`, then
  sets `PRAGMA user_version = 3` and continues without bailing. No `lore ingest --force` is required
  for this bump — existing chunks have NULL in the new column and behave as if no predicate is set
  (R11). The hard-bail path is reserved for non-additive bumps; this branch is the additive-only
  case.
- Update each `SELECT` statement that builds `SearchResult` to include the new column. Each
  row-builder asserts the appropriate index and decodes via `row.get::<_, Option<String>>(...)`.
- Update `SearchResult` (`src/database.rs:62-75`) to add `applies_when_json: Option<String>`
  (storing JSON text; deserialisation to `AppliesWhen` happens at the engine boundary so the DB
  layer stays JSON-naive).
- Update `insert_chunk_in_tx` and `upsert_pattern_in_tx` to bind the new column.
- Confirm `clear_all` DDL drop+recreate covers the new column (it shares `CHUNKS_DDL` /
  `PATTERNS_DDL` constants — should be automatic).

**Patterns to follow:**

- `is_universal INTEGER NOT NULL DEFAULT 0 CHECK(...)` plumbing as the column template.
- Compatibility probe at `src/database.rs:629-667` gains a new v2→v3 branch that runs ALTER TABLE
  additions and continues. The existing version-mismatch branch (which prints "Run
  `lore ingest --force`") remains for non-additive future bumps.

**Test scenarios:**

- Happy path: fresh DB created at `SCHEMA_VERSION = 3` includes `applies_when_json` on both tables;
  column reads and writes round-trip an `Option<String>`.
- Edge case: `clear_all` after schema bump produces a clean v3 DB.
- Migration regression: construct a v2 DB via raw SQL (no `applies_when_json` column, populated with
  sample chunks); call `KnowledgeDB::open` (without `--force`); the probe applies the ALTER TABLE
  additions, bumps `PRAGMA user_version` to 3, returns successfully without bailing. Existing chunks
  now have `applies_when_json = NULL`; ingesting a new pattern with `applies_when` populates the
  column normally. Verifies the forward-compatible migration path.
- Integration: `chunks_by_sources` returns the new column for every chunk of a matched source
  (sibling-expansion preserves predicate state without a join).

**Verification:**

- `just ci` passes; tests above pass.
- `PRAGMA table_info(chunks)` and `PRAGMA table_info(patterns)` both list `applies_when_json`.

---

- U2. **Frontmatter parser: nested `applies_when` mapping**

**Goal:** Parse the `applies_when:` block from pattern frontmatter into an `AppliesWhen` struct;
detect malformed top-level keys, wrong types, and unknown nested keys for ingest-time advisories.

**Requirements:** R1, R2, R3, R4, R5 (predicate authoring surface), R8 (parses on any pattern), R9
(malformed → skip-with-warning).

**Dependencies:** None.

**Files:**

- Modify: `src/chunking.rs` (new `parse_frontmatter_applies_when`, `AppliesWhen` struct,
  malformed-detection helper)
- Test: `src/chunking.rs` `#[cfg(test)] mod tests`

**Approach:**

- Define `AppliesWhen { tools: Option<Vec<String>>, bash_command_starts_with: Option<Vec<String>> }`
  in `src/chunking.rs` (it's the parser's output; engine re-exports or wraps as needed).
- Hand-roll a parser that extracts the `applies_when:` line and walks indented child keys (`tools:`,
  `bash_command_starts_with:`) using the same flat-list logic as `parse_frontmatter_tag_list`. Both
  inline-list (`[a, b]`) and block-list forms are supported, mirroring `tags` parsing.
- **Indentation contract** (specified in the parser and documented for authors in U8): top-level
  keys at column 0; nested keys under `applies_when:` at 2-space indent; block-list items under
  nested keys at 4-space indent (`- git`). Tabs are not accepted. Documenting the contract prevents
  0/2/0 indentation mismatches that would silently fail to parse.
- Parse `applies_when` once at `chunk_by_heading` (`src/chunking.rs:46`) alongside the existing
  `frontmatter_has_tag(content, "universal")` call near line 54. Serialise to JSON once. Thread the
  resulting `Option<String>` into every `Chunk` produced by the `flush` closure (whole-file
  semantics — predicate is frontmatter-derived, not per-heading).
- Return `(Option<AppliesWhen>, Vec<MalformedPredicateEntry>)` where `MalformedPredicateEntry`
  carries the file path, the offending key, and a short reason.
- Detect: typo'd top-level key (`appliess_when:`, `applies_when:` with sibling typos),
  scalar-where-list expected, unknown nested keys, deeply-nested structures we don't support,
  tab-indented children (reject with a "tabs not supported" malformed entry).
- Empty list (`tools: []`, `bash_command_starts_with: []`) is valid syntax and parses to
  `Some(vec![])`. Documented behaviour: empty allowlists never match (see U8 documentation).

**Patterns to follow:**

- `parse_frontmatter_tag_list` (`src/chunking.rs:287-331`) for the inline-vs-block list shape.
- `frontmatter_near_miss_tags` (`src/chunking.rs:265-282`) for the malformed-detection return-a-Vec
  idiom.

**Test scenarios:**

- Happy path: full block with both keys (inline-list form) parses to populated `AppliesWhen`.
- Happy path: block-list form (`- git\n- gh`) parses identically.
- Happy path: only `tools` set parses to
  `AppliesWhen { tools: Some(...), bash_command_starts_with: None }`.
- Happy path: only `bash_command_starts_with` set parses similarly.
- Happy path: missing block parses to `None` (no advisory).
- Happy path: `tags: [universal]` without `applies_when` block → returns `None` with no malformed
  entries.
- Edge case: empty list `tools: []` parses to `Some(vec![])` (no advisory; documented).
- Edge case: unicode value (`bash_command_starts_with: [gît]`) parses without error.
- Error path: typo'd top-level key (`appliess_when:`) → returns `None` for predicate, one
  `MalformedPredicateEntry` naming the typo. **Covers AE5.**
- Error path: scalar where list expected (`tools: Bash`) → returns `None`, malformed entry names the
  type mismatch.
- Error path: unknown nested key (`applies_when.foo: bar`) → returns predicate with the known keys
  parsed, malformed entry names the unknown key.
- Indentation: 2-space-indented children with inline-list values parse correctly.
- Indentation: 2-space-indented children with 4-space-indented block-list items parse correctly.
- Indentation: tab-indented children → returns malformed advisory entry naming the unsupported
  indentation; pattern fires as if `applies_when` were absent.
- Indentation: 4-space-indented top-level key (e.g. `applies_when:` mistakenly indented under
  `tags:`) → not detected as `applies_when`, parses as `None` (no advisory; structurally identical
  to a missing block).

**Verification:**

- `cargo test -p lore --lib chunking` passes all parser tests including the malformed and
  indentation-variant cases.

---

- U3. **Engine: `CallContext`, `AppliesWhen` evaluator, smart-prefix matcher**

**Goal:** Establish the agent-agnostic engine module. Define `CallContext` and the predicate
evaluator + smart-prefix matcher, all operating without `HookInput` knowledge.

**Requirements:** R3 (semantics), R4 (tools matching), R5 (smart-prefix matcher with sudo/env
stripping), R8 (engine evaluator runs only for universal-tagged chunks — controlled at the caller in
U5, but the function itself is generic).

**Dependencies:** U2 (re-uses `AppliesWhen` type or wraps it).

**Files:**

- Create: `src/engine/mod.rs`, `src/engine/call_context.rs`, `src/engine/predicate.rs` (recommended
  layout — single `src/engine.rs` is acceptable if implementer prefers)
- Test: alongside the source files in `#[cfg(test)] mod tests`

**Approach:**

- The evaluator function is agent-agnostic and evaluates any `AppliesWhen` against any
  `CallContext`. The caller (the hook adapter at U5) is responsible for deciding whether to invoke
  it — Track 1 invokes it only for chunks where
  `is_universal == true AND
  applies_when_json IS NOT NULL`. Non-universal chunks bypass the
  evaluator entirely and flow through the existing dedup pipeline unchanged. This keeps the engine
  generic for future agent integrations and Track 2-B (which extends evaluation to non-universal
  patterns).
- Define
  `CallContext { tool_name: Option<String>, command: Option<String>,
  file_path: Option<String>, description: Option<String>, transcript_tail: Option<String> }`.
  Owned `String`s; one allocation per field, called once per PreToolUse — simple beats borrow
  lifetimes here.
- Re-export or wrap `AppliesWhen` from `chunking.rs`. The decision affects which crate-internal
  module owns the canonical type definition; the implementer may move the struct to the engine if it
  cleans up the import graph.
- `evaluate_applies_when(&AppliesWhen, &CallContext) -> bool`:
  - If `tools` set, current `tool_name` must be in the list (case-sensitive match against Claude
    Code tool names).
  - If `bash_command_starts_with` set, `tool_name` must be `Bash` AND the command (after walking
    past one `sudo` and one `env KEY=VAL` wrapper) must start with one of the listed tokens.
  - All set keys must match (AND across keys).
- `command_matches_with_wrappers(command: &str, allowlist: &[String]) -> bool`:
  - Tokenise on whitespace.
  - **Sudo wrapper.** If first token is `sudo`: advance past it. If the next token is `-u`, consume
    it AND the following token (the user). Continue scanning past short flags like `-E`, `-H`
    (single-token), but stop on the first non-flag token.
  - **Env wrapper.** If the (now-current) token is `env`: advance past it. Repeatedly consume any
    of: `-i` (single-token, hermetic-environment flag), `-u <var>` (two tokens, unset-var flag), or
    `[A-Z_][A-Z0-9_]*=<value>` (single-token, KEY=VAL assignment). Stop on the first token that
    doesn't match any of those shapes.
  - The token after wrappers must equal one of the allowlist values (exact match, case- sensitive).
- Helper does NOT pass through `clean_terms`, `split_into_words`, or any FTS-cleaning. Raw command
  in, prefix match out.

**Patterns to follow:**

- `language_from_bash` (`src/hook.rs:808-824`) for the table-driven matching shape.
- `skip_agent` (`src/hook.rs:779-781`) for the early-filter idiom.

**Test scenarios:**

- Happy path: predicate `bash_command_starts_with: [git]` + Bash "git status" → fires.
- Happy path: predicate `bash_command_starts_with: [git]` + Bash "sudo git status" → fires
  (smart-prefix walks past sudo). **Covers AE1.**
- Happy path: predicate `bash_command_starts_with: [git]` + Bash "env GIT_PAGER=cat git log" → fires
  (smart-prefix walks past env).
- Happy path: predicate `bash_command_starts_with: [git, gh]` + Bash "gh pr create" → fires.
- Happy path: predicate `tools: [Bash]` only + any Bash call (e.g. "ls") → fires. **Covers AE3.**
- Happy path: predicate `tools: [Bash]` only + Edit foo.rs → suppressed. **Covers AE3.**
- Happy path: predicate with both keys + Bash "git push" → fires; + Bash "ls" → suppressed;
  - Edit foo.rs → suppressed. **Covers AE4.**
- Edge case: predicate `bash_command_starts_with: [git]` + Bash "ls" → suppressed. **Covers AE2.**
- Edge case: empty list `bash_command_starts_with: []` → never matches.
- Edge case: empty list `tools: []` → never matches.
- Edge case: command empty string → no match.
- Edge case: command "sudo" alone (no following token) → no match.
- Happy path: Bash "env -u VAR git status" → fires (env `-u` is consumed as a two-token unset-var
  flag).
- Happy path: Bash "sudo -u user git push" → fires (sudo `-u USER` is consumed).
- Happy path: Bash "env -i git status" → fires (env `-i` is consumed as a hermetic-env flag).
- Happy path: Bash "env A=1 B=2 git status" → fires (multiple KEY=VAL tokens consumed).
- Happy path: Bash "env -u A -u B git status" → fires (multiple `-u` flags consumed).
- Happy path: Bash "sudo -E git push" → fires (sudo with non-`-u` short flag).
- Edge case: documented limitation — Bash "env A=1 env B=2 git status" → does NOT fire (nested env
  wrappers are not unwrapped; only the outer env scope is consumed). Unusual in practice; single env
  covers nearly all realistic invocations.
- Edge case: documented limitation — Bash 'bash -c "git status"' → does NOT fire (quoted-command
  handling not implemented; the matcher sees `bash` as the command head, not the quoted git
  invocation inside).
- Integration with caller: predicate evaluator returns `true` when `applies_when` is `None` (caller
  decides whether to run the evaluator at all; here we test the function itself treats `None` as
  "fires").

**Verification:**

- `cargo test -p lore engine::predicate` passes.
- `tests/invariants.rs` static-grep passes (no `fs::read*` / `File::open` in the new module).

---

- U4. **Engine: move `extract_query` and pure-string helpers**

**Goal:** Migrate `extract_query` and the pure-string helpers it transitively uses from
`src/hook.rs` into the engine module. `extract_query` takes `&CallContext` instead of `&HookInput`.
No behavioural change beyond the lazy→eager transcript read (the eager read itself is U5's adapter
responsibility).

**Requirements:** R8 (engine reusability across future agent integrations — beyond predicate
evaluator alone).

**Dependencies:** U3 (`CallContext` type).

**Files:**

- Create / Modify: `src/engine/query.rs` (gains `extract_query`, `language_from_bash`,
  `language_from_extension`, `filename_terms`, `clean_terms`)
- Create / Modify: `src/engine/text.rs` (gains `split_into_words`, `truncate_str`)
- Modify: `src/hook.rs` (loses these functions; imports them from the engine)
- Test: existing `extract_query` and helper unit tests migrate from `src/hook.rs` to the new engine
  modules. They reformulate from `HookInput` JSON fixtures to `CallContext` direct construction.

**Approach:**

- Move each helper one-for-one into the engine. Adjust signatures to take `&CallContext` fields
  where they currently take `&HookInput` (only `extract_query` itself needs the signature change;
  the helpers already take `&str` or owned strings).
- `extract_query(&CallContext) -> Option<String>` reads `cc.tool_name`, `cc.command`,
  `cc.description`, `cc.file_path`, `cc.transcript_tail` directly — no I/O, no
  `validate_transcript_path` call (the adapter already populated `transcript_tail`).
- Remove the `tool_input_str` accessor's use inside the moved code; callers that need to peek into
  `HookInput` keep that helper in the adapter.

**Patterns to follow:**

- The existing `extract_query` body (`src/hook.rs:721-775`) is the structural reference; the
  migration is mechanical.

**Test scenarios:**

- Migration: every existing `extract_query` test (currently in `src/hook.rs`
  `#[cfg(test)] mod
  tests`) is replicated in the engine module against a hand-built `CallContext`
  and produces identical query strings.
- Migration: every existing `language_from_bash`, `language_from_extension`, `filename_terms`,
  `clean_terms` test is replicated in the engine module unchanged.
- Happy path: `CallContext` with `command: "cargo build"` and `tool_name: "Bash"` produces the
  expected `rust AND (cargo OR build)` style query.
- Happy path: `CallContext` with `file_path: "src/foo.rs"` and `tool_name: "Edit"` produces the
  expected query.
- Edge case: `CallContext` with all-`None` fields → `extract_query` returns `None`.
- Edge case: `CallContext` with only `transcript_tail` set → query draws from transcript-tail terms.

**Verification:**

- `cargo test -p lore engine::query` and `engine::text` pass.
- `cargo test -p lore --tests` (integration) still passes — `tests/hook.rs` consumes `extract_query`
  only indirectly via the adapter.

---

- U5. **Hook adapter: `HookInput → CallContext`, predicate filter call site, suppression logging**

**Goal:** Wire the engine into `src/hook.rs`. Add `HookInput::to_call_context()` (eager
transcript-tail read), insert the predicate filter between `expand_to_siblings` and
`dedup_filter_and_record`, emit predicate-suppression debug logs.

**Requirements:** R3-R5 (predicate evaluation at the hook layer), R10 (predicate-level suppression
logging), R11 (existing universals without `applies_when` continue firing).

**Dependencies:** U3 (engine evaluator), U4 (engine query extraction now used via `CallContext`).

**Files:**

- Modify: `src/hook.rs` — add `HookInput::to_call_context()`, refactor `handle_pre_tool_use` to use
  it, insert predicate filter, add suppression log lines
- Test: `tests/hook.rs` — extend `setup_with_universal_pattern` and `run_pre_tool_use_sequence` for
  predicate AE coverage; add the unit-test for `to_call_context` in `src/hook.rs`
  `#[cfg(test)] mod tests`

**Approach:**

- `impl HookInput { fn to_call_context(&self) -> CallContext }`:
  - Reads `tool_name`, `tool_input.command`, `tool_input.file_path`, `tool_input.description`.
  - Eagerly reads the transcript tail: if `transcript_path` is present and validates,
    `last_user_message` returns the truncated tail. If validation or read fails, `transcript_tail`
    is `None`.
  - All `validate_transcript_path` and `last_user_message` calls stay in `src/hook.rs` — engine
    module never touches the filesystem.
- `handle_pre_tool_use` flow (order matters — preserves existing skip-path behaviour):
  - **`skip_agent` runs FIRST** (existing `src/hook.rs:158` early return for Explore/Plan
    subagents). This MUST happen before `to_call_context` so the eager transcript-tail read does not
    fire for skip-paths. Existing behaviour preserved.
  - Build `CallContext` once via `HookInput::to_call_context()` (eager transcript-tail read happens
    here for non-skip paths only).
  - `extract_query(&cc)` to get the query string.
  - `search_with_threshold` (now consults `min_relevance_universal` for universal results — see U6).
  - `expand_to_siblings`.
  - **NEW:** predicate filter — for each chunk where `is_universal == true` AND `applies_when_json`
    is `Some`, deserialise to `AppliesWhen` and call `evaluate_applies_when(&aw, &cc)`. Suppressed
    chunks are dropped from the local `Vec<SearchResult>` _before_ the dedup call. Universal chunks
    without `applies_when_json` (or `None`-deserialising) pass through unchanged.
  - Aggregate log: `predicate: 7 before -> 5 after (2 suppressed)` after the filter.
  - Per-suppression log: `predicate suppress: <pattern> tool=<tool> cmd_head="<cmd>"` for each
    dropped chunk; pattern source and command head pass through `sanitize_for_log`.
  - **Suppressed chunks are NEVER passed to `dedup_filter_and_record`.** The predicate filter runs
    first and drops them from the `Vec<SearchResult>` before the dedup call sees the list. The dedup
    file therefore never records a suppressed chunk's id, neither on the read side nor the write
    side. This preserves the invariant that a later `PreToolUse` call with a different (matching)
    command can still trigger the same pattern; suppression is per-call, not per-session. Surviving
    chunks (universal or otherwise) flow through `dedup_filter_and_record` as today
    (universal-bypass on read; write side records all surviving chunks).

**Patterns to follow:**

- `expand_to_siblings` (`src/hook.rs:255-274`) for the filter-and-clone Vec<SearchResult> return
  shape.
- `lore_debug!` line shapes at `src/hook.rs:184` (`expand:`), `src/hook.rs:205` (`dedup:`).
- `sanitize_for_log` (`src/hook.rs:520`) for control-character escaping.

**Test scenarios:**

- Unit: `HookInput::to_call_context` with all fields populated produces full `CallContext` including
  transcript tail.
- Unit: `HookInput::to_call_context` with no `transcript_path` → `transcript_tail = None`.
- Unit: `HookInput::to_call_context` with invalid transcript path → `transcript_tail = None` (silent
  failure mirrors existing `extract_query` behaviour).
- Integration: **Explicit AE2 coverage.** Pattern `workflows/git-branch-pr.md` (fixture) tagged
  `universal` with `applies_when.bash_command_starts_with: [git, gh]`. Run the PreToolUse sequence:
  (1) `Bash ls`, (2) `Bash wc -l`, (3) `Bash grep`. Verify: none of these calls have the pattern in
  `additionalContext`; `LORE_DEBUG` output contains exactly three `predicate suppress:` lines naming
  the pattern with the respective commands (`ls`, `wc`, `grep`); **the dedup file size is unchanged
  after the three calls** (assert byte-for-byte equality with the dedup file's pre-sequence state —
  proves the suppressed chunk's id never reached the write side, pinning the invariant against a
  future refactor that moves the predicate filter elsewhere). Then (4) `Bash git push` → pattern IS
  in `additionalContext`, dedup file gains the chunk's id (universal write-side semantics intact).
  **Covers AE1, AE2.**
- Integration: pattern with typo'd `applies_when` (e.g. `appliess_when:`) → ingest emits warning,
  pattern fires on every Bash call as if no predicate set. **Covers AE5.**
- Integration: existing universal pattern `agents/unattended-work.md` (fixture) without
  `applies_when` → continues firing on every Bash call. **Covers R11.**
- Integration: predicate-suppressed chunk does NOT enter dedup file. After suppression on `Bash ls`,
  a subsequent `Bash git push` still injects the pattern (would be silent-skipped if suppression
  incorrectly recorded into dedup).
- Integration: predicate evaluator runs only for `is_universal == true` chunks; non-universal chunks
  continue to flow through `dedup_filter_and_record` unchanged. **Covers R8.**

**Verification:**

- `just ci` passes; integration tests above pass.
- Manual dogfooding: in a fresh session in this repo, `LORE_DEBUG=1 lore hook` fed representative
  `Bash ls` and `Bash git push` events shows the expected suppression / injection.

---

- U6. **Config: `min_relevance_universal` knob**

**Goal:** Add the optional config knob with inherit-from-`min_relevance` semantics; apply it to
universal results inside `search_with_threshold`.

**Requirements:** R6.

**Dependencies:** None (independent of U1-U5; ordering is a convenience).

**Files:**

- Modify: `src/config.rs` (new field, accessor)
- Modify: `src/hook.rs` (`search_with_threshold` consults `effective_min_relevance_universal()` for
  universal results)
- Test: `src/config.rs` `#[cfg(test)] mod tests` (round-trip + accessor tests); extend
  `tests/hook.rs` for the universal-floor behaviour

**Approach:**

- Add `min_relevance_universal: Option<f64>` under `[search]` with `#[serde(default)]` (defaults to
  `None`).
- `impl SearchConfig { fn effective_min_relevance_universal(&self) -> f64 {
  self.min_relevance_universal.unwrap_or(self.min_relevance) } }`.
- In `search_with_threshold`, after each result is scored: filter universal results against
  `effective_min_relevance_universal()`, non-universal against `min_relevance` (current behaviour).
- `Config::default_with` sets `min_relevance_universal: None` so a fresh install matches current
  behaviour exactly.

**Patterns to follow:**

- `min_relevance: f64` plumbing (`src/config.rs:33-34`, `src/config.rs:37-39`,
  `src/config.rs:48-69`).

**Test scenarios:**

- Round-trip: config without `min_relevance_universal` → `effective_min_relevance_universal` equals
  `min_relevance`.
- Round-trip: config with `min_relevance_universal = 0.7` and `min_relevance = 0.6` → effective
  universal floor is 0.7.
- Round-trip: config without `min_relevance_universal` and `min_relevance = 0.8` →
  `effective_min_relevance_universal` is 0.8 (tracks `min_relevance`).
- Search behaviour: universal pattern scoring 0.65 with default config → fires (tracks
  `min_relevance` default 0.6); same pattern with `min_relevance_universal = 0.7` → does not fire.
- Search behaviour: non-universal pattern scoring 0.65 → fires regardless of
  `min_relevance_universal` value.

**Verification:**

- `cargo test -p lore --lib config` passes.
- `cargo test -p lore --tests` passes.

---

- U7. **Ingest: persist `applies_when_json`, surface malformed warnings, MCP audit**

**Goal:** Plumb the parsed `AppliesWhen` from frontmatter into the new column on every ingest path;
emit per-file warnings for malformed predicates; verify the three MCP single-file write paths
round-trip the new column.

**Requirements:** R1 (persistence), R8 (parsing on any pattern), R9 (skip-with-warning), R11 (no
forced migration — patterns without `applies_when` ingest unchanged).

**Dependencies:** U1 (column exists), U2 (parser produces `AppliesWhen` and
`MalformedPredicateEntry`).

**Files:**

- Modify: `src/ingest.rs` — pass parsed `AppliesWhen` into chunk and pattern row construction; fold
  `MalformedPredicateEntry`s into `IngestResult`; emit per-file warnings via `on_progress`
- Modify: `src/server.rs` (or wherever the MCP `add_pattern` / `update_pattern` /
  `append_to_pattern` handlers live) — confirm they reach `ingest_single_file` and the new column
  round-trips
- Test: `tests/ingest.rs` (or wherever ingest integration tests live; if absent, add a new test
  file)

**Approach:**

- Extend `Chunk` and `PatternRow` (`src/chunking.rs:18-36, 179-186`) with
  `applies_when_json: Option<String>` (the JSON-serialised form, or `None` when no predicate).
  Preserve the `Chunk` no-`Default` invariant — every chunk-construction site must explicitly set
  the new field, mirroring the `is_universal` discipline. Update the doc comment at
  `src/chunking.rs:14-17` to name the new field.
- The whole-file parse-once-and-propagate happens in U2 (at `chunk_by_heading`); U7's job is the
  persistence wiring.
- Extend `pattern_row_from` (`src/chunking.rs:195-213`) to mirror `applies_when_json` from
  `chunks.first()` onto the constructed `PatternRow`, matching the `is_universal` mirror at
  line 200. Without this, MCP `update_pattern` would silently drop the column on the patterns row
  even though chunk rows persist it correctly.
- `insert_chunk_in_tx` and `upsert_pattern_in_tx` (U1) bind the new column. The actual
  chunk-and-pattern construction site is `index_single_file` (`src/ingest.rs:1237-1330`), which is
  the common funnel for both CLI single-file ingest and the three MCP write tools.
- Extend `IngestResult` (`src/ingest.rs:111-150`) with
  `malformed_applies_when:
  Vec<MalformedPredicateEntry>`. CLI ingest emits one
  `on_progress("Warning: pattern <path>: malformed applies_when: <reason>")` per entry. MCP write
  tools (which return `WriteResult` and do not construct an `IngestResult`) surface the warning via
  `eprintln!` from inside `index_single_file` so the operator sees it on stderr regardless of
  authoring surface.
- Emit an additional info warning at ingest when a pattern carries `applies_when` but does NOT have
  `universal` in its `tags`:
  `"pattern <path> has applies_when but is not
  universal-tagged; predicate is dormant in Track 1 (see Track 2-B)"`.
  Prevents the silent fail-open mode where a pattern author sets the predicate on a non-universal
  pattern and gets no signal that it does nothing — same skip-with-warning posture as R9.
- MCP audit: behaviourally test all three write tools (`add_pattern`, `update_pattern`,
  `append_to_pattern`). For each: write a body containing valid `applies_when` frontmatter through
  the JSON-RPC layer; query the resulting chunk and pattern rows directly; assert
  `applies_when_json` is non-null on both rows and round-trips identically. Inspection alone is not
  sufficient verification.

**Patterns to follow:**

- `is_universal` chunk/pattern-row construction and DB binding inside `index_single_file`
  (`src/ingest.rs:1237-1330`) — the actual funnel where `applies_when_json` plumbing lands.
- `pattern_row_from` (`src/chunking.rs:195-213`) as the row-builder template for mirroring the new
  field from `chunks.first()` onto `PatternRow` (the `is_universal` mirror at line 200 is the direct
  reference).
- `near_miss_universal_tags` accumulation (`src/ingest.rs:1180-1217`) as the structural template for
  malformed advisory folding.
- `on_progress` warning emission idiom (`src/ingest.rs:507, 676, 688, 720`) for CLI ingest. MCP
  write tools use `eprintln!` from inside `index_single_file` since they do not construct an
  `IngestResult`.
- Out-of-band-writers learning's prescription to audit single-file paths via behavioural test, not
  inspection.

**Test scenarios:**

- Happy path: ingest a pattern with valid `applies_when` → both the chunk row and the pattern row
  have non-null `applies_when_json` containing the serialised JSON. Verify both rows directly to
  catch a missing `pattern_row_from` mirror.
- Happy path: ingest a pattern without `applies_when` → both rows have `applies_when_json = NULL`.
- Error path: ingest a pattern with malformed `applies_when` (typo'd top-level key) → both rows have
  `applies_when_json = NULL`, `IngestResult.malformed_applies_when` lists the entry, stderr (via
  `on_progress`) shows the warning. **Covers AE5.**
- Error path: ingest a pattern carrying `applies_when` but NO `universal` tag → row persists, ingest
  emits an info warning naming the file ("predicate is dormant in Track 1 — see Track 2-B"). Pattern
  is NOT predicate-gated at hook time (R8 invariant — evaluator runs only for universal-tagged
  chunks).
- Integration: MCP `add_pattern` with a body containing valid `applies_when` → query both the
  resulting chunk row AND the pattern row directly; assert `applies_when_json` non-null and
  round-trips identically on both rows.
- Integration: MCP `update_pattern` with valid `applies_when` → both rows updated. Then strip
  `applies_when` from the body and re-update → both rows have `applies_when_json = NULL` (no stale
  residue). Covers
  `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`.
- Integration: MCP `append_to_pattern` round-trips `applies_when_json` on existing chunks without
  dropping the column.
- Integration: MCP write of a malformed `applies_when` → operator sees the warning via stderr (since
  `WriteResult` carries no warnings field).
- Integration: delta ingest of a pattern whose `applies_when` was added in the new commit → chunks
  update with the new value (whole-file unconditional rewrite per the delta-ingest reconciliation
  learning).
- Edge case: pattern with multiple sections — every chunk of the file shares the same
  `applies_when_json` value (whole-file predicate). Pin with a test.

**Verification:**

- `just ci` passes; ingest tests above pass.
- Manually constructed pattern with `applies_when` ingests and produces the expected DB rows.

---

- U8. **Documentation: pattern-authoring, configuration, hook-pipeline references**

**Goal:** Document the new authoring surface and config knob; note the engine/adapter split and the
eager transcript-read.

**Requirements:** R1, R6 (visibility for authors and operators).

**Dependencies:** U2-U7 (need stable behaviour to document accurately).

**Files:**

- Modify: `docs/pattern-authoring-guide.md` — new section "Tool/command predicate (`applies_when`)"
  covering authoring, semantics (OR within list / AND across keys), smart-prefix matcher behaviour,
  documented limitations (multi-env, sudo -u, env -u/-i, quoted values), empty-list semantics
- Modify: `docs/configuration.md` — new row for `[search].min_relevance_universal` with
  inherit-from-`min_relevance` description
- Modify: `docs/hook-pipeline-reference.md` — note the engine/adapter split, the predicate filter
  slot in PreToolUse, the eager transcript-tail read
- Modify: `ROADMAP.md` — mark this work as in-progress / completed when applicable

**Approach:**

- Each doc section includes a short authoring example showing the frontmatter shape.
- Pattern-authoring guide section is a sibling of "Tag Strategy" and "When to use the universal tag"
  (the latter from the prior universal-patterns work).
- Hook-pipeline reference section names the new module path and the boundary contract.

**Test scenarios:**

- Verification: `dprint check` passes on all modified docs.
- Verification: documentation review confirms the documented limitations match the actual
  smart-prefix matcher behaviour from U3.

**Verification:**

- `just ci` passes (includes `dprint check`).
- Manual review.

---

## System-Wide Impact

- **Interaction graph:** New engine module is consumed by `src/hook.rs`. No callbacks, middleware,
  or observers affected. Future agent integrations will consume the same engine module.
- **Error propagation:** Predicate evaluation is total (returns `bool`, no `Result`). Malformed
  predicates at ingest produce per-file `Warning:` lines via the existing `on_progress` channel,
  never errors. Schema migration uses the existing `lore ingest --force` advisory mechanism.
- **State lifecycle risks:** Predicate-suppressed chunks must NOT enter the dedup file — recording
  suppression as "seen" would silently skip a later predicate-passing call. Pinned by U5 integration
  test.
- **API surface parity:** All three MCP single-file write paths (`add_pattern`, `update_pattern`,
  `append_to_pattern`) round-trip `applies_when_json`. Pinned by U7 integration tests.
- **Integration coverage:** Sibling expansion via `chunks_by_sources` returns the new column on
  every chunk row. Predicate is whole-file (every chunk of a pattern shares its frontmatter's
  `applies_when_json`), so siblings behave consistently regardless of which heading matched. Pinned
  by U7 multi-section test.
- **Unchanged invariants:** `tests/invariants.rs` static-grep (engine has no `fs::read*` /
  `File::open`). `dedup_filter_and_record` semantics (universal bypass at `src/hook.rs:700` intact).
  Non-universal pattern firing (only the universal floor changes via U6; non-universal results
  continue to use `min_relevance`). Existing universal patterns without `applies_when` continue
  firing on every relevant call (R11).

---

## Risks & Dependencies

| Risk                                                                                        | Mitigation                                                                                                                                                                                                                                                                                              |
| ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Migration regression: ALTER TABLE fails or leaves DB in inconsistent state                  | U1 migration test: raw-SQL v2 DB → `KnowledgeDB::open` without `--force` → assert columns added, `PRAGMA user_version = 3`, existing rows preserved, new ingest writes the column                                                                                                                       |
| MCP write path silently drops `applies_when_json`                                           | U7 explicit MCP round-trip integration tests for `add_pattern`, `update_pattern`, `append_to_pattern`                                                                                                                                                                                                   |
| Predicate suppression incorrectly recorded into dedup file (next matching call skipped)     | U5 integration test: predicate-suppress on `Bash ls` then verify `Bash git push` still injects the pattern                                                                                                                                                                                              |
| Sibling-expansion + predicate inconsistency (some siblings predicated, others not)          | Whole-file predicate semantics enforced at the chunking layer (U7); multi-section test pins                                                                                                                                                                                                             |
| Engine module accidentally introduces disk I/O, violating `tests/invariants.rs`             | Static-grep enforced by CI; transcript-tail read stays in adapter (U5)                                                                                                                                                                                                                                  |
| `extract_query` behaviour shift (lazy → eager transcript read) introduces a perf regression | `skip_agent` short-circuit runs BEFORE `to_call_context` so Explore/Plan subagents bypass the read entirely (preserving existing behaviour). For non-skip paths, the read is bounded at 32KB cap, once per PreToolUse — same I/O the existing flow already performed when `transcript_path` was present |
| Frontmatter parser regression on existing patterns                                          | U2 parser is additive; existing `tags`-only patterns ingest unchanged. `just ci` regression tests cover existing fixtures                                                                                                                                                                               |
| Pattern authors unable to discover the new field                                            | U8 docs — pattern-authoring guide and configuration reference both updated                                                                                                                                                                                                                              |
| Schema bump conflicts with concurrent v2 → v3 universal-patterns ingest                     | `SCHEMA_VERSION = 3` matches the next available bump after the prior universal-patterns work. Verify `SCHEMA_VERSION` was 2 at planning time; if v3 was claimed by other work in flight, bump to v4                                                                                                     |

---

## Documentation / Operational Notes

- Pattern authors learn about `applies_when` via the pattern-authoring guide.
- Operators learn about `min_relevance_universal` via `docs/configuration.md`.
- After merge, recommend a follow-up PR (separate Track) that migrates `workflows/git-branch-pr.md`
  to use `applies_when.bash_command_starts_with: [git, gh]` and `agents/unattended-work.md` to
  `tools: [Bash]` (per origin R11).
- After merge, Track 2 (instrumentation + threshold tuning) can build on this PR's
  predicate-suppression logging without further engine changes.
- After merge, the next agent integration (Cursor / opencode) lives in its own adapter module;
  engine module needs no changes.

---

## Sources & References

- **Origin document:**
  [`docs/brainstorms/2026-05-07-universal-pattern-predicate-requirements.md`](docs/brainstorms/2026-05-07-universal-pattern-predicate-requirements.md)
- Prior universal-patterns work:
  [`docs/plans/2026-04-20-001-feat-universal-patterns-plan.md`](docs/plans/2026-04-20-001-feat-universal-patterns-plan.md)
- Schema-bump precedent (compatibility advisory):
  [`docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`](docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md)
- Dedup-bypass rationale:
  [`docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`](docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md)
- MCP-audit driver:
  [`docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md`](docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md)
- Architecture invariant (engine no-disk-I/O): [`docs/architecture.md`](docs/architecture.md)
