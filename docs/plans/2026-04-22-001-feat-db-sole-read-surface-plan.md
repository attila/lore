---
title: Restore DB-as-sole-read-surface and render pinned conventions from DB
type: feat
status: complete
date: 2026-04-22
completed: 2026-04-22
origin: docs/brainstorms/2026-04-21-read-surface-invariant-requirements.md
pr: https://github.com/attila/lore/pull/34
---

# Restore DB-as-sole-read-surface and render pinned conventions from DB

## Enhancement Summary

**Deepened on:** 2026-04-22. **Review agents used:** data-integrity-guardian, data-migration-expert,
architecture-strategist, performance-oracle, code-simplicity-reviewer,
pattern-recognition-specialist, spec-flow-analyzer, agent-native-reviewer.

### Blockers caught and integrated

1. **Embedder must run outside the outer transaction.** Both data-integrity and performance reviews
   independently flagged the original R4 as ambiguous: wrapping `delete_by_source` + N ×
   `(embed + insert_chunk)` in one transaction would hold the SQLite write lock across N Ollama HTTP
   round-trips (~2 s per 10-chunk file), blocking every concurrent reader. R4b is now an explicit
   requirement: embeddings are pre-computed _before_ the transaction opens.
2. **Tampered `raw_body` threat model.** Removing the disk read also removes `validate_within_dir`
   - `sanitize_for_log` from the render path. R2 now states the explicit threat decision:
     `knowledge.db` write access is the existing trust boundary; no extra runtime sanitisation is
     introduced. Pinned render test pins the decision so a future refactor can't silently change it.

### High-impact precision fixes integrated

- **R4 tightened** with three sub-points: embedder-outside-transaction (R4b), `delete_by_source`
  removes the `patterns` row atomically (R4c), and the outer transaction uses BEGIN IMMEDIATE (not
  the rusqlite default DEFERRED) so writer-vs-writer races serialise cleanly.
- **R9 new**: upgrade advisory text rewritten to be version-agnostic. The existing string in
  `src/database.rs` names the universal-patterns feature specifically and will confuse users on a
  v1→v2 upgrade.
- **Architecture doc scope sharpened.** The invariant explicitly carves out session-local state
  (dedup files, lockfiles), agent-harness inputs (Claude Code transcripts), configuration, and git
  metadata. Without this, `last_user_message` reading the transcript from `$HOME` would read as a
  violation.
- **Unit 3 invariant lint widened** to cover `fs::read*` / `File::open` in runtime modules with an
  explicit allow-list, not only DISTINCT-over-chunks queries.
- **Rollback is explicitly unsafe.** Dependencies & Risks now states: the schema probe uses `>=`, so
  reverting this PR against a v2 DB silently passes the probe but leaves an orphan `patterns` table
  on subsequent `clear_all` calls. Users must delete `knowledge.db` before downgrading.
- **PatternSummary not widened.** Pattern-recognition review surfaced that adding
  `raw_body: Option<String>` to the existing listing struct conflates two return shapes. Plan now
  introduces a dedicated `UniversalPattern` struct for the render-side read; `PatternSummary` stays
  lean for listing; `PatternRow` stays the ingest-side write shape.

### Spec-flow gaps closed

- Delete-after-ingest: patterns row removal covered by R4c.
- Orphan-state diagnostics: optional consistency check at `lore_status` time; flagged as a future
  consideration rather than a required add to keep scope bounded.
- Non-universal `raw_body` unbounded: documented in State lifecycle risks.
- Concurrent ingest: serialised by BEGIN IMMEDIATE; noted in Dependencies & Risks.

### Decision resolved during deepen

- **`content_hash` and `ingested_at` stay.** Code-simplicity review argued for cutting both under
  YAGNI ("no reader in this PR, zero queued consumer, cut until needed"). Owner decision
  (2026-04-22): keep both. Rationale: pre-1.0 with no release process and a small user base, so the
  amortised "extra schema bump later" cost is low; bigger picture, the project doesn't subscribe to
  blanket YAGNI when there's a genuine systems-design reason — the columns encode per-pattern
  identity that any future per-row diagnostic, delta short-circuit, or staleness query will want.
  Recording here so a future reviewer doesn't re-litigate.

### Findings not integrated (correctly deferred)

- Pre-splitting `docs/architecture.md` into `docs/architecture/*.md` per invariant — premature for
  one entry.
- Case-insensitive filesystem `source_file` collision under PRIMARY KEY — pre-existing, out of
  scope; noted as a known edge.
- Full MCP-agent sandbox permission doc — CHANGELOG polish, not a plan change.

---

## Overview

Introduce a `patterns` table as the authorial-body source for the pinned-render path, bump the
schema to `SCHEMA_VERSION = 2`, migrate every pattern-level query off
`SELECT DISTINCT … FROM
chunks` onto the new table, and codify the invariant (_`knowledge.db` is the
sole runtime read surface for indexed content; the patterns directory is an authoring/git surface
only; ingest is the sole sanctioned disk→DB pipeline_) in a new `docs/architecture.md` linked from
`CONTRIBUTING.md`.

Follow-up to PR #33 (`feat/universal-patterns`, squash-merged at `569a1f6`). The universal-patterns
feature shipped correctly but introduced `std::fs::read_to_string` in `render_pinned_conventions` —
the first runtime disk read outside ingest. Sandbox test drives (nono.sh) surfaced the consequence:
the agent needs two read surfaces (`knowledge.db` + the patterns directory) when historically it
needed only one.

## Problem Statement

Before PR #33, every runtime reader of indexed content — the MCP server, the PreToolUse /
PostToolUse hooks, every CLI subcommand — read through `knowledge.db`. The patterns directory was
purely for authoring and git: humans edit there, `add_pattern` / `update_pattern` /
`append_to_pattern` write there, and `ingest` reads it once to populate the DB. An agent consuming
lore via MCP needed exactly one read surface.

PR #33 added `render_pinned_conventions` (`src/hook.rs:548`), which reads the authorial pattern body
from disk because `chunks` stores heading-split fragments and doesn't carry a round-trip
reconstruction. The feature works, but the implementation quietly elevated the patterns directory
from "authoring surface" to "runtime read dependency". The
`docs/todos/pinned-conventions-render-
from-db-not-disk.md` (P2) entry captured the symptom. This
plan elevates it to a codified architectural invariant and restores it by moving the authorial body
into the DB.

A secondary benefit falls out of the fix: two sources of truth for "which patterns exist?"
(`list_patterns` / `universal_patterns` via `SELECT DISTINCT … FROM chunks` versus the new
`patterns` table) is exactly the class of drift that caused the original violation. Migrating those
queries in the same PR prevents the cleanup tail from recreating the problem at smaller scale.

## Proposed Solution

A new `patterns` table keyed by `source_file` holds the authorial body plus per-pattern metadata,
one row per pattern file. Chunks continue to hold heading-split fragments for search; `patterns`
holds the whole document for rendering and listing.

```sql
CREATE TABLE IF NOT EXISTS patterns (
    source_file  TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    tags         TEXT NOT NULL,
    is_universal INTEGER NOT NULL DEFAULT 0 CHECK (is_universal IN (0, 1)),
    raw_body     TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    ingested_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

`ingested_at` uses SQLite's `datetime('now')` default to match the existing `ingest_metadata`
convention (`src/database.rs` — the `INGEST_METADATA_DDL` const already uses this format). Output:
`YYYY-MM-DD HH:MM:SS` in UTC. Rust code inserting a row does not need to supply the column; the
default fires on INSERT.

Four changes consume it:

1. **Ingest population.** Every path that writes chunks (`full_ingest`, `delta_ingest`,
   `ingest_single_file`, `add_pattern`, `update_pattern`, `append_to_pattern`) writes the
   corresponding `patterns` row in the same transaction as its chunk writes. Modifications replace
   the row; single-file ingest wraps `delete_by_source` + `INSERT INTO patterns` + all chunk inserts
   in one outer transaction. Full ingest extends `clear_all`'s DROP+CREATE dance to include
   `patterns`.
2. **Pinned render migrates to DB.** `render_pinned_conventions` reads `raw_body` from `patterns`
   and drops the `validate_within_dir` + `std::fs::read_to_string` dance. The pinned-section byte
   cap (`PINNED_SECTION_TOTAL_LIMIT_BYTES`) still applies at render time.
3. **Pattern-level query migration.** `list_patterns` and `universal_patterns` become direct queries
   against `patterns`. `stats().sources` becomes `SELECT COUNT(*) FROM patterns`. `source_files()`
   becomes `SELECT source_file FROM patterns ORDER BY source_file`.
4. **Schema probe bumps to v2.** `KnowledgeDB::open` already probes `PRAGMA user_version` (added in
   #33). Changing `SCHEMA_VERSION` from `1` to `2` makes existing v1 databases fail-fast with the
   existing `lore ingest --force` advisory. `open_skipping_schema_check` remains the sanctioned
   remedy-path bypass. The composition test from the post-review lesson is carried over.

Invariant codification lands alongside: new `docs/architecture.md` (peer to
`hook-pipeline-reference.md`, `search-mechanics.md`, `pattern-authoring-guide.md`) opens with a
one-line invariants index and a first section pinning _DB-as-sole-read-surface_. `CONTRIBUTING.md`
links to it.

## Technical Considerations

### Patterns table: shape and relationship to chunks

The table is the per-pattern entity that has always been implicit in the codebase. Existing
`list_patterns` reconstructs it by finding the chunk with minimum `heading_path` length per
`source_file` and correlating `MAX(is_universal)` across siblings (`src/database.rs:346-378`).
Making it explicit:

- **Keys on `source_file`**, matching the existing de-facto primary key used throughout ingest
  (`delete_by_source`, chunk `source_file` column, sibling-expansion in the hook).
- **No foreign key** between `chunks.source_file` and `patterns.source_file`. Zero FK usage
  currently exists in the codebase (grep confirms), and the "DB as derived artefact" convention from
  `rust/sqlite.md` prefers DROP+CREATE over ALTER. The 1:1 invariant is enforced by the ingest layer
  via outer transactions; correctness depends on ingest, not on SQLite constraints.
- **`content_hash` is `fnv1a`, 16-hex-char TEXT.** Reuses the existing `src/hash.rs::fnv1a` helper
  already used for session-ID hashing in `dedup_file_path`. Non-cryptographic (named `content_hash`
  to reflect this — the requirements doc's `content_sha` implied SHA, which we deliberately avoid)
  is sufficient for "did the file change?" delta detection. Input is the full file contents
  (pre-frontmatter-strip) so that frontmatter-only edits — including the `universal` tag flip —
  produce a different hash. Avoids adding `sha2` or `blake3` as a dependency, respecting the
  project's binary-size watch.
- **`ingested_at` and `content_hash` land unread in this PR.** Decision logged in the requirements
  doc's Key Decisions: one schema bump carrying three columns is cheaper than three bumps carrying
  one each, because every bump forces users through the `lore ingest --force` rebuild under the
  compatibility-check lesson. `ingested_at` enables future diagnostics on `lore_status`;
  `content_hash` enables future delta-ingest short-circuiting. Carrying cost: a few bytes per row;
  no runtime path exercises either column in this PR.

### Reconciliation atomicity (R4)

Single-file ingest currently composes `delete_by_source` (one transaction) with a loop of
`insert_chunk` calls (each its own transaction). For R4's 1:1 invariant between `patterns` rows and
files, the `patterns` row write must land atomically relative to the chunk writes — no reader can
ever observe a `patterns` row pointing at a `source_file` whose chunks have been deleted but not
reinserted, or vice-versa.

**Approach:** introduce a private `ingest_single_file_inner(tx, chunks_with_embeddings, …)` helper
that takes an outer transaction opened with `BEGIN IMMEDIATE` (via
`conn.transaction_with_behavior(TransactionBehavior::Immediate)`, not the default `DEFERRED` from
`unchecked_transaction()`). The helper receives a pre-computed `Vec<(Chunk, Option<Embedding>)>` —
**embedder calls run before the transaction opens**, never inside it. Inside the transaction:
`DELETE FROM patterns WHERE source_file = ?1`, `INSERT OR
REPLACE INTO patterns …`,
`delete_by_source`'s three DELETEs inlined, the per-chunk INSERTs, and a `debug_assert!`-gated
invariant check that `patterns` contains exactly one row and `chunks` contains exactly N rows for
this `source_file`. Commit once.

**Why embedder-outside-transaction is non-negotiable.** Today each `insert_chunk` is its own short
transaction, and Ollama HTTP happens between transactions — write-lock holds are millisecond-scale.
Collapsing everything into one outer transaction without lifting the embedder would hold the SQLite
write lock across N × ~200 ms of network I/O (a 10-chunk file = ~2 s of held lock), starving
concurrent readers past the 5 s `busy_timeout`. R4b is the acceptance-blocking pin that prevents the
naïve implementation from shipping.

**Why BEGIN IMMEDIATE.** Rusqlite's `unchecked_transaction()` defaults to `BEGIN DEFERRED`, which
delays write-lock acquisition to the first write. Under concurrent writers, two DEFERRED
transactions can both read, then one's promotion to write fails with `SQLITE_BUSY` — sometimes
rolling back work already done in that transaction. `IMMEDIATE` acquires the write lock at BEGIN, so
writers serialise cleanly with no rollback surprise.

**This naturally overlaps with** `docs/todos/index-single-file-reconciliation-single-transaction.md`
(P3). With R4b, this PR closes the P3 todo's embedder-outside-lock concern for the single-file path
along with the transaction-atomicity concern. Mark the todo complete after Unit 1 ships.

Full ingest's existing `clear_all` (`src/database.rs:156-167`) is already transactional. Extend it
to DROP+CREATE `patterns` in the same transaction.

### Query migration

Four callers currently derive pattern-level state from `chunks`:

| Caller                                       | Current query                                                           | New query                                                                                      |
| -------------------------------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `list_patterns` (`src/database.rs:346`)      | Correlated subquery finding min-heading-path root + `MAX(is_universal)` | `SELECT source_file, title, tags, is_universal FROM patterns ORDER BY source_file`             |
| `universal_patterns` (`src/database.rs:381`) | Filters `list_patterns()` in Rust                                       | `SELECT source_file, title, tags, 1 FROM patterns WHERE is_universal = 1 ORDER BY source_file` |
| `stats().sources` (`src/database.rs:430`)    | `SELECT COUNT(DISTINCT source_file) FROM chunks`                        | `SELECT COUNT(*) FROM patterns`                                                                |
| `source_files` (`src/database.rs:453`)       | `SELECT DISTINCT source_file FROM chunks ORDER BY source_file`          | `SELECT source_file FROM patterns ORDER BY source_file`                                        |

The `GROUP BY c1.source_file` at `src/database.rs:362` inside `search_hybrid` operates over search
matches, not pattern-level state. **Leave alone.** Under the R3 audit cap of ~5 additional callers,
the total is four migrations.

### Pinned render migration

`render_pinned_conventions` (`src/hook.rs:548-611`) currently: loads universal patterns from DB
(titles only), for each one canonicalises `knowledge_dir.join(source_file)` via
`validate_within_dir`, reads the canonical path via `std::fs::read_to_string`, trims, appends to the
render buffer until the 32 KB cap is hit.

New shape: `universal_patterns()` returns rows from the `patterns` table with `raw_body` included.
The render loop walks rows, applies the 32 KB cap against `raw_body` directly, and appends the body.
The `validate_within_dir` call, the symlink-TOCTOU hardening, and the `fs::read_to_string` call all
disappear. **The containment check stays in ingest** (where disk reads are sanctioned —
`ingest_single_file`, `update_pattern`, `append_to_pattern` all still call `validate_within_dir`).
Per-file body cap (`UNIVERSAL_BODY_HARD_LIMIT_BYTES = 8 KB`) continues to fire at ingest; the
render-time cap continues to fire if a tampered DB bypasses ingest.

### Architecture doc: invariants index

Open with a one-line index so future invariants slot in without a structural rewrite:

```markdown
# Architecture invariants

1. [DB as sole runtime read surface](#db-as-sole-runtime-read-surface)

## DB as sole runtime read surface

`knowledge.db` is the sole runtime read surface for indexed content…
```

When the second invariant lands, the author appends to the index and adds a section — no
re-organisation. Stylistic polish, low cost.

### What stays out

Per the requirements doc's Scope Boundaries, the following are named and parked:

- **Delta-ingest short-circuit using `content_hash`.** Column lands unread. Future PR wires a "hash
  matches → skip re-index" branch into `delta_ingest`. Needs its own measurement (binary- size
  impact is zero; correctness risk is near-zero, but benchmarking is its own exercise).
- **`get_pattern(source_file)` MCP tool.** Now trivial to add (one SQL select), but nothing in this
  PR needs it. Clean future entry point.
- **CLI read-surface audit.** `lore list`, `lore search`, `lore status` all go through DB today via
  the `db.*` methods; the audit in Unit 3 confirms this. If anything surfaces that reads disk at
  runtime, it migrates in the same PR under R3. Expected finding: nothing.
- **P3 single-transaction todo.** R4 resolves the patterns+chunks atomicity half; the embedder-
  outside-lock half remains open.

## System-Wide Impact

### Interaction graph

**Ingest paths (all become transactional across `patterns` + `chunks`):**

`full_ingest` → `clear_all` (DROP+CREATE extends to `patterns`) → per-file `ingest_single_file` (new
outer transaction spans `patterns` row write + all chunk inserts) → commit per file.

`delta_ingest` → for each changed file, `ingest_single_file` (new outer transaction).

`add_pattern` / `update_pattern` / `append_to_pattern` → write markdown to disk (sanctioned: this is
the authoring surface) → `ingest_single_file` with new outer transaction → chunks and `patterns` row
updated atomically.

**Render path (disk reads vanish):**

`SessionStart` / `PostCompact` → `handle_session_start` → `format_session_context` →
`render_pinned_conventions` → **NEW** `db.universal_patterns()` returns `raw_body` per row → render
loop appends bodies against `PINNED_SECTION_TOTAL_LIMIT_BYTES` cap → emit. No
`std::fs::read_to_string` call in this path.

**Query paths (single source of truth):**

`list_patterns` / `universal_patterns` / `stats` / `source_files` → direct `patterns`-table queries.
Callers (`lore list`, `lore_status`, MCP `list_patterns`, CLI `cmd_list_sources` if any) are
unaffected at the interface.

**Open / upgrade path (same shape as #33):**

`KnowledgeDB::open` → `PRAGMA user_version` probe → if `< SCHEMA_VERSION (2)` → advisory error
`"run \`lore ingest --force\` to rebuild with the new
schema"`.`cmd_ingest`+`--force`+ no`--file`→`KnowledgeDB::open_skipping_schema_check`→`clear_all` →
DROP+CREATE chunks and patterns → re-ingest → all rows populated.

### Error & failure propagation

- **`db.universal_patterns()` query failure at SessionStart** → `format_session_context` already
  returns `anyhow::Result`; bubble up, hook handler logs and exits 0 (per existing "hook never
  breaks the agent" contract). User gets a session without pinned conventions; no agent disruption.
- **`patterns` row missing for a `source_file` referenced by a chunk** (ingest bug, not a runtime
  expectation) → `universal_patterns` skips it silently; the chunk still appears in search results,
  just not in the pinned section. No crash, no silent data exposure.
- **`patterns` table missing on an old (v1) DB** → caught by `PRAGMA user_version` probe at
  `KnowledgeDB::open`, user sees the advisory, runs `lore ingest --force`, probe bypassed via
  `open_skipping_schema_check`, `clear_all` rebuilds schema, `full_ingest` repopulates.
- **Outer transaction fails mid-ingest** (rare: Ollama timeout, disk full) → rollback rolls back
  `patterns` row change and all chunk changes. Pre-existing behaviour for the chunk side; now
  symmetric for patterns.

### State lifecycle risks

- **1:1 invariant enforcement is purely ingest-layer.** Any future ingest path that bypasses the
  `ingest_single_file_inner` helper risks drifting the two tables. Mitigation: all three write-path
  entry points (`full_ingest`, `delta_ingest`, single-file variants) route through the same helper.
  Code comment on the helper makes the contract explicit.
- **`raw_body` is a point-in-time snapshot of the file at ingest.** If a user edits the markdown
  directly (without re-running ingest) and then starts a session, `raw_body` is stale. This is
  unchanged from the prior behaviour for `chunks` — both tables go stale together. Delta-ingest
  (when run) re-syncs both.
- **Downgrade from v2 → v1 requires symmetric rebuild.** Revert + users re-run
  `lore ingest
  --force` under v1. Requirements doc's Dependencies / Assumptions section calls this
  out explicitly. The PR description will repeat the guidance.
- **Schema probe does not retroactively fix v1 DBs it encounters.** Users get the advisory, action
  is theirs. Matches #33's behaviour — no silent migration.

### API surface parity

- **`list_patterns` MCP tool** — unchanged wire shape. Internal implementation swaps from the
  chunks-DISTINCT query to the patterns-table query; `PatternSummary` structure preserved.
- **`lore list` CLI** — unchanged output. Reads the new query; `[universal]` marker still renders
  from `is_universal`.
- **`search_patterns` MCP tool** — unchanged. Continues to return chunks; `is_universal` on each
  chunk row still sourced from `chunks.is_universal`.
- **`lore_status` MCP tool** — unchanged. `sources_indexed` continues to come from `stats()`, which
  now reads `patterns`.
- **SessionStart payload** — identical to the user's eye. `## Pinned conventions` section structure
  preserved; only the body source changes.

### Integration test scenarios

1. **Sandbox simulation (R6 acceptance gate).** Ingest a knowledge base, then chmod-000 the patterns
   directory (or use a test harness that simulates denied read). Open the DB, trigger SessionStart.
   Pinned section renders correctly; no `fs::read` call is attempted against the patterns directory.
2. **Schema upgrade composition test.** Start with a v1 database (created by running #33-era code)
   with at least one universal pattern. Open → probe fails with advisory. Run
   `lore ingest
   --force` → `open_skipping_schema_check` → `clear_all` drops v1 chunks and
   recreates at v2 → `full_ingest` populates both `chunks` and `patterns` → open at v2 succeeds →
   hook renders from `patterns` row → no disk read attempted. This is the R6 gate.
3. **Single-file ingest atomicity pin.** Edit a pattern body, call `ingest_single_file`. Concurrent
   reader (simulated via a second `KnowledgeDB::open`) reading mid-transaction sees either the old
   state or the new state, never a mismatch (e.g. new `raw_body` with old chunks).
4. **Tag flip round-trip.** Add `universal` to a pattern's frontmatter, save, `delta_ingest` detects
   the change (via `content_hash` difference — this validates the hash input choice of whole-file),
   `patterns.is_universal` flips to 1, next SessionStart includes the pattern in the pinned section.
5. **Invariant-scope pin.** Grep the compiled binary or `src/` tree for `std::fs::read_to_string`
   calls. Confirm the only remaining occurrences are in ingest-reachable code paths. A test or lint
   can automate this assertion.

## Acceptance Criteria

### Functional

- [ ] **R1** New `patterns` table exists with columns `source_file PRIMARY KEY`, `title`, `tags`,
      `is_universal`, `raw_body`, `content_hash`, `ingested_at`. DDL uses
      `CREATE TABLE IF NOT
      EXISTS` per project convention.
- [ ] **R2** `render_pinned_conventions` sources `raw_body` from the `patterns` table. No
      `std::fs::read_to_string` call remains in the SessionStart / PostCompact rendering path. The
      `validate_within_dir` + canonical-path dance is removed from this caller (but retained for
      ingest callers).
- [ ] **R2b** _Threat-model decision (pinned)._ `knowledge.db` write access is treated as the
      existing trust boundary — an adversary who can tamper with DB rows can already influence what
      the agent sees via chunks. Removing the disk-read path does not introduce a new exposure;
      `raw_body` rendering does not add control-character sanitisation. This is an explicit
      decision, not an oversight: sanitising user-authored markdown would mangle legal content (code
      blocks, escape sequences in examples). A regression test inserts a row with ANSI escapes and
      asserts the render output contains them verbatim, locking the decision against silent
      reversal.
- [ ] **R3** `list_patterns`, `universal_patterns`, `stats().sources`, and `source_files` query the
      `patterns` table directly. The `GROUP BY` inside `search_hybrid` is confirmed as a
      search-internal aggregation (not pattern-level state) and left unchanged. No additional
      pattern-level DISTINCT-over-chunks callers exist; if audit finds any, up to five migrate in
      this PR and the rest are punted as documented follow-ups.
- [ ] **R4** Every ingest path (`full_ingest`, `delta_ingest`, `ingest_single_file`, and the write
      operations behind `add_pattern` / `update_pattern` / `append_to_pattern`) maintains a 1:1
      invariant between `patterns` rows and pattern files. Single-file ingest wraps
      `delete_by_source` + `INSERT OR REPLACE INTO patterns` + per-chunk inserts in a single outer
      transaction opened with `BEGIN IMMEDIATE` (not rusqlite's default `DEFERRED` from
      `unchecked_transaction()`) so writer-vs-writer races serialise cleanly under the existing
      `busy_timeout = 5000 ms`. Full ingest's `clear_all` DROP+CREATEs `patterns` alongside chunks
      inside the existing transaction.
- [ ] **R4b** _Embedder runs outside the outer transaction._ `ingest_single_file_inner(tx, …)`
      receives pre-computed `Vec<(Chunk, Option<Embedding>)>`; no `embedder.embed()` call executes
      while the outer transaction is open. Without this, a 10-chunk file holds the SQLite write lock
      for ~2 s of Ollama HTTP, blocking concurrent readers past `busy_timeout`. Test: instrument the
      embedder fake to fail if called between `BEGIN IMMEDIATE` and `COMMIT`.
- [ ] **R4c** _Deletion is atomic across tables._ When a pattern file is deleted (delta-ingest
      observes the file missing) the `patterns` row is removed in the same transaction as the chunk
      deletions. `delete_by_source` is extended to remove the matching `patterns` row, or wrapped in
      a higher-level `delete_pattern_and_chunks(tx, source_file)` helper that is the documented
      entry point. Test: `delete_by_source_removes_patterns_row` is an acceptance-blocking
      integration test, not optional.
- [ ] **R4d** _Failure-loud invariant guard._ `ingest_single_file_inner` ends with a `debug_assert!`
      that `SELECT COUNT(*) FROM patterns WHERE source_file = ? = 1` and
      `SELECT COUNT(*) FROM chunks WHERE source_file = ? = N` (N being the number of chunks the
      caller just inserted). Production builds skip the assert; debug builds + CI catch drift if a
      future write path forgets the outer transaction.
- [ ] **R5** `SCHEMA_VERSION` bumps to `2`. The existing `PRAGMA user_version` probe rejects v1 DBs
      with the advisory "run `lore ingest --force` to rebuild the index with the new schema".
      `open_skipping_schema_check` bypasses the probe when `--force` and no `--file`.
- [ ] **R6** Composition test exercises the full v1 → v2 upgrade loop end-to-end: start with a v1
      DB, probe fires, `full_ingest --force` completes, new schema populated, hook renders from DB
      only, zero pattern-directory file reads during render. This test is the acceptance gate for
      the PR.
- [ ] **R7** New `docs/architecture.md` exists, peer to `hook-pipeline-reference.md` /
      `search-mechanics.md` / `pattern-authoring-guide.md`. Opens with a one-line invariants index.
      First section codifies the read-surface invariant with the single explicit exception (ingest).
      Names the pre-fix `render_pinned_conventions` disk-read as the reason the doc exists, notes
      "this invariant is now enforced" once the PR lands. **Scope carve-outs are explicit:** the
      invariant applies to _indexed content_ (pattern bodies, chunks, tags, titles). It does NOT
      apply to (a) session-local state such as the dedup file and lockfile, (b) agent-harness inputs
      such as the Claude Code transcript read by `last_user_message`, (c) configuration files
      (`knowledge.toml`), or (d) git metadata queried via subprocess. Authoring writes to the
      patterns directory via `add_pattern` / `update_pattern` / `append_to_pattern` are sanctioned
      but must conclude by re-ingesting the written file in the same call — the doc states this as a
      companion clause so a future authoring path can't silently drop the re-ingest step.
- [ ] **R8** `CONTRIBUTING.md` links to `docs/architecture.md`. The new doc lives outside the
      knowledge directory so the ingest pipeline does not index it as a pattern.
- [ ] **R9** _Upgrade advisory text is version-agnostic._ The current string in
      `check_schema_compatibility` names the universal-patterns feature specifically ("this database
      predates the universal-patterns feature"). Rewrite to carry the version numbers: _"lore: this
      database was written by an older version of lore (schema v{stored} < v{current}). Run
      `lore ingest --force` to rebuild the index with the new schema."_ Update the open-rejects-v1
      test to assert on the version-agnostic `lore ingest --force` substring rather than any
      feature-specific phrase, so a future v3 bump reuses the string without needing to rewrite the
      advisory again.

### Non-functional

- [ ] Storage impact measured on `lore-patterns` and noted in the PR description. Expected: ~30
      patterns × ~2 KB avg ≈ 60 KB additional DB size. Hard ceiling: 100 patterns with bodies near
      the 8 KB universal cap → ~400 KB including SQLite overflow-page overhead (bodies >4 KB spill
      to overflow pages on the default page size). Negligible.
- [ ] SessionStart wall-clock time impact: non-negative, likely negative (one DB query replaces N
      filesystem round-trips + N canonicalisations). Not instrumented; rough smoke test on
      `lore-patterns` during implementation suffices.
- [ ] Zero new dependencies. `content_hash` reuses `src/hash.rs::fnv1a`.
- [ ] Binary size impact: zero (no new crates; no new unsafe blocks; one extra table DDL).

### Quality gates

- [ ] `just ci` clean: `dprint check`, `cargo clippy --all-targets -- -D warnings`,
      `cargo test
      --all-targets`, `cargo deny check`, `cargo doc --no-deps`.
- [ ] All new DB code paths have unit tests in `src/database.rs::tests` (patterns table CRUD,
      clear_all DROP+CREATE, migrated query shapes, atomic single-file round-trip).
- [ ] SessionStart / PostCompact rendering covered by integration tests in `tests/hook.rs` (sandbox
      simulation, schema upgrade composition, stale `raw_body` after direct disk edit without
      re-ingest).
- [ ] `tools_list_returns_all_six_tools` insta snapshot unchanged — the MCP tool surface doesn't
      change in this PR.

## Implementation Units

### Unit 1 — Schema v2 foundation (patterns table + ingest population)

**Goal:** persist per-pattern authorial body and metadata atomically with chunk writes; bump
`SCHEMA_VERSION` to `2`.

Touched files:

- `src/database.rs` — add `PATTERNS_DDL` const alongside `CHUNKS_DDL`, `PATTERNS_FTS_DDL`,
  `INGEST_METADATA_DDL`. Add `SCHEMA_VERSION = 2`. Extend `init` to run `PATTERNS_DDL`. Extend
  `clear_all` to DROP+CREATE `patterns` inside the existing transaction. Add
  `upsert_pattern(tx: &Transaction, row: &PatternRow)` taking an external transaction. Add
  `delete_pattern(tx: &Transaction, source_file: &str)` taking an external transaction.
  Remove/replace the correlated-subquery implementation of `list_patterns` with a direct
  `patterns`-table query; same for `universal_patterns` (now a `WHERE is_universal = 1` variant, not
  a Rust-side filter over `list_patterns`). Migrate `stats().sources` and `source_files()` to query
  `patterns`.
- `src/hash.rs` — no changes. Reused as-is by the ingest layer.
- `src/chunking.rs` — add a `pattern_row_from(file_contents, source_file, chunks) -> PatternRow`
  helper that extracts `title` (first chunk's title or first heading), `tags` (frontmatter tag
  list), `is_universal` (already computed per-chunk — all chunks of a file share the flag),
  `raw_body` (frontmatter-stripped file body; matches what agents actually want to see), and
  `content_hash` (`format!("{:016x}", fnv1a(file_contents.as_bytes()))`). **Decision note:**
  `raw_body` is frontmatter-stripped (not whole-file) because the frontmatter block is a metadata
  concern that agents don't need rendered into pinned context. Frontmatter edits still change
  `content_hash` because the hash is over the whole file.
- `src/ingest.rs` — introduce `ingest_single_file_inner(tx, …)` that takes an outer transaction and
  issues the patterns-row upsert + chunk writes atomically. `ingest_single_file` becomes a thin
  wrapper that opens the transaction and commits. `full_ingest` calls the helper per file within its
  existing per-file loop. `add_pattern` / `update_pattern` / `append_to_pattern` already route
  through `ingest_single_file` — they gain atomicity for free.
- `src/main.rs` — no changes beyond any prop-through for the new `PatternRow` type if surfaced.

Tests (in `src/database.rs::tests`, `src/chunking.rs::tests`, `tests/single_file_ingest.rs`):

- `patterns_table_round_trips_all_columns` — upsert a row, select back, every field matches.
- `clear_all_drops_and_recreates_patterns` — insert rows, call `clear_all`, patterns table is empty
  and schema is fresh (select from `sqlite_master`).
- `schema_version_is_2_after_init` — `PRAGMA user_version` reads `2`.
- `open_rejects_v1_db_with_advisory` — manually create a DB with `user_version = 1` and no patterns
  table, `KnowledgeDB::open` returns an error whose message names `lore ingest
  --force`.
- `ingest_single_file_writes_patterns_row_and_chunks_atomically` — mid-transaction reader sees
  either old or new state, never mismatched.
- `ingest_single_file_replaces_patterns_row_on_modify` — ingest a file, edit body, re-ingest, row
  has new `raw_body` and `content_hash`.
- `delete_by_source_removes_patterns_row` — single-file ingest for a renamed/deleted file removes
  the patterns row along with chunks.
- `content_hash_differs_on_frontmatter_edit` — edit only the frontmatter tags, hash changes
  (validates whole-file hash input).
- `list_patterns_queries_patterns_table_not_chunks_distinct` — insert rows into `patterns` but
  nothing into `chunks`, `list_patterns` returns them (proves the query source switched).
- `universal_patterns_filters_by_is_universal_column` — same setup, only rows with
  `is_universal = 1` returned.
- **`delete_by_source_removes_patterns_row`** — acceptance-blocking (R4c). Single-file ingest for a
  deleted file removes both `chunks` and the `patterns` row in one transaction.
- **`embedder_is_not_called_inside_outer_transaction`** — acceptance-blocking (R4b). A fake embedder
  that panics if invoked while a transaction is open must not panic during `ingest_single_file`.
  Proves embeddings are pre-computed.
- **`begin_immediate_serialises_concurrent_writers`** — two threads open connections, both call
  `ingest_single_file` against different files; second blocks on busy_timeout, first commits, second
  succeeds without rollback. (Uses `--test-threads=1` or explicit thread control if needed.)
- `pattern_row_from_handles_no_frontmatter` — a pattern file with no `---` block: helper returns
  `tags = ""`, `title = first-heading-or-filename-stem`, `is_universal = false`,
  `raw_body = whole file`.

**Done when:** all tests pass; `just ci` clean; `stats().sources` and `source_files()` pass their
existing tests unchanged (they assert behaviour, not SQL).

### Unit 2 — Pinned render migrates to DB

**Goal:** `render_pinned_conventions` reads `raw_body` from `patterns`. No disk reads remain in the
rendering path.

Touched files:

- `src/database.rs` — introduce a dedicated
  `UniversalPattern { source_file, title, tags,
  raw_body }` struct returned by
  `universal_patterns()`. Do NOT widen `PatternSummary` — `PatternSummary` stays the listing shape
  (consumed by `lore list` and MCP `list_patterns`, where `raw_body` is dead weight and actively
  harmful over the MCP wire). `PatternRow` stays the ingest-side write shape. Three roles, three
  types, no overload. `universal_patterns` selects
  `SELECT source_file, title, tags, raw_body FROM patterns WHERE is_universal = 1
  ORDER BY source_file`.
- `src/hook.rs` — `render_pinned_conventions` (currently `:548-611`) walks the
  `Vec<UniversalPattern>` returned by `db.universal_patterns()`, applies
  `PINNED_SECTION_TOTAL_LIMIT_BYTES` against each row's `raw_body`, and appends. Remove the
  `validate_within_dir` call, the canonical-path read, and the `eprintln` branches for
  path-traversal / missing-file / read-failed (dead code once disk isn't touched). The render-time
  truncation marker stays. The render path does NOT sanitise `raw_body` — per R2b, DB-write access
  is the trust boundary, not this render stage.

Tests (in `src/hook.rs::tests` and `tests/hook.rs`):

- `render_pinned_conventions_reads_raw_body_from_db` — insert a patterns row with a specific
  raw_body, call the render fn, output contains that body verbatim.
- `render_pinned_conventions_does_not_touch_filesystem` — use a test harness that shadows the
  knowledge_dir with a non-existent path; render succeeds.
- `render_pinned_conventions_respects_total_byte_cap` — insert five universal rows that collectively
  exceed 32 KB, render truncates at the cap with the expected marker.
- `hook_session_start_sandboxed_patterns_dir_renders_from_db` (integration) — set the patterns dir
  to a path with no read permission (chmod-000 for the test duration), `assert_cmd` lore hook emits
  the pinned section correctly. Composition counterpart to R6.
- **`render_preserves_raw_body_control_chars_verbatim`** — pins R2b. Insert a universal pattern row
  whose `raw_body` contains literal ANSI escapes and newlines; render output contains them verbatim.
  If a future refactor adds `sanitize_for_log` to this path, this test fires loud.
- `session_start_on_empty_knowledge_base_renders_no_pinned_section` — `full_ingest --force` on an
  empty dir succeeds; SessionStart payload contains no `## Pinned conventions` header.
- `session_start_with_orphan_chunks_no_patterns_row_renders_nothing` — chunks exist but no
  corresponding patterns row (simulated inconsistent state): render skips the pattern silently;
  search still returns chunks. Documents the graceful-degradation contract.

**Done when:** all tests pass; grep of `src/hook.rs` shows no `fs::read` or `read_to_string` in the
render path; `just ci` clean.

### Unit 3 — Pattern-level query migration complete + DISTINCT audit lint

**Goal:** single source of truth for "which patterns exist?". Any future DISTINCT-over-chunks
pattern-state query fails the lint.

Touched files:

- `src/database.rs` — confirm every pattern-level query introduced in Unit 1 is wired. Remove any
  lingering DISTINCT-over-chunks helper that's no longer called.
- `src/server.rs` — confirm MCP `list_patterns` and `lore_status` handlers work unchanged against
  the migrated queries.
- `src/main.rs` — confirm `cmd_list` output is byte-identical for representative fixtures.
- Add `tests/invariants.rs` (new file) with two ripgrep-based checks:
  1. **DISTINCT/GROUP BY over `source_file` in `src/`** returns zero hits outside the
     `search_hybrid` exemption (documented inline in the test body). Future pattern-level queries
     that regress to chunks-DISTINCT fail the check.
  2. **`fs::read*`, `File::open`, `OpenOptions` in `src/hook.rs`, `src/server.rs`, `src/main.rs`**
     returns zero hits outside a pinned allow-list: `last_user_message` transcript read, dedup file
     reads/writes (`read_dedup`, `write_dedup`, `reset_dedup`), lockfile operations, config load.
     Each exempted caller is named in the test as a documented carve-out per R7. A new runtime disk
     read outside these sites — the class of regression PR #33 introduced — fails CI. The test is
     brittle by design (text-grep over source): easy to update when an exemption is legitimately
     added, loud when a regression tries to sneak in unreviewed.

Tests:

- `invariants_no_distinct_over_chunks_source_file_outside_search_hybrid` — ripgrep-based text
  assertion. Low-effort automated lint.
- Re-run the existing `list_patterns_*`, `universal_patterns_*`, `stats_*`, `source_files_*` tests
  under the new backing queries; they must all pass unchanged (they assert behaviour).

**Done when:** all tests pass; the invariant-lint test greenlights; `just ci` clean.

### Unit 4 — Architecture doc, CONTRIBUTING link, CHANGELOG, ROADMAP

**Goal:** codify the invariant so it survives review churn.

Touched files:

- `docs/architecture.md` — new. Opens with a one-line invariants index. First section codifies
  _DB-as-sole-read-surface_ with the ingest exception explicitly named. Calls out the historical
  `render_pinned_conventions` disk-read as the motivating violation and notes the PR that restored
  it.
- `CONTRIBUTING.md` — add a pointer: _Architecture invariants that shape design choices live in
  `docs/architecture.md`. Read it before introducing a runtime disk read or a second read surface._
- `CHANGELOG.md` — entry under unreleased:

  > **feat: DB as sole runtime read surface.** Pattern bodies now live in a new `patterns` table;
  > `SessionStart` / `PostCompact` render from the DB instead of re-reading the source markdown.
  > **Breaking for existing knowledge bases:** schema bumps to v2. After upgrading, run
  > `lore ingest --force` once before your next session — `lore` will refuse to start otherwise with
  > a friendly advisory. `--force` is a destructive rebuild that re-embeds every chunk through
  > Ollama; budget time accordingly. Sandboxed read-only agents (e.g. nono.sh) no longer need
  > filesystem access to the patterns directory for the pinned-render path at session start. Agents
  > that call write tools (`add_pattern` / `update_pattern` / `append_to_pattern`) still need
  > patterns-directory write access because those tools write markdown to disk as the authoring
  > surface.

- `ROADMAP.md` — if there's a "Completed" section, note the PR; otherwise add a one-line entry under
  whatever current-work section exists.
- `docs/todos/pinned-conventions-render-from-db-not-disk.md` — delete (work is done).

Tests:

- No new tests beyond Unit 2 + Unit 3 + composition test coverage. Doc changes are verified by
  `dprint check` in `just ci`.

**Done when:** all files exist / are updated; todo is deleted; `just ci` clean; PR description reads
cleanly from the CHANGELOG.

## Alternative Approaches Considered

- **Bare `raw_body` column on chunks (or a two-column mini patterns table).** Rejected in the
  brainstorm. Shortest diff, but `list_patterns` / `universal_patterns` would still use
  DISTINCT-over-chunks gymnastics, recreating the same drift risk we're fixing.
- **Lossless chunks + reconstruct at render time.** Rejected in the brainstorm. Requires round-trip
  fidelity as a tested invariant; any regression silently corrupts pinned rendering.
- **Defer query migration to a later PR.** Rejected in the brainstorm. Two sources of truth for the
  same question is the class of drift this PR exists to prevent.
- **Add `sha2` or `blake3` for `content_hash`.** Rejected. `src/hash.rs::fnv1a` is already present
  and sufficient for non-cryptographic change detection. Adding a crypto-hash crate increases binary
  size without a need — the binary-size investigation is parked precisely because the baseline
  matters. Non-cryptographic naming (`content_hash` not `content_sha`) keeps the intent honest.
- **Foreign key from `chunks.source_file` to `patterns.source_file`.** Rejected. Zero FKs in the
  codebase today; adding one introduces pragma-`foreign_keys` considerations, cascade semantics, and
  a departure from the "DB as derived artefact" convention. The 1:1 invariant is enforced by the
  ingest layer through outer transactions; correctness comes from code, not schema constraints.
- **Split into two PRs: patterns-table + render migration, then a separate query-migration PR.**
  Rejected in the brainstorm. A transitional state with two sources of truth is exactly the class of
  drift we're fixing. One PR, one invariant, one migration.

## Dependencies & Risks

- **Schema rebuild requirement (user-facing).** Same shape as #33. The PR description's CHANGELOG
  entry is the operator-facing notice. The `PRAGMA user_version` probe is the runtime safety net.
  Users who already rebuilt for #33 will rebuild once more for this.
- **Rollback is NOT safe-by-default.** The `check_schema_compatibility` probe uses `>=` (not `==`):
  a v2 DB reopened under reverted v1 code silently _passes_ the probe, then runs v1 queries against
  a schema that has an orphan `patterns` table v1 doesn't know about. The v1 `clear_all` does not
  DROP `patterns`, so the orphan persists across subsequent full rebuilds — stale `raw_body` lingers
  forever. Correct rollback: revert + users **delete `knowledge.db` entirely** before re-ingesting
  under v1. The PR description must state this explicitly; users who assume naïve revert works will
  silently diverge. Future fix out of scope: tighten the probe to reject `stored_version > current`
  with its own "downgrade needs a fresh DB" advisory.
- **Full-ingest atomicity gap (pre-existing, carried forward).** `full_ingest` iterates files with
  per-file transactions; an Ollama outage mid-batch leaves the DB partially populated after the
  `clear_all` DROP+CREATE. Unchanged by this PR. Still tracked as the `full_ingest` backup-and-swap
  follow-up noted in #33's plan.
- **Storage growth on large knowledge bases.** Expected bound: ~60 KB on a 30-pattern base at ~2 KB
  avg body; ~400 KB worst case on a 100-pattern base with bodies near the 8 KB universal cap
  (overflow-page overhead included — 8 KB bodies exceed the default 4 KB page inline threshold and
  spill one overflow page per row). Neither FTS nor vec is touched by the new table. Measure on
  `lore-patterns` during implementation and note the actual number in the PR description.
- **`raw_body` drift from disk without re-ingest.** If a user edits a markdown file directly and
  starts a session without re-ingesting, `raw_body` is stale (same behaviour as `chunks` today —
  both go stale together). Delta ingest re-syncs. Documented.
- **Non-universal `raw_body` is unbounded.** The 8 KB `UNIVERSAL_BODY_HARD_LIMIT_BYTES` fires only
  when `is_universal = 1`. A 200 KB non-universal pattern now lives whole in `patterns.raw_body`.
  The render path filters `is_universal = 1` so this never lands in agent context. Future consumers
  of `raw_body` (e.g. the parked `get_pattern` MCP tool) must cap at read time or document the
  unbounded behaviour.
- **Concurrent ingest runs serialise cleanly.** `BEGIN IMMEDIATE` (R4) acquires the SQLite write
  lock at transaction open; a second writer blocks until the first commits, bounded by
  `busy_timeout = 5000 ms`. No deadlock risk — transaction ordering is uniform. Long holds caused by
  embedder-in-transaction are prevented by R4b.
- **`clear_all` DROP TABLE and concurrent readers.** During `lore ingest --force`, a reader on a
  separate connection (MCP server, hook) with an open prepared statement against `chunks` or
  `patterns` may observe `SQLITE_SCHEMA` / `SQLITE_BUSY` on its next step after the DROP commits.
  The existing hook "never break the agent" contract (`src/main.rs:523-529`, exit 0 on error)
  absorbs this; MCP server enters degraded mode per #33. No new handling required, but the PR
  description should note that running `lore ingest --force` alongside an active agent session may
  transiently drop the pinned section for that session (hook re-runs on next tool call succeed).

## Future Considerations

- **Delta-ingest short-circuit using `content_hash`.** Compare `patterns.content_hash` against
  `fnv1a(file_contents)` before re-indexing. Skip files with matching hashes entirely. Requires
  measurement on a representative knowledge base (the existing delta path uses git to gate
  re-indexing, so the gain may already be mostly captured).
- **`get_pattern(source_file)` MCP tool.** Now trivial to add — one
  `SELECT * FROM patterns
  WHERE source_file = ?`. Useful when an agent wants a specific pattern in
  full, not ranked search results.
- **`ingested_at` surfaced on `lore_status`.** Oldest row's `ingested_at` tells operators how stale
  their index is relative to their patterns repo. Opt-in metadata field under the existing
  `include_metadata` flag.
- **Move `is_universal` off the chunks table.** Currently both `chunks.is_universal` and
  `patterns.is_universal` exist. The chunk-side flag is still used for per-row ordering in
  `search_with_threshold`'s partition + dedup bypass. A future simplification could pull from
  `patterns` via JOIN — but the JOIN cost on every search is non-zero, and the current
  denormalisation is fine. Not urgent.
- **Invariant #2.** The architecture doc's invariants index is scaffolded for a second entry.
  Candidate: "Hooks never break the agent" (the existing `anyhow::Result` + exit-0 fail-safe pattern
  in `src/main.rs:523-529`).

## Sources & References

### Origin

- **Origin document:**
  [`docs/brainstorms/2026-04-21-read-surface-invariant-requirements.md`](../brainstorms/2026-04-21-read-surface-invariant-requirements.md).
  Key decisions carried forward: (1) new `patterns` table with `raw_body` + metadata (not a bare
  column on chunks); (2) query migration for `list_patterns`, `universal_patterns`, `stats`,
  `source_files` in the same PR; (3) R4 reconciliation semantics — 1:1 invariant, atomic single-file
  ingest; (4) invariant admits exactly one exception (ingest); (5) upgrade path mirrors #33
  (`SCHEMA_VERSION = 2`, `lore ingest --force` advisory, composition test as gate); (6)
  `ingested_at` + `content_hash` bundled into the same schema bump.

### Internal references

- Schema: `src/database.rs:97-167` (open, init, clear_all — extend for `patterns`), `:105-116` (open
  / open_skipping_schema_check pair), `:156-167` (`clear_all` transaction), `:172-190`
  (`delete_by_source` — compose inside new outer transaction), `:197-240` (`insert_chunk` — inner
  transactions fine; outer spans call site).
- Queries: `src/database.rs:346-378` (`list_patterns` — correlated subquery to migrate), `:381-387`
  (`universal_patterns` — Rust filter to migrate), `:430-445` (`stats` — `COUNT(DISTINCT)` to
  migrate), `:453-458` (`source_files` — `DISTINCT` to migrate), `:362` (`GROUP BY
  c1.source_file`
  inside `search_hybrid` — **leave alone**, documented exemption).
- Hook render: `src/hook.rs:532-611` (`PINNED_SECTION_TOTAL_LIMIT_BYTES` +
  `render_pinned_conventions` — migrate to DB).
- Hash helper: `src/hash.rs:14-23` (`fnv1a`) reused for `content_hash`.
- Ingest entry points: `src/ingest.rs` — `full_ingest`, `delta_ingest`, `ingest_single_file`,
  `add_pattern`, `update_pattern`, `append_to_pattern`; `enforce_universal_body_cap` (unchanged,
  still fires at ingest).
- Schema-probe entry point: `src/main.rs::cmd_ingest` via `should_skip_schema_probe`.
- Tests scaffolding: `tests/hook.rs` for integration coverage; `tests/single_file_ingest.rs` for
  atomicity; `src/database.rs::tests` for CRUD and query shape.

### Institutional learnings consulted

- [`docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`](../solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md)
  — R6's composition-test gate is the direct application of this learning. Probe → advisory → remedy
  path must be exercised end-to-end in one test.
- [`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`](../solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md)
  — R4's atomicity requirement guards against a new write path (patterns-table upsert) being
  silently undone by a concurrent or interleaved DB reader.
- [`docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`](../solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md)
  — tag-flip round-trip test (integration scenario #4) carries this forward for the `is_universal`
  column on the new table.

### Project conventions consulted

- `rust/sqlite.md` — "Treat the database as a derived artefact"; DROP+CREATE over ALTER; minimal FK
  usage. Confirmed by zero FK matches in `src/`.
- `rust/conventions.md` — binary-size watch; zero new dependencies for `content_hash`.
- `rust/tooling.md` — `just ci` as the single quality gate.
- `agents/unattended-work.md` — PR creation uses `--body-file /tmp/pr-body.md` (already active in
  the session).

### Related work

- PR #33 (`feat/universal-patterns`, merged at `569a1f6`) — introduced the violation this PR fixes;
  introduced `SCHEMA_VERSION`, `open_skipping_schema_check`, `clear_all` DROP+CREATE,
  `enforce_universal_body_cap`. This PR extends all four.
- `docs/todos/pinned-conventions-render-from-db-not-disk.md` — the parked P2 that this PR closes.
- `docs/todos/index-single-file-reconciliation-single-transaction.md` — P3 partially overlapping
  with R4. Planning confirms: R4 closes the patterns+chunks atomicity half; the embedder-
  outside-lock half remains open.

### Post-ship notes (2026-04-22)

What actually shipped on PR #34, for anyone reading this after merge.

**Delivered (R1-R9 all green).** Schema v2, `patterns` table, ingest refactor (embedder outside
transaction, 1:1 invariant enforced by new `delete_pattern_and_chunks_in_tx` /
`upsert_pattern_in_tx` / `insert_chunk_in_tx` helpers), `universal_patterns()` migrated to return
the new `UniversalPattern` shape with `raw_body`, `render_pinned_conventions` reads from DB, four
pattern-level queries migrated, `SCHEMA_VERSION = 2` + version-agnostic advisory, R4d
`debug_assert!` post-commit invariant check, `tests/invariants.rs` with two static-grep checks,
`docs/architecture.md` with the DB-as-sole-read-surface invariant + explicit trust-boundary clause,
`CONTRIBUTING.md` link, `CHANGELOG` entry with the rollback-is-unsafe warning, the two related
`docs/todos/` entries removed (one closed by this work, one closed by R4's atomicity).

**Deferred from plan, documented on PR.** `BEGIN IMMEDIATE` could not be used — rusqlite's
`transaction_with_behavior` requires `&mut Connection`, which the codebase's `KnowledgeDB` does not
hold, so the implementation falls back to `unchecked_transaction()` (`BEGIN DEFERRED`). The R4b
guarantee (embedder runs outside the transaction window) is unaffected, and the write-lock hold time
is measured in milliseconds regardless. `begin_immediate_tx`'s docstring notes the trade-off for
future readers.

**Bonus finding surfaced during smoke test.** Delta ingest doesn't reconcile against disk when an
MCP-authored file's lifecycle (`add_pattern` → `update_pattern` → `append_to_pattern` → `git
rm`)
cancels out in git-diff terms across the `last_ingested_commit..HEAD` window. The orphan row stays
in the DB until the next `lore ingest --force`. This is pre-existing (MCP single-file writes don't
bump `last_ingested_commit`) and out of this PR's scope, but captured as a learning: see
[`docs/solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md`](../solutions/best-practices/out-of-band-writers-bypass-delta-checkpoint-2026-04-22.md)
and
[`docs/todos/mcp-writes-bump-last-ingested-commit.md`](../todos/mcp-writes-bump-last-ingested-commit.md)
for the follow-up.

**Smoke-tested live** against the author's `lore-patterns` knowledge base on 2026-04-22: v1-DB
advisory fires correctly, `--force` rebuilds into v2 cleanly, patterns dir at chmod 000 still
renders pinned conventions (the original motivating sandbox case), MCP add/update/append round-trips
refresh `raw_body` in the DB, next `SessionStart` renders the new body, invariant lint catches a
planted drift regression. Nine scripted tests, all green.

### Brainstorm-to-plan notes

- **Skipped SpecFlow Analyzer (Step 3 of `/ce:plan`).** Context was saturated from the
  immediately-preceding `/ce:review` (#33 post-merge) + `/ce:brainstorm` + `document-review` passes
  within the same session. All edge cases in the requirements doc's R1-R8 + Deferred questions were
  already exercised.
- **Skipped external research (Step 1.5).** Strong local context; internal Rust refactor with a
  direct predecessor (#33). External best-practices would add cost without signal.
- **Resolved deferred questions inline:** FK shape (none), hash algorithm (`fnv1a`, rename
  `content_hash`), DISTINCT audit (four callers, within cap), architecture doc structure
  (invariants-index opener). Each resolution is documented in Technical Considerations above.
