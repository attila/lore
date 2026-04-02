---
title: "feat: Delta ingest via git diff"
type: feat
status: active
date: 2026-04-02
---

# feat: Delta ingest via git diff

## Overview

Replace the full-database-clear-and-reindex behavior of `lore ingest` with a delta-based approach
that uses `git diff --name-status` to detect which files changed since the last successful ingest.
Only changed, added, renamed, and deleted files are re-processed, eliminating Ollama round-trips for
unchanged files. A `--force` flag preserves access to the current full-reindex behavior.

## Problem Frame

Every `lore ingest` call currently clears all three database tables and re-embeds every markdown
file through Ollama. For a knowledge base with 50+ patterns, this takes significant time — each file
requires an HTTP round-trip to Ollama for embedding. Since the knowledge base is a git repo, we can
use git's change detection to skip files that haven't changed, reducing ingest time from O(all
files) to O(changed files).

## Requirements Trace

- R1. `lore ingest` defaults to delta mode — only processes files changed since the last successful
  ingest
- R2. `lore ingest --force` performs a full re-index (current behavior)
- R3. Delta detection uses `git diff --name-status` against a stored commit SHA
- R4. Handle all git status codes: Added (A), Modified (M), Deleted (D), Renamed (R)
- R5. Store the last-ingested commit SHA in a metadata table in SQLite
- R6. Graceful fallback to full ingest when delta is not possible (no stored commit, not a git repo,
  stored commit missing from history)
- R7. `lore init` always performs full ingest (first time, no delta possible)
- R8. Progress output distinguishes delta from full mode and reports skip counts

## Scope Boundaries

- Not changing the chunking or embedding logic — only the file selection layer
- Not adding config options for delta behavior beyond `--force`
- Not tracking per-file content hashes — git diff is the sole change detection mechanism
- Not changing how `lore init` works beyond recording the commit SHA after ingest

## Context & Research

### Relevant Code and Patterns

- `src/ingest.rs:76-154` — current `ingest()` function: walks dir, calls `db.clear_all()`, processes
  all files
- `src/ingest.rs:377-410` — `index_single_file()`: already implements per-file delete-and-reindex
- `src/database.rs:147-167` — `delete_by_source()`: deletes all chunks for a given source file
- `src/database.rs:137-144` — `clear_all()`: nukes all three tables
- `src/git.rs:32-39` — `is_git_repo()`: checks if directory is a git repo
- `src/git.rs:51-63` — `git_output()`: runs a git command and returns stdout
- `src/main.rs:50-51` — `Commands::Ingest` variant (currently no args)
- `src/main.rs:236-266` — `cmd_ingest()` function

### Institutional Learnings

- No prior delta ingest learnings in `docs/solutions/`
- ROADMAP.md explicitly lists this as "Up Next" with the same approach described

## Key Technical Decisions

- **Metadata table over config file**: Store `last_ingested_commit` in a new `ingest_metadata`
  SQLite table rather than the TOML config. The DB is the artifact that needs to stay in sync with
  the commit — coupling them in the same store prevents drift.
- **`git diff --name-status` over content hashing**: Git already tracks exactly what changed.
  Content hashing would require reading every file on every ingest to compare — defeating the
  purpose.
- **Fallback to full ingest is silent except for a one-line message**: When delta isn't possible
  (first run, missing commit, non-git dir), the user sees "Full ingest (no previous commit
  recorded)" rather than an error.
- **Record commit SHA after successful ingest only**: If ingest fails partway, the old SHA stays —
  next run will re-process the same delta plus any new changes.
- **Reuse `index_single_file()` for per-file processing**: The existing helper already handles
  delete-old-chunks + re-chunk + re-embed + re-insert. Delta ingest calls it for each changed file
  instead of clearing everything first.

## Open Questions

### Resolved During Planning

- **Should renamed files get re-embedded or just have their source_file updated?** Re-embed via
  `index_single_file()`. The chunk IDs include the source path, so renaming changes IDs. Simpler and
  safer to delete-old + index-new than to try to update IDs in place across three tables.
- **Should `lore init` record the commit SHA?** Yes. After the initial full ingest succeeds, record
  HEAD so subsequent `lore ingest` calls can delta.

### Deferred to Implementation

- Exact error message wording for fallback scenarios
- Whether `git diff` needs `--find-renames` threshold tuning (start with git defaults)

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

```
lore ingest [--force]
  │
  ├─ --force? ──────────────────────────────────► full_ingest()
  │                                                 ├─ clear_all()
  │                                                 ├─ walk + chunk + embed all
  │                                                 └─ store HEAD as last_commit
  │
  └─ delta path
       ├─ load last_commit from ingest_metadata
       │    ├─ None? ────────────────────────────► full_ingest()
       │    └─ Some(sha)
       │         ├─ is_git_repo()? No ───────────► full_ingest()
       │         ├─ commit exists? No ───────────► full_ingest()
       │         └─ git diff --name-status sha..HEAD
       │              ├─ no changes ─────────────► "Already up to date"
       │              ├─ A/M files ──────────────► index_single_file() each
       │              ├─ D files ────────────────► delete_by_source() each
       │              ├─ R files ────────────────► delete_by_source(old) + index_single_file(new)
       │              └─ store HEAD as last_commit
       └─ report: N files changed, M skipped
```

## Implementation Units

- [ ] **Unit 1: Add `ingest_metadata` table to database**

**Goal:** Create a key-value metadata table in SQLite for storing ingest state.

**Requirements:** R5

**Dependencies:** None

**Files:**

- Modify: `src/database.rs`
- Test: `src/database.rs` (inline tests)

**Approach:**

- Add `CREATE TABLE IF NOT EXISTS ingest_metadata (key TEXT PRIMARY KEY, value TEXT)` to
  `KnowledgeDB::init()`
- Add `pub fn get_metadata(&self, key: &str) -> anyhow::Result<Option<String>>` method
- Add `pub fn set_metadata(&self, key: &str, value: &str) -> anyhow::Result<()>` method
- Use `INSERT OR REPLACE` for upsert semantics

**Patterns to follow:**

- `KnowledgeDB::init()` for table creation pattern
- `delete_by_source()` for simple parameterized queries

**Test scenarios:**

- Happy path: set a key, get it back, verify value matches
- Happy path: set a key twice, second value overwrites first
- Happy path: get a non-existent key returns None
- Edge case: set empty string value, get it back as Some("")

**Verification:**

- `get_metadata` / `set_metadata` round-trip correctly in tests
- `init()` still succeeds on existing databases (IF NOT EXISTS)

- [ ] **Unit 2: Add git diff helper to `git.rs`**

**Goal:** Add a function that runs `git diff --name-status` between two commits and parses the
output into structured change records.

**Requirements:** R3, R4

**Dependencies:** None

**Files:**

- Modify: `src/git.rs`
- Test: `src/git.rs` (inline tests)

**Approach:**

- Add a `FileChange` enum: `Added(String)`, `Modified(String)`, `Deleted(String)`,
  `Renamed { from: String, to: String }`
- Add
  `pub fn diff_name_status(repo_dir: &Path, from_commit: &str) -> anyhow::Result<Vec<FileChange>>`
- Run `git diff --name-status <from_commit>..HEAD` via existing `git_output()`
- Parse each line: status code tab-separated from path(s)
- Filter to only `.md` / `.markdown` files (matching ingest's extension filter)
- Add `pub fn head_commit(repo_dir: &Path) -> anyhow::Result<String>` — wraps `git rev-parse HEAD`
- Add `pub fn commit_exists(repo_dir: &Path, sha: &str) -> bool` — wraps `git cat-file -t <sha>`

**Patterns to follow:**

- `git_output()` for running git commands and capturing stdout
- `is_git_repo()` for simple boolean git checks

**Test scenarios:**

- Happy path: add a file, commit, diff shows Added
- Happy path: modify a file, commit, diff shows Modified
- Happy path: delete a file, commit, diff shows Deleted
- Happy path: rename a file (git mv), commit, diff shows Renamed with from/to
- Edge case: no changes between commits returns empty vec
- Edge case: non-markdown files are filtered out
- Error path: invalid from_commit returns error
- Happy path: `head_commit()` returns current HEAD SHA
- Happy path: `commit_exists()` returns true for valid SHA, false for bogus

**Verification:**

- All status codes (A/M/D/R) correctly parsed in real git repos (tempdir tests)

- [ ] **Unit 3: Implement delta ingest logic in `ingest.rs`**

**Goal:** Add a `delta_ingest()` function that processes only changed files, and refactor the
existing `ingest()` to become `full_ingest()` (called by delta when fallback is needed).

**Requirements:** R1, R2, R3, R4, R5, R6, R8

**Dependencies:** Unit 1, Unit 2

**Files:**

- Modify: `src/ingest.rs`
- Test: `src/ingest.rs` (inline tests)

**Approach:**

- Rename current `ingest()` to keep it as the full-ingest path, but add a
  `db.set_metadata("last_ingested_commit", &head)` call at the end on success
- Add new public `ingest()` entry point that:
  1. Check if `knowledge_dir` is a git repo → if not, fall back to full with message
  2. Load `last_ingested_commit` from metadata → if None, fall back to full
  3. Check if stored commit exists in history → if not, fall back to full with message
  4. Run `git::diff_name_status()` to get changes
  5. If empty, report "Already up to date" and return early
  6. For each change: `Added`/`Modified` → `index_single_file()`, `Deleted` →
     `db.delete_by_source()`, `Renamed` → delete old + index new
  7. On success, store new HEAD commit
- Extend `IngestResult` with `mode: IngestMode` enum (`Full`, `Delta { unchanged: usize }`) for
  progress reporting
- Keep `full_ingest()` as a separate public function for `--force` and `lore init`

**Patterns to follow:**

- Existing `index_single_file()` for per-file processing
- `ingest()` progress callback pattern

**Test scenarios:**

- Happy path: delta ingest with one added file — only that file is indexed, others untouched
- Happy path: delta ingest with one deleted file — chunks removed, other files untouched
- Happy path: delta ingest with modified file — old chunks replaced with new
- Happy path: delta ingest with renamed file — old source deleted, new source indexed
- Happy path: no changes since last commit — returns early with zero files processed
- Happy path: full ingest records HEAD commit in metadata
- Edge case: non-git directory falls back to full ingest
- Edge case: missing stored commit falls back to full ingest
- Edge case: first-ever ingest (no metadata) falls back to full ingest
- Integration: delta ingest after full ingest produces correct cumulative state

**Verification:**

- Delta ingest only calls embedder for changed files (not unchanged)
- Database contains correct chunks after delta (no orphans, no missing)
- Metadata table has correct commit SHA after each ingest variant

- [ ] **Unit 4: Add `--force` flag to CLI and wire up delta/full dispatch**

**Goal:** Add `--force` flag to `lore ingest` command and update `cmd_ingest()` and `cmd_init()` to
use the new ingest functions.

**Requirements:** R2, R7, R8

**Dependencies:** Unit 3

**Files:**

- Modify: `src/main.rs`
- Test: `tests/` (integration tests if they exist, otherwise CLI smoke test)

**Approach:**

- Add `#[arg(long)] force: bool` to `Commands::Ingest`
- In `cmd_ingest()`: if `force`, call `full_ingest()`; otherwise call `ingest()` (delta entry point)
- In `cmd_init()`: keep calling `full_ingest()` directly (always full on init)
- Update progress output to show mode: "Delta ingest..." vs "Full ingest..."
- Show summary: "Done: 3 files changed, 47 unchanged" for delta, or existing output for full

**Patterns to follow:**

- Existing clap derive patterns in `Commands` enum
- `cmd_ingest()` progress callback to stderr

**Test scenarios:**

- Happy path: `lore ingest` without `--force` uses delta mode
- Happy path: `lore ingest --force` uses full mode
- Happy path: progress output shows delta/full mode indicator
- Happy path: `lore init` always uses full ingest and records commit

**Verification:**

- `--force` flag accepted by clap parser
- Delta is default, force triggers full
- `lore init` records commit SHA so subsequent `lore ingest` can delta

## System-Wide Impact

- **Interaction graph:** `cmd_ingest()` and `cmd_init()` are the only callers of the ingest
  pipeline. MCP server and hook pipeline don't call ingest. Write operations (`add_pattern`,
  `update_pattern`, `append_to_pattern`) use `index_single_file()` independently and are unaffected.
- **Error propagation:** If delta ingest encounters a git error mid-processing, it should still
  store the commit SHA only on full success. Partial failures leave the old SHA so the next run
  retries.
- **State lifecycle risks:** The metadata table and the chunk data must stay consistent. Recording
  the commit SHA only after successful completion ensures this.
- **API surface parity:** The MCP tools (`add_pattern`, `update_pattern`, etc.) don't trigger full
  ingest. They already do single-file indexing. No MCP changes needed.
- **Unchanged invariants:** Search, hook pipeline, MCP server, and write operations are completely
  unaffected. They read from the same three tables regardless of how data was ingested.

## Risks & Dependencies

| Risk                                                                   | Mitigation                                                                         |
| ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Git history rewrite makes stored SHA unreachable                       | `commit_exists()` check with fallback to full ingest                               |
| Rename detection misses a move (git sees delete+add instead of rename) | Both delete+add and rename produce correct final state — just costs an extra embed |
| Large delta after long gap could be slow                               | Acceptable — still faster than full reindex; user can see progress                 |
| Non-git knowledge dirs lose delta capability                           | Graceful fallback to full ingest with informational message                        |

## Sources & References

- Related code: `src/ingest.rs`, `src/database.rs`, `src/git.rs`, `src/main.rs`
- ROADMAP.md "Up Next" item describing this feature
