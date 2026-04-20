---
title: Universal patterns via tag-based SessionStart injection
type: feat
status: active
date: 2026-04-20
origin: docs/brainstorms/2026-04-20-universal-patterns-requirements.md
---

# Universal patterns via tag-based SessionStart injection

## Enhancement Summary

**Deepened on:** 2026-04-20 **Sections enhanced:** all units, plus new Security Considerations,
expanded acceptance criteria (R8-R11), and revised Dependencies & Risks. **Review agents used:**
architecture-strategist, performance-oracle, security-sentinel, code-simplicity-reviewer,
agent-native-reviewer, data-integrity-guardian, data-migration-expert, spec-flow-analyzer,
pattern-recognition-specialist.

### Key improvements integrated

1. **Architecture: partition moves into the search layer.** The deferred `top_k` decision is
   resolved by returning `PartitionedResults` from `search_with_threshold` (Unit 3 refactor). Keeps
   `cmd_search` and the hook in lockstep; eliminates layer-straddling.
2. **Security: path-traversal guard at SessionStart.** `validate_within_dir` wraps the new body
   re-read so DB-tampering can't turn universal patterns into an arbitrary-file-read primitive into
   agent context (Unit 2).
3. **Schema-mismatch detection moves to startup.** `KnowledgeDB::open` probes
   `PRAGMA table_info(chunks)` and emits the `lore ingest --force` advisory once at startup, instead
   of relying on per-event hook stderr that users won't see (Unit 1).
4. **Agent-native parity: `is_universal` on `search_patterns` results and `[universal]` marker on
   `lore list`.** Cuts the `universal_pattern_count` field on `lore_status` (YAGNI — no consumer) in
   favour of two surfaces that close the authoring/agent feedback loop (Unit 4).
5. **Body-size guardrail promoted to Unit 1.** Per-pattern body-size warning at ingest is the only
   real guard against the dominant cost vector. Count guardrail (>3 patterns) stays.
6. **Reuse `PatternSummary`** instead of inventing a new `UniversalPattern` struct.
7. **Schema constraint hardened** with `CHECK (is_universal IN (0, 1))`.
8. **Five additional tests pinned** for security, integrity, and spec-flow gaps surfaced during
   review.
9. **CHANGELOG copy rewritten** to be explicit about destructive rebuild and Ollama time budget.

### New considerations discovered (deferred to follow-up)

- **`full_ingest` atomicity gap.** `clear_all` then per-file embed-and-insert is non-transactional;
  an Ollama outage mid-batch leaves the DB in worse state than before. Pre-existing risk this plan
  exposes more loudly. Documented in Dependencies & Risks; recommended follow-up todo:
  backup-and-swap around `clear_all`.
- **Per-heading universal granularity.** Current architecture has `is_universal` at file granularity
  (file-level frontmatter tag). Future patterns where only one heading should be pinned cannot be
  represented. Deferred — no current authoring need.
- **Cycle-based dedup TTL composition.** Universal is the always-on degenerate case (TTL=0) of the
  future cycle-based primitive. The partition-in-search-layer choice (point #1) eases that future
  refactor; the binary `is_universal` flag will need to generalise into a per-chunk re-injection
  policy when TTL lands.

## Overview

Add a new always-on tier to lore's pattern injection: patterns whose `tags:` frontmatter list
contains `universal` get their full body emitted at every `SessionStart` (and re-emitted at
`PostCompact`) under a dedicated `## Pinned conventions` section, AND bypass the `PreToolUse` dedup
filter so they re-inject on every relevant tool call. The relevance gate stays intact — universal
patterns still must be ranked above `min_relevance` for the current tool call's extracted FTS5
query.

The feature closes the orthogonal half of runtime discoverability that the shipped coverage-check
skill (PR #32) does not address: process-level conventions (commit rules, push discipline, branch
naming, PR etiquette) whose value comes from continuous reinforcement, not one-shot relevance.

## Problem Statement

The motivating incident is captured verbatim in `ROADMAP.md` lines 10-19. Summary: an agent ran
plain `git push` mid-session, hit `main`-protection rejection, and only then realised the
`workflows/git-branch-pr.md` "Pushing" section already prescribed `git push origin HEAD`. The
pattern was discoverable (relevance 1.0), the hook injected it on the first git command of the
session, and session deduplication then correctly suppressed it on every subsequent git call —
including the failing push.

A reinforcing learning from
`docs/solutions/integration-issues/additional-context-timing-in-pretooluse-hooks-2026-04-02.md`
makes this worse than it appears: SessionStart-injected content has the same one-tool-call delay as
PreToolUse content. The agent doesn't see SessionStart patterns during the first tool composition.
So a SessionStart-only injection of universal patterns wouldn't fix the motivating incident either —
it would only help with the post-compaction case. The PreToolUse re-injection on every relevant tool
call is what actually closes the gap. This validates the brainstorm's choice of "SessionStart +
bypass PreToolUse dedup" over "SessionStart only".

## Proposed Solution

A single new `is_universal INTEGER NOT NULL DEFAULT 0 CHECK (is_universal IN (0, 1))` column on the
`chunks` table, populated at ingest time from the frontmatter `tags:` list. Three injection-pipeline
changes consume it, plus three visibility surfaces:

1. **At `SessionStart` and `PostCompact`** — `format_session_context` prepends a
   `## Pinned conventions` section containing the full body of each universal-tagged pattern
   (re-read from the source markdown file under `knowledge_dir`, with a `validate_within_dir`
   containment check), above the existing pattern title index. The header is omitted entirely when
   no universal patterns exist.
2. **Inside `search_with_threshold`** — search results return as a
   `PartitionedResults { universal: Vec<SearchResult>, ranked:
   Vec<SearchResult> }` value with
   the `top_k` cap applied to `ranked` only. The hook handler concatenates them; `cmd_search`
   flattens. This keeps the two callers in lockstep.
3. **At `PreToolUse`** — the universal slice bypasses the dedup filter (read-side: the filter
   ignores `is_universal = true` chunks when checking membership). The non-universal slice flows
   through the existing dedup pipeline.
4. **At ingest time** — three advisories emitted to stderr:
   - Always: a `Universal patterns: N` summary line after the existing summary (zero or more — keeps
     the count discoverable for new authors with one universal pattern).
   - When N > 3: name the patterns and prompt deliberation.
   - When any single universal pattern body exceeds ~1KB: a per-pattern advisory (the dominant
     token-cost vector at runtime).
   - When a tag's lowercased form equals `universal` but the exact match doesn't (e.g. `Universal`,
     `universally`): a near-miss advisory suggesting the author check spelling.
5. **At `KnowledgeDB::open`** — a `PRAGMA table_info(chunks)` probe detects the missing
   `is_universal` column on databases that pre-date this feature and emits a single, friendly
   `lore ingest --force` advisory once at startup.
6. **MCP `search_patterns` results** — gain an `is_universal: bool` field per result, so MCP-driven
   agents can reason about which results would re-inject.
7. **`lore list` CLI** — universal patterns get a `[universal]` suffix in the output, closing the
   authoring read-back loop without requiring a session restart.
8. **`LORE_DEBUG=1` traces** — distinguish universal vs dedup-filtered injections for runtime
   debugging of the second-mutation-route hazard.

Schema change is applied via the standard `CREATE TABLE IF NOT EXISTS` block with no migration code:
per project convention (`rust/sqlite.md` → "Treat the database as a derived artefact"), schema
changes take effect by re-ingesting. The CHANGELOG entry is explicit that this is a breaking schema
change requiring `lore ingest
--force`, that the rebuild is destructive, and that re-embedding
through Ollama takes proportional time.

## Technical Considerations

### Schema and ingest

The `is_universal` column lives on the `chunks` table, not on a (currently non-existent) patterns
table. Reasons:

- Search results return chunks (`SearchResult` at `src/database.rs:63-72`).
- The dedup filter operates on chunk IDs.
- The `chunks_by_sources` source-expansion step at `src/hook.rs:195` already returns chunks.
  Carrying `is_universal` on the chunk row means no JOIN is required at any consumer site.

Column definition:

```sql
is_universal INTEGER NOT NULL DEFAULT 0 CHECK (is_universal IN (0, 1))
```

The `CHECK` constraint is defense-in-depth: the read path partitions on truthy values, and a stray
`2` from a future refactor would silently treat the chunk as universal. The check costs nothing and
makes both author and refactor mistakes fail at INSERT time.

The frontmatter parser at `src/chunking.rs:181-219` (`extract_frontmatter_tags`) already extracts
the comma-joined tags string. Add a sibling helper
`frontmatter_has_tag(content, "universal")
-> bool` and propagate `is_universal: bool` through the
`Chunk` struct (`src/chunking.rs:11-26`) and into the database via `insert_chunk`
(`src/database.rs:187-232`).

**Do not add a `Default` impl or builder for `Chunk`.** All ~14 test fixtures and the MCP
`add_pattern` path construct `Chunk` directly. A `Default` impl would silently zero `is_universal`,
re-introducing the silent-default hazard for future call sites. Rust's compile error on the missing
field is the safety property; preserve it.

### Dedup bypass: read-side filter, not write-side

Per the explicit guidance in
`docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`:

> "If 'universal' patterns must bypass the dedup filter, you cannot just 'skip writing the IDs'; you
> must skip the _read_ (`seen.contains`) for universal IDs."

The dedup filter at `src/hook.rs:528-563` (`dedup_filter_and_record`) is modified to inspect each
result's `is_universal` flag and treat universal chunks as not-in-set regardless of the file's
actual contents. Universal chunks are still recorded as seen — this keeps the dedup file as a
faithful "what has been injected this session" log and is defensive against any future code path
that might consult it for purposes other than gating PreToolUse.

(Write-side filtering — never recording universal IDs — would also work in isolation but is more
brittle to future changes and contradicts the documented learning.)

### Partition belongs in `search_with_threshold`, not the hook handler

Following architecture-review guidance: `search_with_threshold` at `src/hook.rs:347-412` is shared
by `cmd_search` and the PreToolUse handler — the docstring at `:347-350` explicitly notes it exists
to prevent drift between those two paths. Putting the universal/non-universal partition inside the
hook handler reintroduces drift on the layer above.

The function returns a structured value:

```rust
struct PartitionedResults {
    universal: Vec<SearchResult>,
    ranked: Vec<SearchResult>, // top_k cap applied here only
}
```

`cmd_search` flattens the two slices for CLI output (universal first, then ranked).
`handle_pre_tool_use` keeps them separated, passes only `ranked` through `dedup_filter_and_record`,
then concatenates for formatting. This resolves the deferred `top_k` post-expansion question from
the brainstorm: the cap stays where it is today (inside `db.search_hybrid`), applied to the `ranked`
slice only.

### SessionStart body source: re-read from disk with containment guard

The chunk table stores post-frontmatter-strip content in chunks; the authoritative pattern body
lives in the source markdown file. Pattern bodies for the `## Pinned conventions` section are
re-read fresh from `knowledge_dir.join(source_file)` at SessionStart time, using the
`std::fs::read_to_string` idiom already established at `src/ingest.rs:922,930,988,996,1063` and
`src/server.rs:2198,2239`.

**Security guard:** the re-read is wrapped in `validate_within_dir` (`src/ingest.rs:1095-1105`) so a
tampered DB row containing `source_file = '../../../etc/passwd'` cannot turn universal patterns into
an arbitrary-file-read primitive that lands in agent context. The existing `source_file` derivation
at ingest time (via `path.strip_prefix(knowledge_dir)` at `src/ingest.rs:552, 642`) already
guarantees relative paths under normal flows; this guard hardens against direct DB tampering.

SessionStart fires once per session plus once per PostCompact — re-reading 1-3 small markdown files
plus a per-file canonicalisation is sub-millisecond on local FS. **NFS caveat:** networked
filesystems add ~5-50ms round-trip per file; encrypted FS (LUKS, FileVault) is fine. The dominant
SessionStart cost remains the existing `git rev-parse` subprocess at `src/hook.rs:437` (~10-30ms
typical), so the marginal cost of universal-body reads is noise.

### Schema-mismatch detection at startup

`KnowledgeDB::open` (in `src/database.rs`) probes `PRAGMA table_info(chunks)` once on connection and
checks for the `is_universal` column. If absent on a non-empty database, emit a single friendly
stderr message and return an error rather than letting individual SELECTs fail later:

```
lore: This database predates the universal-patterns feature.
Run `lore ingest --force` to rebuild the index with the new schema.
This is expected after upgrading; see CHANGELOG for details.
```

This converts a confusing per-event error chain (`no such column:
is_universal` at first
SessionStart, first `lore search`, etc.) into a single actionable startup message at every entry
point. The `PRAGMA table_info` call costs microseconds.

### Composition cascade hazard

Per
`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`,
the dedup-bypass path is a "second mutation route over shared state" — exactly the shape of issue
that learning warns about. The hazard-pin test required by that learning is included in Unit 3's
test list:
**`PreToolUse: universal pattern still injects after PostCompact has truncated and re-emitted SessionStart`**.

### Tag removal reconciliation (both ingest paths)

Per
`docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`,
the `universal` tag must be honoured in both directions: removing it from a pattern's frontmatter
must take effect without `--force` on the next delta ingest. This falls out for free since
frontmatter changes are content changes that delta ingest detects; the chunks get re-inserted with
the updated `is_universal` value. **Both ingest paths must be tested**: `delta_ingest` (the standard
reconciliation path) AND `ingest_single_file` (the fast feedback loop used by the coverage-check
skill).

### Coverage-check interaction

The shipped coverage-check skill (PR #32) measures `PreToolUse` discoverability by simulating tool
calls and piping them through `lore extract-queries`. Universal patterns by definition can re-inject
without passing search relevance, so coverage-check has no special signal for them — but they still
pass through `extract_query` like any other pattern. Position: **no behaviour changes to
coverage-check in this feature**, but the report adds a `[universal — bypasses PreToolUse dedup]`
marker next to per-pattern rows so authors don't pointlessly chase low coverage scores on patterns
that bypass the very channel coverage-check measures.

### Partition implementation (zero-clone)

The partition step uses `Vec::into_iter().partition(|r| r.is_universal)` to move `SearchResult`
values without cloning their `String` body fields. Filter-and-cloned alternatives would copy bodies
twice per call. The cost difference is small per call but compounds with body size and result count.

## System-Wide Impact

### Interaction graph

`SessionStart` event arrives → `handle_session_start` (`src/hook.rs:132`) → `format_session_context`
(`src/hook.rs:425`) → **NEW** `db.universal_patterns()` → re-reads each body file (with
`validate_within_dir` guard) → emits `## Pinned conventions` section

- existing index → `HookOutput::SystemMessage`. Same chain runs at `PostCompact`
  (`src/hook.rs:268`).

`PreToolUse` event arrives → `handle_pre_tool_use` (`src/hook.rs:152`) → `extract_query` →
`search_with_threshold` (`src/hook.rs:351`, **MODIFIED**: returns `PartitionedResults`) →
`chunks_by_sources` source expansion runs internally (`src/hook.rs:195`) → returns
`PartitionedResults` with `ranked` capped at `top_k` and `universal` uncapped → handler passes
`ranked` to `dedup_filter_and_record` (with the read-side universal exemption built into the filter)
→ format both lists → concatenate into `additionalContext` → emit. `LORE_DEBUG=1` traces distinguish
`[universal injected]`, `[ranked injected]`, `[ranked filtered by dedup]`.

`KnowledgeDB::open` → **NEW** `PRAGMA table_info(chunks)` probe → if `is_universal` absent and table
non-empty, emit friendly stderr + return error.

### Error & failure propagation

- **`db.universal_patterns()` query failure** at SessionStart → `format_session_context` already
  returns `anyhow::Result`; bubble up, hook handler logs and exits 0 (per existing "hook never
  breaks the agent" contract at `src/main.rs:523-529`). The user gets a session without pinned
  conventions; no agent disruption.
- **Pattern body file missing or unreadable** at SessionStart → log to stderr via `eprintln!`, omit
  that one pattern from the pinned section, continue. The pattern's title still appears in the
  existing index.
- **Pattern body fails `validate_within_dir`** (DB tampering) → log with the offending path to
  stderr, omit the pattern, continue.
- **`is_universal` column missing** (user upgraded binary without `lore ingest --force`) → caught at
  `KnowledgeDB::open` startup, emits the friendly advisory once. Hook entry points still hit the
  fail-safe error contract.

### State lifecycle risks

- The dedup file is per-session, ephemeral, and truncated at PostCompact — no cross-session leakage.
  Universal-pattern entries written to it are ignored on read (the read-side filter), so tagging or
  untagging universal patterns mid-session works correctly **for re-injection decisions**.
- **Mid-session untagging caveat:** the `## Pinned conventions` body already emitted in the current
  session's SessionStart payload has been consumed into the agent's conversation context. Untagging
  mid-session removes the pattern from future re-injection but does NOT retract context the agent
  has already seen. Effect is fully realised at the next session boundary.
- The `is_universal` column on chunks is set at insert time. No backfill, no migration. Tag changes
  take effect when the chunk is re-inserted (delta ingest sees the frontmatter change as a content
  change).
- **Backwards compatibility (downgrade safety):** all existing SELECTs in `src/database.rs` use
  explicit column lists (verified by data-migration review — zero `SELECT *` in `src/`). A user who
  runs the new binary's `lore ingest --force` and then downgrades gets a working old binary against
  a DB with one ignored extra column.
- The `## Pinned conventions` section content is computed fresh at every SessionStart from the live
  source files — no stale snapshot.

### API surface parity

- **`lore search` CLI** — flattens the `PartitionedResults` returned by `search_with_threshold`
  (universal first, then ranked) so output stays a flat list. No JSON schema change.
- **`search_patterns` MCP tool** — gains `is_universal: bool` per result. MCP-driven agents can
  reason about which results would re-inject. `tools_list_returns_all_five_tools` insta snapshot
  regenerated.
- **`lore_status` MCP tool** — **unchanged** (the proposed `universal_pattern_count` field is cut as
  YAGNI; the `search_patterns` and `lore list` surfaces above carry the same information).
- **`lore list` CLI** — universal patterns get a `[universal]` suffix after the existing tag
  display, e.g.:

  ```
  Workflow conventions [conventions, git, commit, push] [universal]
  ```

  This closes the authoring read-back loop. Plain titles for non-universal patterns are unchanged.

### Integration test scenarios

1. **End-to-end happy path**: ingest a pattern tagged `universal` → SessionStart returns the pinned
   section with the full body → PreToolUse for a relevant tool call returns the body inside
   `additionalContext`, even after the same call has fired three times in a row.
2. **Tag removal mid-session (delta path)**: pattern tagged universal, ingest, run a session, untag
   the pattern, re-run `lore ingest` (delta), start a new session — the pattern is no longer in the
   pinned section.
3. **Tag removal (single-file ingest path)**: same scenario but using `lore ingest --file` for the
   re-ingest. Sibling test for completeness.
4. **Tag addition with no body change (delta)**: existing pattern, author edits frontmatter only to
   add `universal`, runs `lore ingest` (delta) — chunks re-inserted with `is_universal = true`, next
   SessionStart includes the pinned body.
5. **Composition cascade hazard pin**: PreToolUse fires three times for the same query, then
   PostCompact truncates and re-emits SessionStart content, then PreToolUse fires again — universal
   pattern is in `additionalContext` on every PreToolUse call.
6. **Schema mismatch detection**: open a database that pre-dates this feature (no `is_universal`
   column), trigger `KnowledgeDB::open` → user-facing error message names `lore ingest --force` as
   the resolution.
7. **Ingest warning at boundary**: ingest 3 universal patterns → summary line "Universal patterns:
   3" but no >3 advisory. Ingest 4 universal patterns → summary line + stderr warning naming all
   four.
8. **Body-size warning**: ingest a universal pattern whose body exceeds 1KB → per-pattern stderr
   advisory naming the file.
9. **Tag-misspelling near-miss**: ingest a pattern whose tags include `Universal` (capital U) →
   near-miss advisory; ingest unaffected.
10. **Path-traversal guard**: tamper a chunk row to set `source_file = '../../../etc/passwd'`,
    trigger SessionStart → pattern omitted, stderr log, no file read attempted outside
    `knowledge_dir`.
11. **Empty pinned section omission**: ingest no universal patterns → SessionStart payload contains
    no `## Pinned conventions` header at all (not an empty section).
12. **Low-relevance universal does NOT inject**: ingest a `git`-tagged universal pattern, fire
    PreToolUse for `Edit Cargo.toml` whose extracted query has no overlap with `git`-related tokens
    → universal pattern absent from response (R4 negative).

## Acceptance Criteria

### Functional

- [ ] **R1** A pattern with `tags:` containing `universal` (case-sensitive, exact match) is treated
      as universal. Other tags coexist normally. A near-miss tag (lowercased form equals `universal`
      but exact match fails) emits a stderr advisory at ingest.
- [ ] **R2** SessionStart payload contains a `## Pinned conventions` section above the existing
      `Available patterns:` index, with each universal pattern's full body in source-file order. The
      header is omitted entirely when no universal patterns exist (not emitted as an empty section).
- [ ] **R3** PostCompact re-emits the same SessionStart payload (`##
      Pinned conventions`
      section included) — falls out of going through `format_session_context`.
- [ ] **R4** PreToolUse: universal patterns matching the current query above `min_relevance` are
      present in `additionalContext` on every call, not just the first. Universal patterns NOT
      matching the current query are absent.
- [ ] **R5** PreToolUse: universal results are additive — non-universal results are still up to
      `top_k`, regardless of how many universal results are present.
- [ ] **R6** `lore ingest` always emits a `Universal patterns: N` summary line after the existing
      summary lines (zero or more). When N > 3, an additional advisory line names the patterns and
      prompts deliberation. When any single universal pattern body exceeds ~1KB, a per-pattern
      body-size advisory names the file. Ingest exit code unchanged.
- [ ] **R7** `docs/pattern-authoring-guide.md` gains a "When to use the universal tag" subsection,
      sibling to "Vocabulary Coverage Technique" and "Tag Strategy". Includes guidance on the
      `## Pinned
      conventions` ↔ "universal" naming divergence (the `tags:` value is
      `universal`; the section header in SessionStart is `## Pinned
      conventions` — chosen
      because it reads better as user-facing copy).
- [ ] **R8** `search_patterns` MCP tool results include `is_universal:
      bool` per result. The
      `tools_list_returns_all_five_tools` insta snapshot is regenerated to reflect the schema
      change.
- [ ] **R9** `lore list` CLI output marks universal patterns with a `[universal]` suffix after the
      existing tag display.
- [ ] **R10** `LORE_DEBUG=1` traces distinguish `[universal injected]`, `[ranked injected]`, and
      `[ranked filtered by dedup]` so the dedup-bypass mutation route is traceable.
- [ ] **R11** Pattern body files referenced from `is_universal` chunks are passed through
      `validate_within_dir` before re-read at SessionStart. Tampered DB rows naming paths outside
      `knowledge_dir` cause the pattern to be omitted with a stderr log; no read attempt is made.

### Non-functional

- [ ] SessionStart wall-clock time impact: < 50ms additional with up to five universal patterns of
      typical body size (~2KB each) on local filesystems. NFS-backed `knowledge_dir` may add
      25-250ms aggregate due to per-file round-trip latency; this is documented but not mitigated.
- [ ] PreToolUse wall-clock time impact: zero measurable difference for sessions with no universal
      patterns. With universal patterns, the partition + concatenation step is bounded by result
      count (microseconds, sub-millisecond using `into_iter().partition`).
- [ ] Binary size impact: negligible. No new dependencies.

### Quality gates

- [ ] All new code paths have unit tests (database, chunking, hook partition, dedup-read filter,
      schema probe, `validate_within_dir` guard).
- [ ] All new SessionStart and PreToolUse code paths have integration tests in `tests/hook.rs` using
      `assert_cmd::Command` and the existing `FakeEmbedder` harness. Test names follow the existing
      `hook_*` prefix convention for `tests/hook.rs` and the bare snake-case predicate convention
      for `src/*::tests` modules.
- [ ] `just ci` clean: `dprint check`, `cargo clippy --all-targets -- -D
      warnings`,
      `cargo test --all-targets`, `cargo deny check`, `cargo doc --no-deps`.
- [ ] `tools_list_returns_all_five_tools` insta snapshot regenerated for R8.

## Implementation Units

### Unit 1 — Schema, chunking, ingest, startup probe (foundation)

**Goal:** persist `is_universal` per chunk; surface ingest-time advisories; detect schema mismatch
at startup.

Touched files:

- `src/database.rs` — add column to `CREATE TABLE chunks` (line 116-125) with
  `CHECK (is_universal IN (0, 1))`. Update `Chunk` writes in `insert_chunk` (lines 187-232), add
  `is_universal` to `SearchResult` (lines 63-72) and to the SELECT in `search_fts`, `search_vector`,
  `chunks_by_sources`. Add new method `universal_patterns() ->
  Result<Vec<PatternSummary>>`
  returning the existing `PatternSummary` type (`src/database.rs:81-86`) for
  `WHERE is_universal = 1` — reuses the existing struct rather than introducing a new
  `UniversalPattern` type. Add `KnowledgeDB::open` probe via `PRAGMA table_info(chunks)`: if
  `is_universal` absent and `chunks` non-empty, emit the friendly `lore ingest --force` advisory and
  return error.
- `src/chunking.rs` — add `frontmatter_has_tag(content: &str, tag: &str) ->
  bool` next to
  `extract_frontmatter_tags` (line 181). Add `is_universal: bool` to `Chunk` (line 11-26) —
  **explicitly do NOT add `Default` impl or builder**; preserve compile-error wall for all call
  sites. Set `is_universal` in `chunk_by_heading` (line 43) and `chunk_as_document` (line 138).
- `src/ingest.rs` — add `universal_count: usize` and `oversized_universal_bodies: Vec<String>` and
  `near_miss_universal_tags: Vec<String>` to `IngestResult` (line 69-81). Increment / append in
  `full_ingest`, `delta_ingest`, `ingest_single_file` whenever a chunk with `is_universal = true` is
  inserted, when its body exceeds 1024 bytes, or when its frontmatter tags include a near-miss form.
- `src/main.rs` — extend `print_ingest_summary` (line 407-454) to emit:
  - Always: `Universal patterns: N` line (zero or more).
  - When N > 3: advisory with names.
  - For each oversized universal body: per-pattern advisory.
  - For each near-miss tag: spelling advisory.

Tests (test-first, in `src/database.rs::tests`, `src/chunking.rs::tests`,
`tests/single_file_ingest.rs`):

- `frontmatter_has_tag_matches_exact_tag_in_list` — `tags: [foo,
  universal, bar]` → `true`.
  `tags: [foo, universally]` → `false`. `tags: [Universal]` → `false` (case-sensitive).
- `frontmatter_has_tag_does_not_match_quoted_tag_with_internal_comma` — `tags: ["universal,thing"]`
  → `false` (negative test for the hand-rolled parser's edge case).
- `chunk_by_heading_propagates_universal_flag` — frontmatter contains `universal` tag → emitted
  chunks all have `is_universal = true`.
- `insert_and_select_chunk_round_trips_is_universal` — insert a chunk with `is_universal = true`,
  query back, flag preserved.
- `chunk_check_constraint_rejects_invalid_is_universal_value` — direct SQL
  `INSERT ... VALUES (..., 2)` fails with constraint violation.
- `universal_patterns_returns_only_universal_tagged` — insert three patterns (one universal, two
  not), `universal_patterns()` returns one row.
- `knowledge_db_open_probe_detects_missing_is_universal_column` — open a DB created with the old
  schema (manually), verify open returns the friendly error.
- `ingest_universal_pattern_sets_chunk_flag` — ingest a single file with `universal` tag, verify
  chunks have `is_universal = true`.
- `ingest_warns_at_four_universal_patterns` — ingest four files all tagged universal, stderr
  contains the `>3` advisory line naming all four. Ingest exit code 0.
- `ingest_does_not_warn_at_three_universal_patterns` — threshold pin for the >3 advisory (the
  always-on summary line still emits).
- `ingest_emits_per_pattern_body_size_warning_above_threshold` — ingest a universal pattern with
  body > 1024 bytes → per-pattern advisory.
- `ingest_emits_near_miss_advisory_for_capitalised_universal_tag` — pattern tagged `Universal` →
  near-miss advisory.

**Done when:** all tests pass, `just ci` clean.

### Unit 2 — SessionStart pinned section emission with security guard

**Goal:** universal pattern bodies appear in `## Pinned conventions` at SessionStart and
PostCompact, with path-traversal protection.

Touched files:

- `src/hook.rs` — modify `format_session_context` (line 425-459) to call `db.universal_patterns()`,
  validate each pattern's `source_file` via `validate_within_dir` against `knowledge_dir`, re-read
  the body via `std::fs::read_to_string(canonical_path)`, and prepend a `## Pinned
  conventions`
  section before the existing `Available patterns:` line (line 448). Omit the entire section header
  when the universal-pattern list is empty.
- The existing `PostCompact` handler at `src/hook.rs:268` already calls `format_session_context` —
  no change required (R3 falls out for free).

Tests (in `src/hook.rs::tests` and `tests/hook.rs`):

- `session_start_with_no_universal_patterns_omits_pinned_section` — payload contains
  `Available patterns:` but no `## Pinned conventions` header.
- `session_start_with_one_universal_pattern_emits_pinned_section_above_index` — payload has
  `## Pinned conventions` followed by the body, then `Available patterns:` index.
- `session_start_universal_body_read_from_source_file_not_chunks` — pin that the emitted body
  matches the source markdown file content verbatim (or with documented post-processing if any).
- `session_start_missing_universal_body_file_logs_and_continues` — delete the source file after
  ingest, trigger SessionStart → other universal patterns still emit, missing one is omitted, stderr
  has a log line.
- `session_start_path_traversal_attempt_omits_pattern_and_logs` — tamper a chunk row to set
  `source_file = '../../../etc/passwd'`, trigger SessionStart → pattern omitted, stderr log, no file
  read attempted outside `knowledge_dir`.
- `post_compact_re_emits_pinned_section` — PostCompact payload matches SessionStart payload
  structure.
- `hook_session_start_e2e_with_universal_pattern` — full `assert_cmd::Command` test with a tempdir
  knowledge base.

**Done when:** all tests pass, `just ci` clean.

### Unit 3 — Search-layer partition + dedup-read bypass + LORE_DEBUG traces

**Goal:** `search_with_threshold` returns `PartitionedResults`; universal chunks re-inject on every
relevant PreToolUse call; non-universal chunks still respect `top_k` and dedup.

Touched files:

- `src/hook.rs` — refactor `search_with_threshold` (line 351-412) to return
  `PartitionedResults { universal: Vec<SearchResult>, ranked:
  Vec<SearchResult> }`. Apply `top_k`
  cap to `ranked` only (matches today's behaviour: cap stays inside `db.search_hybrid` at line 383,
  applied to the initial result set). Partition via `Vec::into_iter().partition(|r| r.is_universal)`
  for zero-clone string movement.
- `src/hook.rs` — modify `handle_pre_tool_use` (line 152-252) to consume `PartitionedResults`. Pass
  `ranked` to `dedup_filter_and_record` (line 211); concatenate `universal ++ filtered_ranked`
  before formatting via `format_imperative` (line 237).
- `src/hook.rs` — modify `dedup_filter_and_record` (line 528-563): the read-side filter ignores
  `is_universal = true` chunks when checking membership. (Write-side still records them — defensive
  consistency with the existing dedup-file-as-injection-log semantic.)
- `src/main.rs::cmd_search` — flatten `PartitionedResults` for CLI output: universal first, then
  ranked. JSON output preserves both groups under named keys for agent consumption.
- `src/hook.rs` and `src/main.rs::cmd_search` — add `lore_debug!` traces distinguishing
  `[universal injected]`, `[ranked injected]`, and `[ranked filtered by dedup]`.

Tests (in `src/hook.rs::tests` and `tests/hook.rs`):

- `search_with_threshold_returns_partitioned_results` — mixed universal
  - non-universal results: returns both groups separately, `ranked` capped at `top_k`, `universal`
    uncapped.
- `pre_tool_use_universal_chunk_present_on_first_and_third_call` — three identical PreToolUse events
  for the same matching query → universal chunk present in all three responses.
- `pre_tool_use_non_universal_chunk_present_on_first_call_only` — same shape, non-universal chunk →
  present only on the first call (dedup works as today).
- `pre_tool_use_universal_chunk_absent_when_query_does_not_match` — universal `git`-tagged chunk,
  PreToolUse for `Edit Cargo.toml` whose extracted query has no overlap → universal chunk NOT
  present (R4 negative pin).
- `pre_tool_use_universal_chunk_absent_when_below_min_relevance` — universal pattern, query that
  matches but with relevance below `config.search.min_relevance` → universal chunk NOT present
  (verifies the relevance gate is not bypassed).
- `pre_tool_use_universal_chunk_additive_to_top_k` — three universal-matching chunks plus six
  non-universal matching chunks, `top_k = 5` → response contains 3 universal + 5 non-universal = 8
  total, not 5.
- `pre_tool_use_dedup_file_records_universal_chunks` — pin the read-side semantic: universal IDs ARE
  written to the dedup file but ignored on read.
- **Composition cascade hazard pin:**
  `hook_pre_tool_use_universal_persists_after_post_compact_truncation` — fire PreToolUse 3x → fire
  PostCompact (truncates dedup) → fire PreToolUse → universal chunk still present.
- **Tag removal reconciliation pins (both ingest paths):**
  - `hook_session_start_after_universal_tag_removal_via_delta_ingest_omits_pattern`
  - `hook_session_start_after_universal_tag_removal_via_single_file_ingest_omits_pattern`
- **Tag addition reconciliation pin:**
  `hook_delta_ingest_detects_universal_tag_added_with_no_body_change` — frontmatter-only edit adding
  the tag; body unchanged; delta ingest still picks it up (because frontmatter is part of file
  content).

**Done when:** all tests pass, `just ci` clean.

### Unit 4 — Agent-native parity, documentation, ROADMAP

**Goal:** make the feature observable from MCP and CLI; document for authors; keep coverage-check
honest.

Touched files:

- `src/server.rs` — add `is_universal: bool` to `search_patterns` result schema. Regenerate the
  `tools_list_returns_all_five_tools` insta snapshot. **Do not** add `universal_pattern_count` to
  `lore_status` (cut as YAGNI per simplicity review).
- `src/main.rs::cmd_list` — append `[universal]` suffix to lines whose pattern has
  `is_universal = true` chunks. Detect via a `db.list_patterns_with_universal_flag()` method (or
  extend `list_patterns` to carry the flag).
- `integrations/claude-code/skills/coverage-check/SKILL.md` — add a one-line note in the
  report-rendering step: when a pattern is universal, append
  `[universal — bypasses PreToolUse dedup]` next to the pattern row in the report.
- `docs/pattern-authoring-guide.md` — new subsection "When to use the universal tag" between "Tag
  Strategy" and the existing checklist. Content per R7: criteria (process-level, value comes from
  continuous reinforcement, body small enough to justify per-call re-injection), worked example
  using the git-workflow incident, explicit warning that body size compounds (a 2KB universal
  pattern matched 50 times in a session = 100KB of repeated context; a 5KB pattern with three
  matches across 50 calls is 750KB), one paragraph on the `## Pinned conventions` header naming
  choice (`tags:` value is `universal`; section header is `## Pinned conventions` because it reads
  better as user-facing copy), and explicit instruction to `lore ingest` after retagging.
- `docs/hook-pipeline-reference.md` — extend the Skills/Hook section to describe the
  `## Pinned conventions` SessionStart structure, the `is_universal` chunk attribute, and the
  partitioned search result shape returned by `search_with_threshold`.
- `README.md` — one sentence in the Use with Claude Code section mentioning the universal tag tier,
  with a pointer to the authoring guide subsection.
- `CHANGELOG.md` — entry under unreleased:

  > **feat: Universal patterns** — patterns tagged `universal` get always-on injection at
  > SessionStart and bypass PreToolUse dedup. **Breaking for existing knowledge bases:** the chunks
  > table gains an `is_universal` column. After upgrading, run `lore ingest --force` once before
  > your next Claude Code session — `lore` will refuse to start otherwise with a friendly advisory.
  > `--force` is a destructive rebuild that re-embeds every chunk through Ollama; budget time
  > accordingly. The new tag is documented in `docs/pattern-authoring-guide.md`.

- `ROADMAP.md` — move the "Universal patterns via tag-based SessionStart injection" entry from "Up
  Next" to "Completed" with a reference to this plan. Note: the existing
  `doc/roadmap-coverage-check-completed` branch (`c39fc66`) also touches this file; rebase
  considerations apply (see "Dependencies" below).

Tests:

- `tests/coverage_check_skill_parses` updated to allow the new
  `[universal — bypasses PreToolUse dedup]` marker substring as a documented output element.
- `server::search_patterns_results_include_is_universal_flag` — field is present on each result row.
- `cmd_list_marks_universal_patterns_with_suffix` — integration test via `assert_cmd::Command`
  confirming the `[universal]` suffix.

**Done when:** all tests pass, `just ci` clean, docs render cleanly via `dprint fmt`.

## Alternative Approaches Considered

- **SessionStart-only injection (no PreToolUse re-injection)** — rejected. SessionStart has the same
  one-tool-call delay as PreToolUse, so the motivating incident would still occur. Documented in
  brainstorm Q1.
- **Always-inject regardless of relevance** — rejected. With one ~500-token universal pattern across
  100 tool calls per session, that's 50K tokens spent on pattern re-emission. The relevance gate
  keeps token cost bounded by genuine applicability.
- **Dedicated `universal: true` frontmatter field instead of a tag** — deferred. Tag is the simpler
  authoring surface today; switching later is reversible. Documented in brainstorm scope boundaries.
- **Counting universal patterns against `top_k`** — rejected. Tagging five patterns would kill all
  other PreToolUse injection — the feature would become a denial-of-service vector against itself.
  Documented in brainstorm Q4.
- **Write-side dedup filter (don't record universal chunks)** — rejected per institutional learning
  at `session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`. Read-side filter is the
  documented correct approach.
- **`ALTER TABLE chunks ADD COLUMN is_universal` migration** — rejected per `rust/sqlite.md`: "Treat
  the database as a derived artefact... migration tooling is unnecessary; schema changes are applied
  by re-ingesting." The CHANGELOG documents the `lore ingest --force` requirement for existing
  users; the startup `PRAGMA` probe makes the requirement self-explanatory at first run.
- **Storing universal pattern bodies in the DB to avoid filesystem reach in the hook layer**
  (architecture-strategist suggestion) — rejected. Adds duplication (chunk-stripped body + raw body)
  for marginal gain. The source markdown file remains the canonical body source; re-reading from
  disk preserves single-source-of-truth and matches how authors edit. The `validate_within_dir`
  guard addresses the security concern that motivated the suggestion.
- **Per-heading universal granularity** (only one heading in a pattern is pinned) — deferred.
  Current architecture has tags at file granularity; no current authoring need for per-heading
  universals. Worth revisiting if a real use case appears.
- **Putting partition in the hook handler instead of `search_with_threshold`** — rejected per
  architecture review. `search_with_threshold` is shared by `cmd_search` and the hook handler;
  partitioning in the handler reintroduces drift the function exists to prevent.
- **`ChunkMetadata` grouping struct on `SearchResult` to absorb future flags** — rejected as
  premature abstraction. Revisit when a second flag is in the pipe.
- **Namespaced reserved tag (`lore:universal` or `lore-pinned`)** — noted as future consideration.
  Bare `universal` is simpler today and the tag-collision risk is low (the >3 advisory and the
  per-pattern advisories surface unintended uses).

## Dependencies & Risks

- **Branch dependency:** the parked branch `doc/roadmap-coverage-check-completed` (`c39fc66`) also
  edits `ROADMAP.md`. Two safe options:
  1. Land that branch first as a separate PR, then this work rebases on top.
  2. Cherry-pick `c39fc66` into this work's branch and open a single PR.

  Recommendation: option 1 — keeps PR scope clean and resolves the parked branch.

- **Schema rebuild requirement:** existing users must run `lore ingest --force` to pick up the new
  column. The CHANGELOG entry is the operator-facing notice. The startup `PRAGMA table_info` probe
  in `KnowledgeDB::open` is the runtime safety net — it refuses to start with the friendly advisory
  rather than silently failing later.

- **`full_ingest` atomicity gap (pre-existing, exposed more loudly).** `full_ingest` at
  `src/ingest.rs:594-683` calls `clear_all` then embeds-and-inserts file-by-file with no
  transactional wrapper. If Ollama outages mid-batch, the database is in a worse state than before.
  This plan does not introduce the risk but exposes it more loudly because users will run
  `lore ingest --force` more often (after every binary upgrade that requires a schema rebuild).
  **Recommended follow-up todo:** backup-and-swap around `clear_all` (rename `knowledge.db` →
  `knowledge.db.bak` before, restore on non-empty `result.errors`). Out of scope for this plan.

- **Body-size compound risk:** per-call re-injection of a large universal pattern body is expensive.
  The R6 advisory at >3 patterns (count) AND the new per-pattern body-size advisory at >1KB jointly
  guard the dominant cost vectors. Future improvement: a per-session runtime body-size warning at
  PreToolUse if total universal-body bytes injected exceeds a threshold (e.g. 8KB). Documented in
  Future Work.

- **Coverage-check signal absence:** universal patterns have no PreToolUse-discoverability signal
  that coverage-check can measure meaningfully. Authoring-guide content explains this;
  coverage-check output marks universal patterns to prevent authors from chasing meaningless
  coverage scores on patterns that bypass the very channel coverage-check measures.

- **Mid-session untag does not retract already-consumed context.** Tagging or untagging mid-session
  takes effect for re-injection decisions on the next tool call after re-ingest, but the body
  already in the agent's session context is not retracted. Effect is fully realised at the next
  session boundary. Documented in State lifecycle risks.

## Future Considerations

- **Per-pattern body-size runtime guard at PreToolUse** for pathological cases the ingest-time
  per-pattern advisory misses.
- **`full_ingest` backup-and-swap atomicity.** Pre-existing risk; this feature increases user
  exposure. Worth a follow-up todo.
- **Cycle-based dedup TTL** (already in ROADMAP "Future" section) — complementary general-case
  primitive. Universal patterns are the always-on degenerate case (TTL = 0); cycle-based TTL handles
  the middle ground (re-inject every N tool calls). The `PartitionedResults` shape introduced here
  eases the future refactor: a third slice (`cycle_due`) slots in alongside `universal` and
  `ranked`.
- **Namespaced reserved tag** (`lore:universal`) to eliminate collision risk with patterns about CSS
  universal selectors, i18n universals, etc. Low priority — the per-pattern advisories surface
  accidental uses.
- **Per-heading universal granularity** — flag individual headings rather than the whole file.
  Deferred until a real authoring need appears.
- **`ChunkMetadata` grouping** on `SearchResult` to absorb future per-chunk flags without
  re-touching every SELECT. Revisit when a second flag is in the pipe.
- **Migration-version stub** in the existing `ingest_metadata` table to enable structured detection
  of "needs reingest" without parsing SQLite errors. Defer until the second column requires it.

## Sources & References

### Origin

- **Origin document:**
  [`docs/brainstorms/2026-04-20-universal-patterns-requirements.md`](../brainstorms/2026-04-20-universal-patterns-requirements.md).
  Key decisions carried forward: (1) cadence = SessionStart + bypass PreToolUse dedup with relevance
  gate intact, (2) format = dedicated `## Pinned conventions` section at top of SessionStart, (3)
  budget interaction = additive beyond `top_k` with soft warning at >3 tagged.

### Internal references

- Schema: `src/database.rs:107-141` (chunks table), `:63-72` (SearchResult), `:81-86`
  (PatternSummary — reused for `universal_patterns`), `:187-232` (insert_chunk), `:333-357`
  (list_patterns template).
- Chunking: `src/chunking.rs:11-26` (Chunk), `:43,138` (chunkers), `:181-219`
  (extract_frontmatter_tags).
- Ingest: `src/ingest.rs:69-81` (IngestResult), `:670` (per-file line), `:594-683` (full_ingest
  atomicity gap site), `:1095-1105` (validate_within_dir — reused at SessionStart),
  `src/main.rs:407-454` (print_ingest_summary).
- Hook: `src/hook.rs:132` (handle_session_start), `:152-252` (handle_pre_tool_use), `:268`
  (handle_post_compact), `:351-412` (search_with_threshold — refactored to return
  PartitionedResults), `:425-459` (format_session_context), `:528-563` (dedup_filter_and_record).
- Tests: `tests/hook.rs:16-50` (config + DB harness), `tests/single_file_ingest.rs:24-28` (memory_db
  helper).

### Institutional learnings consulted

- `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md` —
  read-side dedup filter requirement, dedup file lifecycle.
- `docs/solutions/integration-issues/additional-context-timing-in-pretooluse-hooks-2026-04-02.md` —
  SessionStart and PreToolUse share the one-tool-call delay; validates the cadence decision.
- `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  — hazard-pin test required for the dedup bypass mutation route.
- `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`
  — tag removal must take effect without `--force`; pin with tests on both delta and single-file
  ingest paths.
- `docs/solutions/best-practices/coverage-check-query-source-must-simulate-hook-not-llm-2026-04-08.md`
  — universal patterns bypass the search path coverage-check measures; document the limitation, mark
  in coverage-check output.

### Project conventions consulted

- `rust/sqlite.md` (system-injected) — "Treat the database as a derived artefact" rejected the ALTER
  TABLE migration path in favour of CREATE TABLE IF NOT EXISTS + force-reingest.
- `ci/github-actions-rust.md` (system-injected) — gateway job pattern is unaffected by this work;
  tests must pass `just ci` as gate.

### Related work

- PR #32 (`feat: Coverage check skill`) — addresses the orthogonal PreToolUse-discoverability half.
  Universal patterns address the always-on half.
- Branch `doc/roadmap-coverage-check-completed` (`c39fc66`) — parked ROADMAP cleanup that this plan
  depends on (see Dependencies).

## Documentation Plan

- New "When to use the universal tag" subsection in `docs/pattern-authoring-guide.md` (with
  body-size guidance and the `## Pinned conventions` ↔ `universal` naming-divergence note).
- Updated Skills/Hook section in `docs/hook-pipeline-reference.md` documenting the
  `## Pinned conventions` SessionStart structure and the partitioned search-result shape.
- One-sentence mention in `README.md` under the Use with Claude Code section.
- CHANGELOG entry calling out the destructive `lore ingest --force` requirement and Ollama time
  budget.
- ROADMAP move from "Up Next" to "Completed".
- Coverage-check skill output marker (`[universal — bypasses PreToolUse
  dedup]`) added in
  `integrations/claude-code/skills/coverage-check/SKILL.md`.
