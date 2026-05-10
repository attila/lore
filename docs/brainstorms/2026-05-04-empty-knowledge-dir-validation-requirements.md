---
date: 2026-05-04
topic: empty-knowledge-dir-validation
---

# Empty Knowledge-Directory Detection

> **Refined 2026-05-10.** The original brainstorm proposed fail-fast on empty knowledge directories
> with an opt-in `--allow-empty-knowledge` flag. After applying the project's CLI behaviour ladder
> (three-tier: hard-fail / warn / silent — see project memory), this case classifies as **tier 2:
> warn, exit 0**, not tier 1 (fail). The flag is dropped. This document supersedes that framing. The
> file name is kept for stability of cross-references; the H1 reflects the refined scope
> ("Detection", not "Validation").

## Problem Frame

Lore expects a knowledge directory containing markdown files indexed into `knowledge.db`. When the
_effective_ scan set is empty — either because the directory has no markdown files, or because
`.loreignore` excludes every candidate — the ingest pipeline silently reports success with zero
files indexed. The user thinks indexing happened, finds nothing on search, and has no clear signal
why. This is the silent-failure mode the CLI behaviour ladder's tier-2 warn is designed to surface.

A code scan against today's `src/ingest.rs` (commit `f78d061`, branch `main`) shows asymmetric
handling already in place:

- **All-ignored case** (`.loreignore` swallows every file) — already warns at
  `src/ingest.rs:691-693`:
  `Warning: .loreignore matched every markdown file; nothing will be indexed`.
- **Filesystem-empty case** (no `.md` files at all) — currently silent. `discover_md_files`
  (`src/ingest.rs:655-672`) emits `Found 0 markdown files` and the pipeline proceeds to a
  zero-result ingest. The existing test `ingest_empty_directory_returns_zero` (`src/ingest.rs:1652`)
  asserts the zero-result contract but no user-facing signal.

The fix is to **unify the two cases under a single tier-2 warning**, mirror it at MCP server
startup, and surface the state in the existing `lore_status` report so monitoring and agents can
detect it without parsing stderr.

## Requirements

**R1 — Effective-Empty Warning**

`full_ingest` emits a clear warning via `on_progress` when the effective scan set is empty after
`.loreignore` filtering, regardless of cause. The exact wording is up to implementation; the message
must:

- Distinguish "no `.md` files at all" from "all `.md` files excluded by `.loreignore`" — either as
  one unified message or two distinct messages. The deferred question below resolves which.
- Replace the existing partial warning at `src/ingest.rs:691-693` so the two paths cannot
  double-fire.
- Fire exactly once per ingest run.

Exit status remains `0`. No error, no abort.

**R2 — MCP Server Startup**

`cmd_serve` (`src/main.rs:511`) emits the same warning to stderr at boot when the effective scan set
is empty. The MCP server proceeds to serve regardless — refusing to start would block diagnostic and
agent-introspection use. The check fires once at startup; per-request re-checks are out of scope.

**R3 — `lore_status` Reporting**

The MCP `lore_status` tool (`handle_lore_status` at `src/server.rs:973-1014`) gains two fields in
its JSON metadata:

- `empty_knowledge_dir: bool` — true when the _effective_ scan set on disk is empty (either cause).
  This reports disk state, distinct from `sources_indexed`, which reports the post-ingest database
  state. The two diverge when the user has files on disk but hasn't run ingest yet.
- `knowledge_dir_status: "empty" | "populated"` — derived from the bool, for human readability when
  `lore status` (CLI, `src/main.rs:126-127`) renders the metadata.

The same fields surface in the human-facing `lore status` CLI output.

**R4 — Documentation**

A short note in the README and the relevant clap doc-comments (`Commands::Ingest` around
`src/main.rs:76-83`) describes the warning behaviour. No new flag, no new sub-command, no new doc
file. The original plan referenced `docs/usage.md`, which does not exist in the repo.

**R5 — Test Coverage**

- Inline unit tests in `src/ingest.rs` (alongside `ingest_empty_directory_returns_zero` at line
  1652):
  - Filesystem-empty fires the warning via captured `on_progress` calls.
  - All-ignored fires the warning (replacing today's partial coverage in spirit; the existing
    assertion in `full_ingest_skips_files_matched_by_loreignore` at line 2748 stays).
  - Populated dir does _not_ fire the warning (positive control).
- Integration test in `tests/edge_cases.rs` via `assert_cmd`: empty directory → exit 0, warning on
  stderr.
- `lore_status` MCP test: assert `empty_knowledge_dir: true` and `knowledge_dir_status: "empty"` for
  an effective-empty dir; `false`/`"populated"` otherwise. Add alongside existing `lore_status`
  tests in `src/server.rs`.

Test layout follows project convention (`rust/testing-strategy.md`): inline `#[cfg(test)] mod tests`
for unit, flat `tests/*.rs` for integration. Not `tests/unit/` or `tests/integration/`.

## Success Criteria

- `lore ingest` on an effective-empty directory exits 0 with a visible stderr warning explaining the
  cause.
- `lore serve` on the same directory boots, prints the warning once, and serves.
- `lore_status` reports `empty_knowledge_dir: true` and `knowledge_dir_status: "empty"` for an
  effective-empty dir; `false` and `"populated"` otherwise.
- The existing partial warning at `src/ingest.rs:691-693` is replaced by the unified path; no
  double-firing.
- All new tests pass under `just ci`.

## Scope Boundaries

- **No flag.** No `--allow-empty-knowledge`, no silencer. Per the CLI behaviour ladder, silencer
  flags train users to mask the signal the warning was designed to provide. If a concrete user
  report later requires suppression, add it then with deliberate naming.
- **No exit-status change.** Effective-empty stays `exit 0`. Tier 2.
- **No placeholder file.** No README scaffold, no auto-write. The directory is left as the user gave
  it.
- **No HTTP `/health` endpoint.** The original brainstorm assumed one existed; lore's only "server"
  is the MCP JSON-RPC server over stdio. The state is exposed via the existing `lore_status` MCP
  tool and the `lore status` CLI command.
- **No change to `add_pattern` / `update_pattern` / `append_to_pattern`.** These already handle
  empty knowledge directories via existing paths and are out of scope.
- **No broader edge-case audit.** Other paths (database corruption, partial writes during crash,
  server runtime without restart) are not in scope.

## Key Decisions

- **Tier 2, not tier 1.** Per the CLI behaviour ladder, effective-empty is a
  coherent-but-possibly-unintended state. Continuing produces a recoverable result (empty index,
  empty search) and the user course-corrects on the next run. Fail-fast would force opt-out flags
  whose accumulation reintroduces the silent-failure mode this work is fixing.
- **Effective-empty, not filesystem-empty.** The trigger fires after `.loreignore` filtering. This
  unifies today's asymmetric handling and matches the user-visible failure mode (a populated repo
  with a too-broad `.loreignore` produces the same silent-zero-result as a literally empty dir).
- **No silencer flag.** YAGNI. Add only on a concrete user report, with rename —
  `--no-empty-warning` is honest about what it does; `--allow-…` implies permission for a state that
  does not need permission.
- **Surface via `lore_status`, not a new endpoint.** The original brainstorm speculated a `/health`
  HTTP endpoint that does not exist. The actual surface is the MCP `lore_status` tool plus the
  `lore status` CLI command. Adding fields there is a one-line change in `handle_lore_status` and
  matches existing precedent (`loreignore_active`, `delta_ingest_available`).
- **Field name `empty_knowledge_dir` reports disk state, not index state.** `sources_indexed: 0`
  already exists in `lore_status` and reports database state. The new field reports _disk_
  effective-empty, which diverges when the user has files but hasn't ingested. Both are useful; the
  new field complements the existing one.

## Dependencies / Assumptions

- `clap` v4 derive (`#[derive(Parser)]` + `#[arg(...)]`) is the existing CLI pattern in
  `src/main.rs`. No clap changes are needed because no flag is added. (The original plan's
  pseudocode used the deprecated `App::arg` builder pattern; that error has been removed in this
  refinement.)
- `on_progress` callbacks are the canonical way to surface user-visible messages from `full_ingest`
  (line 692 already uses this path for the partial warning).
- `cmd_serve` and `cmd_ingest` can share an "is effective-empty" helper exposed from `lore::ingest`,
  called from both surfaces.
- `assert_cmd` is already in `dev-dependencies`.
- `ingest::full_ingest` accepts `on_progress: &dyn Fn(&str)`, giving inline tests a hook to capture
  messages without mocking.

## Outstanding Questions

### Resolve Before Planning

_(none. The original brainstorm's three open questions — flag name, downstream guards, placeholder
warning — are all resolved by the warn-only pivot. Downstream guards specifically are dropped: a
code read of `full_ingest` (`src/ingest.rs:678-734`) confirms zero-files iterates an empty vector
cleanly with a zero-result `IngestResult`; search against a zero-pattern index returns zero results
without panic. No defensive guards are needed; pre-emptive guards in `src/search.rs` and
`src/pattern.rs` would be cargo-cult.)_

### Deferred to Planning

- [Affects R1] One unified warning vs two distinct messages for the two causes. Both are acceptable;
  tests assert firing, not wording. Suggested default: two distinct messages, because the recovery
  action differs (add a `.md` file vs relax `.loreignore`).

## Next Steps

→ `/ce:plan` produced; see
[`docs/plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md`](../plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md)
for the implementation plan, anchored against `src/ingest.rs:691-693` (existing partial warning to
replace), `src/ingest.rs:1652` (existing test point), `src/server.rs:973-1014` (`lore_status`
handler), and `src/main.rs:511` (`cmd_serve`).
