---
date: 2026-04-21
topic: read-surface-invariant
---

# Restore DB-as-Sole-Read-Surface and Render Pinned Conventions from DB

## Problem Frame

PR #33 (`feat/universal-patterns`, squash-merged at `569a1f6`) introduced pinned universal pattern
rendering at `SessionStart` and `PostCompact`. `render_pinned_conventions` in `src/hook.rs` reads
pattern bodies from disk, re-opening a runtime read channel that the rest of the system does not
use.

Before #33, the SessionStart hook called `db.list_patterns()` to render a title/tag index. No body
reads. All runtime readers of indexed content — the MCP server, the PreToolUse/PostToolUse hooks,
the CLI subcommands — went through `knowledge.db`. The patterns directory was purely an authoring
and git surface: humans edit there, `add_pattern`/`update_pattern`/`append_to_pattern` write there,
and ingest reads it once to populate the DB.

The violation surfaced during a nono.sh sandbox test drive: universal patterns failed to inject
unless the sandbox was granted explicit filesystem access to the patterns directory, even though
`knowledge.db` was already reachable. The plugin/sandbox story — "an agent consuming lore needs
exactly one read surface" — silently stopped being true.

The parked todo `docs/todos/pinned-conventions-render-from-db-not-disk.md` (P2) captures the
symptom. This brainstorm elevates it to an architectural invariant, codifies it in a new
`docs/architecture.md`, and restores it by storing authorial pattern bodies in the DB.

## Requirements

- **R1.** A new `patterns` table is added, keyed by `source_file`, carrying `raw_body`, `title`,
  `tags`, `is_universal`, `ingested_at`, and `content_sha`. One row per pattern file. Chunks
  reference the corresponding `patterns` row; the exact referential mechanism (foreign key shape,
  cascade semantics) is a planning decision. The table is populated by every ingest path: full
  ingest, single-file ingest, and the write operations behind `add_pattern` / `update_pattern` /
  `append_to_pattern`.
- **R2.** No runtime disk reads remain in the `SessionStart` / `PostCompact` rendering path.
  `render_pinned_conventions` sources pattern bodies from the `patterns` table. The
  `validate_within_dir` containment check stays in ingest (where disk access is sanctioned) but is
  removed from the pinned render path.
- **R3.** `list_patterns` and `universal_patterns` query the `patterns` table directly. After this
  PR, no runtime query path reconstructs pattern-level state by scanning `chunks`. Planning audits
  for additional callers that group or distinct-over `chunks.source_file`; expected population is
  the two named above, and if the audit finds more than five additional call sites the excess is
  punted to a follow-up PR to keep this one bounded.
- **R4.** Every ingest path maintains a 1:1 invariant between `patterns` rows and pattern files
  present in the knowledge directory. Modifications replace the `raw_body` (and refresh
  `content_sha`, `ingested_at`) for an existing row; deletions remove the row; single-file re-ingest
  updates `patterns` and `chunks` atomically within one transaction so no reader ever observes a
  mismatched state. Full ingest's existing DROP+CREATE dance extends to `patterns` and remains
  atomic relative to reader visibility.
- **R5.** `SCHEMA_VERSION` bumps to `2`. The existing `user_version` probe in `KnowledgeDB::open`
  rejects old-schema DBs with the advisory "run `lore ingest --force` to rebuild the index with the
  new schema" — the same shape and remedy path as the `SCHEMA_VERSION = 1` bump in #33.
- **R6.** The composition test from the #33 post-review lesson
  (`docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`)
  is exercised for the new version: an old-schema DB, probe firing, `lore ingest --force`
  completing, new schema populated, hook rendering from DB only, zero pattern-directory file reads
  during render. This test is the acceptance gate for the PR.
- **R7.** A new `docs/architecture.md` is created, peer to `hook-pipeline-reference.md` /
  `search-mechanics.md` / `pattern-authoring-guide.md`. Its opening section codifies the
  read-surface invariant: _`knowledge.db` is the sole runtime read surface for indexed content; the
  patterns directory is an authoring/git surface only; ingest is the sole sanctioned disk→DB
  pipeline._ The section names the `render_pinned_conventions` disk-read (shipped in #33, removed in
  this PR) as the reason the doc exists, and is updated to "this invariant is now enforced" once the
  PR lands.
- **R8.** `CONTRIBUTING.md` links to `docs/architecture.md` so new contributors encounter the
  invariant before introducing the next "just this one case" disk read. The architecture doc itself
  lives in `docs/` outside the knowledge directory, so it is not ingested as a pattern.

## Success Criteria

- Running lore inside a sandbox (e.g. nono.sh) with filesystem access granted only to `knowledge.db`
  — and denied for the patterns directory — renders universal patterns correctly at `SessionStart`
  and `PostCompact`.
- The new composition test (R6) passes. Existing universal-patterns tests (oversized render cap,
  ANSI escape in tampered `source_file`, sibling expansion with mixed universal/non-universal) pass
  unchanged, because they assert behaviour not storage shape.
- `docs/architecture.md` exists and is reachable from `CONTRIBUTING.md`.
- `docs/todos/pinned-conventions-render-from-db-not-disk.md` is deleted (work is done). The plan doc
  notes that the invariant is now codified.

## Scope Boundaries

The following adjacent temptations are **out** of this PR. Each will be captured as a `docs/todos/`
entry if not already, so they remain visible but do not expand scope:

- **P3 single-transaction reconciliation in single-file ingest**
  (`docs/todos/index-single-file-reconciliation-single-transaction.md`) — per explicit guidance,
  kept separate. Note: R4's atomicity requirement may overlap; planning clarifies whether this PR
  resolves the P3 todo as a side-effect or leaves it genuinely untouched.
- **Delta-ingest short-circuit using `content_sha`.** The column lands in R1 but is not read by
  delta-ingest in this PR. That's a performance change, not an invariant change, and needs its own
  measurement.
- **New `get_pattern(source_file)` MCP tool.** Useful future capability once bodies are in the DB,
  but nothing in this PR requires it. Leaves a clean entry point for a later "read a specific
  pattern by file" use case.
- **CLI read-surface audit.** During planning, confirm `lore list`, `lore search`, `lore status`
  already go through DB (expected: they do); if any read disk at runtime, migrate them in the same
  PR. If all are clean, this is not a scope expansion.

## Key Decisions

- **Storage shape: proper `patterns` table, not a bare `raw_body` column.** The schema cost is
  small. The carrying cost of `DISTINCT`-over-chunks in two hot queries (`list_patterns`,
  `universal_patterns`) goes away. Future per-pattern metadata has a natural home. Aligns with "DB
  as derived but _complete_ artefact".
- **Bundle `ingested_at` and `content_sha` into this schema bump even though neither has a reader
  yet.** Each `SCHEMA_VERSION` increment forces every user to rebuild under the compatibility-check
  lesson's advisory flow. One bump carrying three columns is cheaper than three bumps carrying one
  each. `ingested_at` enables future diagnostics (e.g. `lore_status` showing oldest row);
  `content_sha` enables future delta-ingest short-circuiting. Carrying cost: a few bytes per row, no
  runtime path exercises either column in this PR. Accepted as low-cost polish that compounds when
  successors land.
- **Query migration in the same PR, not a later cleanup.** Two sources of truth for "which patterns
  exist?" is the exact class of drift that produced the bug we are fixing. Shipping the new table
  with one reader (the hook) and leaving the old DISTINCT queries in place recreates the problem at
  a smaller scale.
- **Invariant admits exactly one exception: ingest.** The architecture doc states "runtime reads via
  DB; ingest is the sole sanctioned disk→DB pipeline". Any future runtime disk read requires either
  widening this exception (and justifying why) or a new exception with a named reason.
- **Upgrade path matches #33.** Same `lore ingest --force` advisory, same `SCHEMA_VERSION` bump
  semantics, same schema-probe bypass for the sanctioned remedy path, same composition test shape.
  Users who already rebuilt for #33 will rebuild once more for this.

## Dependencies / Assumptions

- `cliff.toml` / `CHANGELOG.md` entry needed for the schema bump (same shape as #33's entry).
- `add_pattern` / `update_pattern` / `append_to_pattern` already trigger a re-ingest of the modified
  file; R1/R4 rely on that existing behaviour to keep the `patterns` row current.
- **Rollback requires the same advisory pattern.** If this PR is reverted post-merge, users who
  rebuilt under `SCHEMA_VERSION = 2` hold a DB the reverted code (`v1`) rejects. Rollback guidance:
  revert + re-run `lore ingest --force` to rebuild under `v1`. This is symmetric with any
  schema-bumping PR and should be noted in the PR description.
- The new `docs/architecture.md` lives outside the knowledge directory, so the ingest pipeline does
  not index it as a pattern. No `.loreignore` entry required.

## Outstanding Questions

### Resolve Before Planning

None. All product-level decisions are above.

### Deferred to Planning

- `[Affects R1][Needs research]` Measure `raw_body` storage cost on `lore-patterns` before
  committing. A rough sanity check: universal patterns are capped at 8 KB per file
  (`UNIVERSAL_BODY_HARD_LIMIT_BYTES`), and non-universal bodies are bounded by reasonable
  pattern-length norms. Total DB growth should be well under 1 MB for typical knowledge bases, but
  planning should produce a concrete number.
- `[Affects R1][Technical]` Decide the exact foreign-key shape between `chunks` and `patterns`
  (cascade on delete, or leave chunks to be cleared by the full-ingest `DROP TABLE` dance). Lean
  toward cascade to simplify single-file re-ingest semantics and to satisfy R4 without extra
  bookkeeping.
- `[Affects R1][Technical]` Decide the `content_sha` hash algorithm and input. Options: SHA-256 vs
  blake3 (existing crate availability, hash stability, speed on 8 KB inputs); whole-file vs
  post-frontmatter body (affects whether frontmatter-only edits change the sha). The choice binds
  any future delta-ingest reader, so it's worth getting right now even with no in-PR consumer.
- `[Affects R3][Technical]` Audit all DB query sites that currently touch `chunks` with `DISTINCT`
  or `GROUP BY source_file`. Expect only `list_patterns` and `universal_patterns`, but confirm.
- `[Affects R7]` Decide whether `docs/architecture.md` opens with a one-line "invariants index" so
  that when a second invariant lands later, the doc structure already accommodates it without a
  rewrite. Stylistic, low-stakes.

## Next Steps

→ `/ce:plan` for structured implementation planning
