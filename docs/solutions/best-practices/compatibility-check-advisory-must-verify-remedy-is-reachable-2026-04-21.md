---
title: "Compatibility-check advisory must verify its advised remedy is reachable"
date: 2026-04-21
category: best-practices
module: db
problem_type: best_practice
component: schema-compatibility-probe
severity: medium
applies_when:
  - "Adding a probe or startup check that emits a user-facing advisory naming a specific command or flag as the remedy"
  - "Designing an upgrade / migration path where the remedy is invoked through the same CLI entry point whose ingress is guarded by the probe"
  - "Writing CHANGELOG entries that promise a path to recovery via an existing subcommand"
  - "Reviewing a PR that adds a check-and-bail pattern whose bail message points at a different code path"
tags:
  - compatibility-probe
  - advisory-remedy-reachability
  - schema-migration
  - composition-test
  - self-contradicting-advisory
  - force-rebuild
  - clear-all-ddl
  - end-to-end-test-gap
related_issues:
  - "lore#33"
---

# Compatibility-check advisory must verify its advised remedy is reachable

## Problem

The universal-patterns change added an `is_universal` column to `chunks` in `src/database.rs`, and
`KnowledgeDB::open` grew a startup probe that rejects pre-migration databases with the advisory "Run
`lore ingest --force` to rebuild the index with the new schema." Three pieces were individually
correct — the probe detects the old schema, the advisory names the canonical rebuild command, and
`clear_all` had long been the mechanism `full_ingest` used to wipe state — but their composition
inside `cmd_ingest` (in `src/main.rs`) was broken. `cmd_ingest` opened the database through the
probing constructor before dispatching to `full_ingest`, so the probe fired on exactly the path the
probe's own error message told users to take. And even if the probe were bypassed, `clear_all`
deleted rows with `DELETE FROM chunks` rather than recreating the table, so the stale column list
survived and the next `insert_chunk` would fail with "no such column: is_universal". The subtlety
was that each component's unit tests passed: the probe correctly rejected old schemas, `clear_all`
correctly emptied the table, and `full_ingest` correctly rebuilt from sources. Only their
composition on the advertised remedy path was wrong.

## Investigation steps that led to discovery

The ce-review `data-integrity-guardian` agent traced the user-visible remedy end-to-end instead of
auditing each function in isolation. Starting from the probe's error text in `src/database.rs`, it
followed the advised command `lore ingest --force` into `cmd_ingest` in `src/main.rs`, noted that
`KnowledgeDB::open` runs before the `force` flag is consulted, and recognised the circular failure:
the advisory told users to run a command that the advisory itself prevented from running. A second
pass examined what would happen if the probe were suppressed — reading `clear_all` showed
`DELETE FROM chunks`, which the agent flagged against the newly added column, predicting the "no
such column" failure on the subsequent insert. Both findings were schema-composition issues
invisible to tests that fixtured a fresh DB.

## Root cause

A compatibility check that emits "run X to fix" must verify that X is reachable and that X will
actually succeed. The probe embedded a remedy in its error message but never confirmed (a) that the
remedy path was exempt from the probe itself, or (b) that the remedy's implementation was compatible
with the schema the probe was guarding. Whenever a diagnostic names a specific command as the
sanctioned cure, that command becomes part of the diagnostic's contract, and any code that gates or
implements the cure is now load-bearing on the diagnostic's truthfulness.

This is the same hazard class documented in
[`composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`](composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md)
and
[`filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`](filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md):
a new guard or advisory is added, but the existing paths around it are not audited for whether they
honour the new contract. The advisory-reachability variant is especially pernicious because the
check and the remedy can each be tested in isolation and pass — the bug only exists at their seam.

A related operational-composition case is
[`reload-plugins-does-not-restart-mcp-servers-2026-04-03.md`](../integration-issues/reload-plugins-does-not-restart-mcp-servers-2026-04-03.md)
where the UX message claimed a remedy succeeded when the remedy could not reach its target process;
and
[`session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md`](../logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md),
where a gate that would deny-first-touch had to be composed with a dedup pass to avoid infinite
denial — mirroring the probe's infinite-block behaviour without `open_skipping_schema_check`.

## Fix

Commit `e2741bf` on branch `feat/universal-patterns`. Three changes, each testable independently;
the load-bearing test is the one that composes them.

### A. Split the constructor so the remedy path can opt out

```rust
pub fn open(db_path: &Path, dimensions: usize) -> anyhow::Result<Self> {
    Self::open_inner(db_path, dimensions, /* check_schema */ true)
}

pub fn open_skipping_schema_check(db_path: &Path, dimensions: usize) -> anyhow::Result<Self> {
    Self::open_inner(db_path, dimensions, false)
}
```

The skip variant is reserved for the single sanctioned remedy path; its doc comment names the
invariant explicitly so a future reader cannot repurpose it casually.

### B. `clear_all` rebuilds the table, not just its rows

```rust
const CHUNKS_DDL: &str = "\
    CREATE TABLE IF NOT EXISTS chunks (
        id TEXT PRIMARY KEY,
        /* ... */
        is_universal INTEGER NOT NULL DEFAULT 0 CHECK (is_universal IN (0, 1)),
        ingested_at TEXT DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_chunks_source_file ON chunks(source_file)";

pub fn init(&self) -> anyhow::Result<()> {
    self.conn.execute_batch(CHUNKS_DDL)?;
    /* ... */
}

pub fn clear_all(&self) -> anyhow::Result<()> {
    let tx = self.conn.unchecked_transaction()?;
    tx.execute_batch("DROP TABLE IF EXISTS chunks")?;
    tx.execute_batch(CHUNKS_DDL)?;
    /* ... */
    tx.commit()?;
    Ok(())
}
```

Sharing the DDL between `init` (fresh-DB path) and `clear_all` (rebuild path) via a single const
eliminates drift. Every future column addition lands in one place.

### C. Explicit skip-probe decision in `cmd_ingest`

```rust
fn should_skip_schema_probe(force: bool, file: Option<&Path>) -> bool {
    force && file.is_none()
}

let db = if should_skip_schema_probe(force, file) {
    KnowledgeDB::open_skipping_schema_check(&config.database, ollama.dimensions())?
} else {
    KnowledgeDB::open(&config.database, ollama.dimensions())?
};
```

Extracted as a helper so the four-cell truth table can be unit-tested; only `--force && !file`
rebuilds the schema, every other combination must still probe. Single-file `--force` (overriding
`.loreignore`) explicitly stays on the probed path.

## Why unit tests passed while integration broke

Each component was tested against a freshly initialised schema, so none of the tests could witness
the old-schema state the probe was built to detect. The probe's unit tests supplied contrived
old-schema fixtures but never followed the advisory through `cmd_ingest`; `clear_all`'s tests
operated on tables that already had `is_universal`, so `DELETE` looked indistinguishable from
`DROP+CREATE`; and `full_ingest`'s tests started from `init`, not from a stale on-disk DB. The
composition — old-schema DB meets `cmd_ingest --force` meets `clear_all` — was the one state no unit
test set up. The regression test at commit `cda011b`
(`knowledge_db_open_skipping_schema_check_bypasses_probe_for_force_ingest`) closes that gap by
exercising the full probe, skip, clear, and re-probe loop against a DB built via raw SQL in the
pre-migration shape, which is the only fixture that makes the bug reproducible.

## Prevention checklist

- If a probe emits `"run <cmd>"`, add a test that invokes `<cmd>` against the exact state the probe
  flagged and asserts recovery.
- Treat the advisory string as an API contract: changing the command text must update the remedy
  test in the same commit.
- For destructive remedies (`--force`, `--reset`, `DROP`), gate the remedy on a bypass path that the
  probe itself cannot block, and test that bypass.
- When the probe and remedy live in different crates/modules, write at least one integration test
  that imports both and runs them end-to-end in a single process.
- Verify the remedy leaves the system in a state the probe now accepts — not just "remedy exits 0".
- Re-read the CHANGELOG / upgrade notes you wrote; every `lore <subcommand>` mentioned as a recovery
  path needs a matching `#[test]` that runs it.
- If the remedy requires a flag to suppress the probe, assert the flag is honoured _and_ that
  omitting the flag still produces the advisory (both directions).
- Prefer one composition test over three unit tests when the bug surface is the seam between them.

## The remedy-completion test pattern

A remedy-completion test is a single integration test that walks the full user-recovery arc: build
the broken state, confirm the probe fires the exact advisory, execute the advisory verbatim, then
re-run the probe and a functional smoke check. Step 1 forces you to materialise the failure mode in
code (not mocks), which catches drift between the probe's predicate and the real-world condition.
Step 2 pins the advisory text. Step 3 must invoke the command the way a user would — including any
flag parsing — because that is where `ingest --force` hit the probe wall. Step 4 is load-bearing: it
asserts the remedy _dissolves_ the condition that triggered the probe and that the system is usable
afterward. Skipping step 4 is how PR #33 shipped green.

```rust
#[test]
fn ingest_force_completes_on_old_schema_db() {
    // 1. Construct the flagged state.
    let db = tmp_db_with_schema(OLD_SCHEMA_VERSION);

    // 2. Assert the check rejects it with the expected advisory.
    let err = KnowledgeDB::open(db.path(), 4).unwrap_err();
    assert!(err.to_string().contains("run `lore ingest --force`"));

    // 3. Run the remedy exactly as the advisory describes it.
    Command::cargo_bin("lore")
        .unwrap()
        .args(["ingest", "--force", "--config", db.config_path()])
        .assert()
        .success();

    // 4. Probe passes AND system is functionally restored.
    let store = KnowledgeDB::open(db.path(), 4).expect("probe should now accept");
    assert!(store.list_patterns().is_ok());
}
```

## Signals a composition gap exists

- The advisory string names a CLI command — grep the test suite for that exact command; zero hits is
  a red flag.
- The probe and the remedy are in different modules and no test file imports both.
- Unit tests for the probe use hand-crafted error states; no test builds the state via the real code
  path that produces it in production.
- CHANGELOG / README describes a recovery procedure in prose but the PR adds no integration test.
- The remedy accepts a flag (`--force`, `--yes`) whose sole purpose is bypassing the probe — check
  that the bypass is wired through, not just parsed.
- Component tests all pass, but no test's name contains both the probe's concept and the remedy's
  command.

## References

- PR: https://github.com/attila/lore/pull/33
- Fix commit: `e2741bf` (clear_all rebuild + open_skipping_schema_check + should_skip_schema_probe
  helper)
- Regression-test commit: `cda011b` (composition test + `should_skip_schema_probe` matrix)
- Plan:
  [`docs/plans/2026-04-20-001-feat-universal-patterns-plan.md`](../../plans/2026-04-20-001-feat-universal-patterns-plan.md)
  — Post-review changes section
- Related: `composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`,
  `filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`,
  `reload-plugins-does-not-restart-mcp-servers-2026-04-03.md`
