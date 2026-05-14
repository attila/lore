---
date: 2026-05-14
topic: schema-migration-strategy
status: convention
---

# Schema Migration Strategy

Ground rules for lore database schema bumps. Derived from the practice of v1→v2 (`is_universal`,
hard-bail), v2→v3 (`applies_when_json`, silent additive), and v3→v4 (`language_json`, silent
additive — this convention's instigating bump).

Sibling to `cli-behaviour-ladder-2026-05-10.md`. Future schema bumps consult this doc rather than
re-deciding from scratch.

## Foundational principle: the DB is a derived artefact

Per the project's SQLite convention (`agents/sqlite.md`):

> Treat the database as a derived artefact. The database is rebuilt from source markdown via
> `lore ingest`. Never store authoritative data in it — it is safe to delete and regenerate at any
> time. This means migration tooling is unnecessary; schema changes are applied by re-ingesting.

Schema migrations cohere with this principle in one important way: **the ALTER TABLE or DROP+CREATE
is purely a cache-shape change. Data always comes from re-ingest of source markdown.** Both
operational strategies below honour this.

The convention is not a ban on `ALTER TABLE`; it is a ban on transforming existing authoritative
data inside the DB. ALTER TABLE that adds a nullable column doesn't transform data — it opens an
empty slot. Subsequent `lore ingest` populates that slot from source markdown.

## Two operational strategies

Choose by answering one question: **does the new binary operate correctly against an unmigrated DB
whose new column is NULL on existing rows?**

### Silent in-place additive

Use when:

- The schema change is purely additive: new column(s) added with `ALTER TABLE`.
- The new column is nullable, no default.
- The new binary has an explicit fallback path for NULL on the new column (i.e., rows with NULL
  behave correctly under the new code path).
- No data transformation is required on existing rows.

User UX: **no message at migration time**. The ALTER TABLE runs at binary startup, takes
milliseconds, prints nothing. Subsequent commands continue normally.

Example: v2→v3 added `applies_when_json TEXT NULL` to `chunks` and `patterns`. Old rows with NULL
behave as "no predicate" in the predicate evaluator — graceful fallback. New patterns can declare
predicates; old patterns are unaffected until re-ingested.

Example: v3→v4 (this slice) added `language_json TEXT NULL` to `chunks` and `patterns`. Old rows
with NULL fall back to the FTS-coincidence retrieval path per the slice's plan R10 — graceful
fallback. The structural retrieval gate only activates for rows where `language_json` is populated,
which happens via normal re-ingest.

### Hard-bail with `lore ingest --force`

Use when:

- The schema change isn't additive: column rename, type change, table split or merge, index rebuild,
  FTS5 tokenizer change.
- The new binary requires the new data to operate correctly (no graceful NULL fallback).
- A backfill or transformation is required on existing rows.

User UX: friendly advisory prints, binary refuses to serve queries until the user runs
`lore ingest --force`. Established wording:

```
lore: this database was written by an older version of lore (schema vN < vM).
Run `lore ingest --force` to rebuild the index with the new schema.
This is expected after upgrading; see CHANGELOG for details.
```

Example: v1→v2 added `is_universal` with no fallback path; the engine required the field populated
to decide injection behaviour. Existing rows with no value would behave incorrectly, so the binary
hard-bails and requires `lore ingest --force` to rebuild the cache from source markdown.

### When both are technically possible

Prefer silent in-place additive. It's zero user friction and preserves operational continuity. The
hard-bail path is the right tool when the schema change is structurally non-additive or when the new
behaviour can't tolerate NULL.

## UX contract

- **Silent migration:** no user-visible message at migration time. Migration is invisible. The new
  column is NULL on existing rows until re-ingest re-populates them.
- **Hard-bail:** friendly advisory prints, binary refuses to serve queries until
  `lore ingest --force` completes. One bump → one advisory variant; no per-bump custom wording
  unless the user-facing action genuinely differs.
- **Never silent migration with data loss or transformation.** The DB is the cache; authoritative
  data lives in source markdown.
- **Never partial-state.** The DB is at exactly one schema version after migration; no half-migrated
  intermediate states. Either the ALTER TABLE completes atomically or the bail fires before any
  change.
- **Both strategies rely on re-ingest as the data path.** ALTER TABLE is cache-shape only;
  `lore ingest` is what fills the cache from source.

## Code-structure contract

Every schema bump must satisfy (formalised from
`docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`
and the v2→v3 / v3→v4 implementations):

1. **Shared DDL constants.** `CHUNKS_DDL`, `PATTERNS_DDL`, `PATTERNS_FTS_DDL` (or equivalents) are
   single-source-of-truth `const`s used by both `init` and `clear_all`. Adding a column to a DDL
   constant automatically propagates to clear-and-recreate paths.
2. **DROP+CREATE in `clear_all`,** not `DELETE FROM` — the latter doesn't pick up new columns.
3. **`open_skipping_schema_check` variant** for the `--force` remedy path, so the schema probe
   doesn't block the remedy itself.
4. **`should_skip_schema_probe(force, file)` helper** with truth-table unit tests — the four-cell
   matrix (force × file presence) must produce the correct skip/no-skip decision.
5. **Idempotency for in-place additive bumps:** use `column_exists()` guards around each
   `ALTER TABLE` so re-running migration on an already-migrated DB is a no-op.
6. **Atomic migration:** wrap the in-place additive `ALTER TABLE` statements in `BEGIN IMMEDIATE` /
   `COMMIT` so partial states aren't observable. Bump `PRAGMA user_version` in the same transaction.
7. **Remedy-completion integration test** for hard-bail bumps: use `Command::cargo_bin("lore")`
   against a raw-SQL old-schema fixture; assert the probe fires with the expected advisory, run
   `lore ingest --force`, assert the probe accepts the new state. Grep the test suite for the exact
   advisory string; zero hits = red flag (the test isn't pinned to the message users actually see).

## Versioning contract

- `SCHEMA_VERSION` is an integer constant in `src/database.rs`, monotonically increasing.
- `PRAGMA user_version` in the DB stores the current schema version.
- Bumps land in the same commit as the feature requiring them — no split commits.
- Schema versions are never decremented or reused.
- Pre-v1 databases (no `PRAGMA user_version`) are treated as v0 and route to the hard-bail path.

## Documentation contract

Every schema bump produces:

- A `CHANGELOG.md` entry per the project's CHANGELOG convention (user-facing, one assertive-voice
  sentence, ends with `(#N)`).
- A history comment in `src/database.rs` (currently around line 626) documenting the bump, its
  migration type, and what column or structure changed.
- For hard-bail bumps: a "what to do after upgrading" note in CHANGELOG making the
  `lore ingest --force` action explicit.
- For changes to the data model layout (new tables, new columns affecting reader layout): a line in
  `docs/architecture.md` if the engine-module shape changes.

## Testing contract

Every schema bump must include:

- Fresh-init test: a new DB initialises directly at the new version.
- Hand-built old-schema fixture test for each prior version that still migrates (not the hard-bail
  path's pre-v1 default).
- For in-place additive: confirm existing rows continue working with NULL on the new column.
- For hard-bail: the remedy-completion integration test described in the code-structure contract.
- Regression sweep: full `cargo test` passes with no existing test broken.

## Bump history

For reference and worked-example value:

| From | To | Type            | Column / Change                                                | Fallback / Remedy                                                               |
| ---- | -- | --------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| v0   | v1 | Initial         | Initial schema                                                 | n/a                                                                             |
| v1   | v2 | Hard-bail       | `is_universal INTEGER NOT NULL DEFAULT 0` on chunks / patterns | New binary required field populated; no graceful NULL state; `--force` required |
| v2   | v3 | Silent additive | `applies_when_json TEXT NULL` on chunks / patterns             | Predicate evaluator treats NULL as "no predicate" → graceful                    |
| v3   | v4 | Silent additive | `language_json TEXT NULL` on chunks / patterns                 | Retrieval falls back to FTS-coincidence per slice R10 when NULL → graceful      |

## When to revisit this doc

- After every schema bump: add a row to the bump history table.
- If a new strategy emerges that doesn't fit either silent-additive or hard-bail (e.g., a "silent
  rebuild on first run" pattern — currently unused), document it as a third strategy with its own UX
  and code contracts.
- If the foundational "DB is a derived artefact" principle changes (e.g., lore starts storing
  authoritative data in the DB), revise the principle section first.
