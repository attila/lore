---
title: "feat: .loreignore — gitignore-style exclude file for pattern repositories"
type: feat
status: completed
date: 2026-04-06
origin: docs/brainstorms/2026-04-05-loreignore-requirements.md
---

# feat: .loreignore — gitignore-style exclude file for pattern repositories

## Overview

Add support for a `.loreignore` file at the root of pattern repositories. Files and directories
matching the patterns in this file are excluded from indexing during both full and delta ingest.
When `.loreignore` changes, delta ingest runs a reconciliation pass to remove stale entries from the
database.

## Problem Frame

Every `.md` file in a pattern repository is indexed, including repository documentation (README,
CONTRIBUTING, LICENSE) and tooling directories (.github/). These pollute the knowledge database with
chunks that dilute search relevance. Users have no way to exclude files without removing them from
the repository. (See origin: `docs/brainstorms/2026-04-05-loreignore-requirements.md`)

## Requirements Trace

- R1. `.loreignore` at repository root specifies exclusions
- R2. Gitignore-style glob syntax: bare filenames, trailing-slash directories, wildcards, recursive
  globs, anchoring rules, negation patterns (`!` to un-ignore)
- R3. Purely opt-in — no `.loreignore` means no filtering
- R4. `.loreignore` itself is never indexed (already excluded by markdown-only filter)
- R5. Comments (`#`), empty lines, malformed patterns (warn and skip)
- R6. Full ingest filters matched files during directory walk
- R7. Delta ingest filters matched files from diff output
- R8. Delta ingest detects `.loreignore` changes and reconciles stale entries
- R9. `.loreignore` deletion: no files removed, previously ignored files re-indexed on next full
  ingest
- R10. Debug logging for skipped files
- R11. Debug logging for reconciliation removals

## Scope Boundaries

- No nested `.loreignore` in subdirectories
- No MCP tool interface changes — `add`, `append`, `update` are unaffected
- `.loreignore` path is not configurable
- Un-ignoring files (removing patterns from `.loreignore`) does not re-index them during delta
  ingest — requires a full re-ingest or file modification. This is a known v1 limitation

## Context & Research

### Relevant Code and Patterns

- `src/ingest.rs:247` — `full_ingest()`: `WalkDir` file discovery, `db.clear_all()` before re-index
- `src/ingest.rs:165` — `delta_ingest()`: processes `Vec<FileChange>`, handles Add/Modify/Delete/
  Rename
- `src/ingest.rs:558` — `index_single_file()`: shared helper for chunk/embed/insert
- `src/git.rs:269` — `diff_name_status()`: parses `git diff --name-status`, filters by
  `is_markdown_path()`
- `src/git.rs:230` — `is_markdown_path()`: private, checks `.md`/`.markdown` extension
- `src/database.rs:145` — `clear_all()`: drops and recreates FTS/vec tables
- `src/database.rs:162` — `delete_by_source()`: deletes all chunks for a source file
- `src/database.rs:333` — `list_patterns()`: returns per-source summaries (heavier than needed for
  reconciliation)
- `src/database.rs:418` — `get_metadata()`/`set_metadata()`: key-value store in `ingest_metadata`
- `src/debug.rs:34` — `lore_debug!` macro: `[lore debug]` prefixed, gated by `LORE_DEBUG`
- `Cargo.toml` — current deps include `walkdir = "2"`, no glob/ignore crates

### Institutional Learnings

- Delta ingest only sees committed files via `git diff --name-status` — uncommitted changes are
  invisible (see
  `docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-
  testing-2026-04-05.md`)
- Before adding a crate, inspect its exports and verify API compatibility with current dependency
  versions (see `docs/solutions/build-errors/sqlite-vec-no-rust-export-register-via-ffi.md`)
- Security hardening bounded all file reads — apply same convention to `.loreignore` (see commit
  `18ac741`)

## Key Technical Decisions

- **Crate: `ignore`** — the `ignore` crate (by BurntSushi, same author as `walkdir` and `ripgrep`)
  provides `ignore::gitignore::Gitignore` which parses gitignore-style files and matches paths. This
  handles trailing-slash directory matching, anchoring rules, recursive globs, and negation patterns
  natively. We use `Gitignore` for pattern matching only, keeping the existing `WalkDir` walk
  intact. This is a smaller integration surface than replacing `WalkDir` with `ignore::WalkBuilder`

- **`.loreignore` change detection via content hash:** Store an FNV-1a hash of `.loreignore`
  contents in `ingest_metadata` (key: `loreignore_hash`). On each delta ingest, read and hash the
  current `.loreignore`, compare to the stored hash. If different (or file added/deleted), trigger
  the reconciliation pass. This avoids modifying `diff_name_status()` or the `FileChange` enum

- **Reconciliation via database scan:** Add `source_files() -> Vec<String>` to `KnowledgeDB` (simple
  `SELECT DISTINCT source_file FROM chunks`). During reconciliation, check each source file against
  the ignore list and `delete_by_source()` for matches. Pattern repos are small, so this is a
  bounded operation

- **Renamed file handling:** Apply ignore check to `from` and `to` independently. If `to` is
  ignored: delete `from` chunks, skip indexing `to`. If `from` was ignored (no chunks): just index
  `to`. If both ignored: skip entirely. If neither: normal rename

- **Bounded read:** Limit `.loreignore` to 64 KiB, consistent with the transcript tail bounded read
  pattern

- **Hash sentinel for absent `.loreignore`:** Use empty string (`""`) consistently. When
  `.loreignore` is absent, store `""`. When present, store the content hash. Both full and delta
  ingest compare against this value. No `delete_metadata` method needed

- **Disk read for `.loreignore`, not git show:** Read `.loreignore` from the working tree, not via
  `git show HEAD:.loreignore`. This is simpler, matches full ingest behaviour (which also reads from
  disk), and avoids an extra git subprocess call. Users should commit `.loreignore` before running
  ingest for consistent results, matching the existing delta ingest convention

- **Reconciliation runs before FileChange processing:** When `.loreignore` has changed, run the
  reconciliation pass first (remove stale entries), then process the FileChanges from the diff. This
  ensures a clean database state before applying new changes

## Open Questions

### Resolved During Planning

- **Crate choice:** `ignore` crate, using `Gitignore` struct for matching (not `WalkBuilder`)
- **`.loreignore` change detection:** Content hash in metadata, not git diff inspection
- **Database query for reconciliation:** New `source_files()` method, not reusing `list_patterns()`
- **Rename edge cases:** Independent ignore checks on `from` and `to` paths
- **Un-ignore behaviour:** Accept limitation for v1, document it

### Deferred to Implementation

- Exact `ignore` crate version to pin (check latest compatible with Rust 2024 edition)
- Whether `Gitignore::new()` or `GitignoreBuilder` is the right constructor for our use case
- Final hash function choice — FNV-1a is used for session deduplication but `fnv1a_hash` is private
  to `hook.rs`. Either extract it to a shared `pub(crate)` utility, duplicate the 4-line
  implementation, or use `std::collections::hash_map::DefaultHasher`

## Implementation Units

- [x] **Unit 1: Add `ignore` crate and `.loreignore` parsing module**

**Goal:** Add the `ignore` dependency and create a reusable module for reading and matching against
`.loreignore` patterns.

**Requirements:** R1, R2, R5

**Dependencies:** None

**Files:**

- Modify: `Cargo.toml`
- Create: `src/loreignore.rs`
- Modify: `src/lib.rs` (add module declaration)
- Test: `src/loreignore.rs` (inline `#[cfg(test)] mod tests`)

**Approach:**

- Add `ignore` crate to `Cargo.toml`
- Create `src/loreignore.rs` with a function that takes a `knowledge_dir` path, reads `.loreignore`
  if present, and returns an `Option<ignore::gitignore::Gitignore>` matcher
- Apply 64 KiB bounded read before parsing
- Handle comments, empty lines, and malformed patterns (warn to stderr, skip invalid lines)
- Return `None` when no `.loreignore` file exists (R3 backward compatibility)
- Provide a helper function
  `is_ignored(gitignore: &Gitignore, rel_path: &str, is_dir: bool) ->
  bool` for use by both ingest
  paths. Check `Gitignore::matched()` result — `Match::Ignore` means excluded, `Match::Whitelist`
  (negation) means explicitly included, `Match::None` means no match

**Patterns to follow:**

- `src/debug.rs` for module structure
- Security hardening bounded reads (commit `18ac741`)
- `src/embeddings.rs` for `#[cfg(test)]` test utility pattern (`FakeEmbedder`)

**Test scenarios:**

- Happy path: `.loreignore` with `README.md`, `docs/`, `*.txt` correctly matches expected files
- Happy path: patterns with trailing slash match directories only
- Happy path: anchored patterns (containing `/`) match from root only
- Happy path: recursive globs (`**/*.draft.md`) match in subdirectories
- Edge case: `.loreignore` file does not exist — returns `None`
- Edge case: `.loreignore` contains only comments and blank lines — zero effective patterns
- Edge case: `.loreignore` exceeds 64 KiB — warn and return `None` (no filtering), consistent with
  the bounded-read security convention
- Error path: malformed glob pattern — warning emitted, other patterns still work
- Happy path: negation pattern (`!important.md`) un-ignores a file matched by an earlier pattern
  (`*.md`)
- Edge case: negation without a preceding exclusion — file is not ignored (no-op)
- Edge case: pattern that would match `.loreignore` itself — irrelevant since `.loreignore` has no
  `.md` extension, but verify no panic

**Verification:**

- Module compiles, all unit tests pass
- `cargo deny` passes with new dependency

---

- [x] **Unit 2: Add `source_files()` to KnowledgeDB**

**Goal:** Expose a lightweight query to enumerate all distinct source files in the database.

**Requirements:** R8 (reconciliation support)

**Dependencies:** None (can be done in parallel with Unit 1)

**Files:**

- Modify: `src/database.rs`
- Test: `src/database.rs` (inline tests)

**Approach:**

- Add `pub fn source_files(&self) -> anyhow::Result<Vec<String>>` to `KnowledgeDB`
- Query: `SELECT DISTINCT source_file FROM chunks ORDER BY source_file`
- Uses the existing `idx_chunks_source_file` index

**Patterns to follow:**

- `list_patterns()` at line 333 for query structure
- `stats()` at line 399 for simple aggregate queries

**Test scenarios:**

- Happy path: database with 3 source files returns all 3 in sorted order
- Edge case: empty database returns empty vec
- Happy path: after `delete_by_source()`, removed file no longer appears

**Verification:**

- Method returns correct results in existing ingest test infrastructure

---

- [x] **Unit 3: Integrate `.loreignore` filtering into full ingest**

**Goal:** Full ingest skips files matched by `.loreignore` during the directory walk.

**Requirements:** R6, R10

**Dependencies:** Unit 1

**Files:**

- Modify: `src/ingest.rs` (in `full_ingest()`)
- Test: `src/ingest.rs` (inline tests)

**Approach:**

- At the start of `full_ingest()`, call the `.loreignore` parser with `knowledge_dir`
- After the existing WalkDir collection and markdown extension filter, apply the ignore matcher to
  each relative path
- Emit `lore_debug!` for each skipped file with the matched pattern
- Store the `.loreignore` content hash in metadata after successful ingest (for delta ingest
  comparison)
- Log a warning when filtering results in zero files to index

**Patterns to follow:**

- Existing WalkDir filter chain at line 261-272
- `lore_debug!` usage in `src/hook.rs`

**Note:** `clear_all()` does not clear `ingest_metadata`, so the `loreignore_hash` from a previous
ingest survives. `full_ingest` must always write the current hash (or clear it if no `.loreignore`
exists) to keep the value in sync.

**Test scenarios:**

- Happy path: `.loreignore` with `README.md` — README is not indexed, other files are
- Happy path: `.loreignore` with `docs/` — entire directory excluded
- Happy path: no `.loreignore` — all markdown files indexed (backward compatibility)
- Edge case: `.loreignore` that matches all markdown files — zero files indexed, warning logged
- Edge case: `.loreignore` present but empty — no filtering applied
- Happy path: negation in full ingest — `*.md` + `!important.md` excludes all markdown except
  `important.md`
- Integration: `db.stats().sources` matches expected count after filtering

**Verification:**

- Full ingest skips matched files
- Unmatched files are indexed as before
- `loreignore_hash` metadata is stored

---

- [x] **Unit 4: Integrate `.loreignore` filtering into delta ingest with reconciliation**

**Goal:** Delta ingest skips matched files, detects `.loreignore` changes, and reconciles stale
entries.

**Requirements:** R7, R8, R9, R10, R11

**Dependencies:** Units 1, 2 (Unit 3 can be developed in parallel; hash-storage contract is shared
but not blocking)

**Files:**

- Modify: `src/ingest.rs` (in `delta_ingest()` and `ingest()`)
- Test: `src/ingest.rs` (inline tests)

**Approach:**

- In the `ingest()` entry point, before calling `delta_ingest()`, read and hash `.loreignore`
- Compare hash to stored `loreignore_hash` metadata
- Pass the ignore matcher into `delta_ingest()`
- Extend `delta_ingest` signature to accept `ignore: Option<&ignore::gitignore::Gitignore>` as a
  parameter. The matcher is constructed in `ingest()` and passed through
- In the `FileChange` processing loop:
  - `Added`/`Modified`: skip if path is ignored, log with `lore_debug!`. Do not increment
    `result.files_processed` for skipped changes
  - `Deleted`: process normally (deleting chunks for an ignored file is harmless)
  - `Renamed { from, to }`: check each side independently — skip delete if `from` was ignored (no
    chunks), skip index if `to` is ignored
- The `.loreignore` hash comparison and reconciliation happen inside the git-repo branch of
  `ingest()`, after the `is_git_repo` guard, so non-git repos remain unaffected
- When `.loreignore` hash differs from stored value (or was added/deleted):
  - Run reconciliation: call `source_files()`, check each against ignore matcher,
    `delete_by_source()` for matches
  - Emit `lore_debug!` for each removal
  - Update stored hash
- When `.loreignore` is deleted: reconciliation finds no matches (no ignore list), update stored
  hash to empty/absent
- Log progress message on every `.loreignore` change (not just removals) noting that any un-ignored
  files require `lore ingest --force` to re-index
- Compute the `unchanged` file count after filtering ignored changes from the `existing_changed`
  calculation to avoid deflated counts

**Patterns to follow:**

- Existing `delta_ingest()` structure at line 165
- `FNV-1a` hashing pattern from session deduplication (or standard `std::hash`)
- Metadata storage via `set_metadata()`

**Test scenarios:**

- Happy path: delta ingest with `.loreignore` — new file matching pattern is skipped
- Happy path: delta ingest without `.loreignore` — all changes processed normally
- Happy path: `.loreignore` added between commits — reconciliation removes matching indexed files
- Happy path: `.loreignore` modified to add new pattern — newly matched files removed
- Happy path: `.loreignore` deleted — no files removed, hash cleared
- Edge case: `.loreignore` is the only file that changed — reconciliation still runs
- Edge case: renamed file where `to` is ignored — old chunks deleted, new file not indexed
- Edge case: renamed file where `from` was ignored — no old chunks to delete, new file indexed
- Happy path: reconciliation with negation — file matches exclude but is un-ignored by `!`, should
  not be removed during reconciliation
- Edge case: `.loreignore` modified to add negation for previously excluded file — file is not
  re-indexed (known v1 limitation), warning fires
- Error path: malformed pattern during reconciliation — warning emitted, valid patterns still
  applied
- Integration: `db.stats().sources` reflects correct count after reconciliation

**Verification:**

- Delta ingest filters changes correctly
- Reconciliation removes stale entries when `.loreignore` changes
- Hash metadata stays in sync with `.loreignore` state

---

- [x] **Unit 5: Integration tests**

**Goal:** End-to-end tests covering the full lifecycle across both ingest paths.

**Requirements:** All

**Dependencies:** Units 3, 4

**Files:**

- Create: `tests/loreignore.rs`
- Create: `tests/fixtures/loreignore/` (test fixture directory)

**Approach:**

- Integration tests using the standard `tempdir + git_init + FakeEmbedder` pattern
- Test the full ingest → delta ingest → reconciliation lifecycle
- Verify search results reflect the filtering

**Patterns to follow:**

- `tests/e2e.rs` for CLI integration test structure
- `src/ingest.rs` inline tests for the tempdir + git pattern

**Test scenarios:**

- Happy path: full ingest with `.loreignore` excluding README.md — search does not return README
  chunks
- Happy path: add `.loreignore` after initial ingest, run delta — previously indexed README chunks
  removed
- Happy path: modify `.loreignore` to exclude additional file, run delta — file's chunks removed
- Happy path: delete `.loreignore`, run delta — no chunks removed, next full ingest re-indexes
  everything
- Edge case: `.loreignore` with `*.md` excluding everything — zero chunks, warning in output
- Happy path: negation pattern un-ignores a specific file — that file appears in search results
  while other excluded files do not
- Edge case: compound `.loreignore` edit — same change adds a new exclusion and negates another.
  Excluded file's chunks are removed, un-ignored file is not re-indexed until full ingest
- Edge case: repository with no `.loreignore` — identical behaviour to current

**Verification:**

- All integration tests pass
- `just ci` passes

---

- [x] **Unit 6: Documentation updates**

**Goal:** Update product documentation to cover `.loreignore` syntax, behaviour, and examples.

**Requirements:** All (documentation surface)

**Dependencies:** Units 3, 4 (feature must be implemented before documenting)

**Files:**

- Modify: `docs/pattern-authoring-guide.md`
- Modify: `docs/configuration.md`
- Modify: `docs/search-mechanics.md`
- Modify: `README.md`

**Approach:**

- `docs/pattern-authoring-guide.md`: add a subsection to the file structure and grouping section
  explaining `.loreignore` for excluding non-pattern files (README, LICENSE, CI config). Include a
  practical example `.loreignore` file
- `docs/configuration.md`: add `.loreignore` to the configuration surface with syntax reference,
  supported patterns (bare filenames, directories, wildcards, recursive globs, negation), and note
  the 64 KiB size limit
- `docs/search-mechanics.md`: mention in the ingest pipeline section that `.loreignore` filters
  files before chunking, and that delta ingest reconciles stale entries when the file changes
- `README.md`: add brief mention in the documentation table or features list

**Patterns to follow:**

- Existing documentation style: Oxford grammar, British English, full words over abbreviations,
  italics for emphasis, plain blockquotes for asides
- `docs/pattern-authoring-guide.md` existing "File structure and grouping" section for placement

**Test expectation:** none — documentation only

**Verification:**

- `dprint fmt` passes on all modified docs
- Cross-references between docs are consistent

## System-Wide Impact

- **Interaction graph:** `.loreignore` is read-only input to the ingest pipeline. No callbacks,
  hooks, or MCP tools are affected. The `lore hook` subcommand operates on search results, which
  will reflect the filtered index
- **Error propagation:** Malformed patterns warn to stderr and are skipped. File read errors for
  `.loreignore` itself should warn and fall back to no-filtering (graceful degradation)
- **State lifecycle risks:** The `loreignore_hash` metadata value must stay in sync. If it gets
  corrupted or lost, the worst case is an unnecessary reconciliation pass on the next delta ingest —
  not data loss
- **API surface parity:** MCP tools (`add_pattern`, `update_pattern`, `append_to_pattern`) are
  explicitly unaffected. They write to caller-specified paths and bypass the ingest pipeline
- **Unchanged invariants:** The markdown-only extension filter (`is_markdown_path`) remains in
  place. `.loreignore` layers additional filtering on top — it never widens what gets indexed

## Risks & Dependencies

| Risk                                                                | Mitigation                                                                                                                           |
| ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `ignore` crate adds transitive dependencies, increasing binary size | LTO + strip + opt-level z already in release profile. Measure binary size before/after. The crate is well-maintained and widely used |
| Reconciliation pass on large databases could be slow                | Pattern repos are small (typically <100 files). `source_files()` uses an indexed column. Acceptable for v1                           |
| Un-ignoring files requires full re-ingest                           | Documented as known limitation. Log a message during reconciliation. Revisit if user feedback indicates this is a pain point         |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-04-05-loreignore-requirements.md](docs/brainstorms/2026-04-05-loreignore-requirements.md)
- Related code: `src/ingest.rs`, `src/git.rs`, `src/database.rs`
- `ignore` crate: https://crates.io/crates/ignore (by BurntSushi)
- Security hardening: commit `18ac741`
- Session deduplication (FNV-1a pattern): `src/hook.rs`
