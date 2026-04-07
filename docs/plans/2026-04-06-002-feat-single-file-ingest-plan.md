---
title: "feat: Single-file ingest (`lore ingest --file`)"
type: feat
status: completed
date: 2026-04-06
completed: 2026-04-06
pr: attila/lore#31
---

# feat: Single-file ingest (`lore ingest --file`)

## Overview

Add `lore ingest --file <path>` so pattern authors can index a single markdown file without
committing it first. Today the only way to re-index a file is `lore ingest` (delta against committed
history) or `lore ingest --force` (full walk). Both require a git commit for the edit to be visible
to search, which is the main friction point in the edit → ingest → search feedback loop during
pattern authoring.

Single-file ingest bypasses git entirely for the write path: it reads the named file from disk,
validates it lies inside the knowledge directory, and upserts its chunks into the database. It does
**not** update `last_ingested_commit`, does **not** walk the repository, and does **not** run
`.loreignore` reconciliation. It is orthogonal to delta state.

## Problem Frame

From ROADMAP.md:

> Single-file ingest (`lore ingest --file <path>`) — index one file without requiring a git commit,
> enabling a fast edit-ingest-search feedback loop for pattern authoring. Removes the current
> workaround of committing a WIP before testing discoverability. Update the vocabulary coverage
> technique section in `docs/pattern-authoring-guide.md` when shipped.

The existing vocabulary coverage workflow (`docs/pattern-authoring-guide.md:160-189`) literally
prescribes "commit the pattern and run an ingest" as step 2. That workaround is captured in
`docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`.
Single-file ingest eliminates it and unblocks the Pattern QA skill (also in Up Next), which
automates the same loop.

## Requirements Trace

- **R1.** `lore ingest --file <path>` upserts a single markdown file into the index without
  requiring a git commit. Works on uncommitted, untracked, and modified files.
- **R2.** Does not update `last_ingested_commit`. Subsequent `lore ingest` (delta) still sees real
  changes from the last committed state.
- **R3.** Does not walk the repository and does not run `.loreignore` reconciliation.
- **R4.** Respects `.loreignore` by default; a `--force` override allows indexing an otherwise
  ignored file. Rationale: the mental model stays consistent with walk-based ingest, and the
  override exists for the author-is-iterating-on-a-draft case.
- **R5.** Rejects paths that are not inside the knowledge directory (canonicalized), and rejects
  non-markdown extensions (`.md`, `.markdown`). Clear error messages, exit code 1.
- **R6.** Acquires the write lock on the database before touching it, same as `lore ingest` and MCP
  write handlers.
- **R7.** Progress and status to **stderr**, per CLI output conventions. Exit 0 on success, 1 on
  runtime failure, 2 on usage error (missing flag / invalid arguments).
- **R8.** Mutually exclusive with `--force`: `lore ingest --force --file x.md` is a usage error.
  `--force` in combination with `--file` is reserved for the `.loreignore` override (see R4); see
  Open Questions for the final flag name.

## Scope Boundaries

- **Not** repeatable: single path per invocation. Multiple-file support can come later if demand
  exists.
- **Not** a write path for creating new files from arbitrary content — that is what `add_pattern` /
  `update_pattern` MCP tools do. `--file` only re-indexes something already on disk.
- **Not** exposed as an MCP tool. The MCP write tools already produce fresh-from-disk indexing as a
  side effect.
- **Not** updating the `IngestMode::Delta { unchanged }` accounting. Single-file ingest gets its own
  mode variant.
- **Not** reconciling `.loreignore`. If the author wants reconciliation they can run `lore ingest`
  normally.

## Context & Research

### Relevant Code and Patterns

- `src/ingest.rs:900` — `fn index_single_file(...)`. Already encapsulates the read → delete old
  chunks → chunk → embed → insert sequence. **Single-file ingest will call this directly.** It
  already returns `(chunks, embedding_failures)`.
- `src/ingest.rs:50-75` — `IngestMode` and `IngestResult`. Add a new `SingleFile` mode variant.
- `src/ingest.rs:939` — `fn validate_within_dir`. Path traversal guard used by `add_pattern`. Reuse
  directly for the `--file` path argument.
- `src/loreignore.rs` — `load(knowledge_dir)` returns the matcher and hash atomically;
  `is_ignored(matcher, rel, is_dir)` evaluates a path. No new parser work needed.
- `src/main.rs:57-61, 119, 256-345` — `Commands::Ingest { force }`, dispatch, and `cmd_ingest`.
  Extend the CLI enum and dispatch to a new code path when `--file` is present.
- `src/lockfile.rs` — `WriteLock::open` + `WriteLock::acquire` + `lock_path_for`. Same write lock
  used by `cmd_ingest`.
- `tests/loreignore.rs` — reference for integration test shape (tempdir + git init + `memory_db()` +
  `FakeEmbedder`).
- `docs/pattern-authoring-guide.md:160-189` — Vocabulary Coverage Technique. Rewrite step 2 to use
  `lore ingest --file`.

### Institutional Learnings

- `docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`
  — captures the exact friction this plan removes. Update or supersede once shipped.
- `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`
  — reminds us reconciliation is a _walk-based_ concept. Single-file ingest deliberately opts out.
- `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md` — progress
  to stderr, per convention.

### External References

None needed. Local patterns are established and recent.

## Key Technical Decisions

- **Reuse `index_single_file`, not a new helper.** It already does exactly the delete-old +
  re-chunk + re-embed + insert sequence we need. Calling it keeps the chunking + embedding pipeline
  identical between walk-based and single-file paths — no behavioural drift.
- **New `IngestMode::SingleFile { path: String }` variant.** Keeps the result shape uniform so the
  existing error-reporting tail in `cmd_ingest` can be reused. The path is carried in the variant
  because the summary line needs it.
- **Do not touch `META_LAST_COMMIT` or `META_LOREIGNORE_HASH`.** These represent walk-based state;
  single-file ingest is orthogonal. Leaving them alone means the next `lore ingest` still sees real
  git changes and still runs reconciliation if `.loreignore` changed.
- **Respect `.loreignore` by default via `loreignore::load` + `is_ignored`.** Cheap check and keeps
  the mental model consistent. The override lives on a separate flag (see Open Questions) so agents
  do not accidentally bypass it.
- **Canonicalise the file path and reuse `validate_within_dir`.** Same guard used by `add_pattern`,
  so path-traversal protection is uniform across write paths. File must also exist and have a
  `.md`/`.markdown` extension.
- **Acquire the write lock before touching the database.** Same code path as `cmd_ingest`; MCP
  writers are already serialised against this lock.
- **Clap: `--file` and `--force` on the same `Ingest` subcommand.** `--force` without `--file` keeps
  current semantics (full walk). `--file` without `--force` is the new path. Combining `--force`
  with `--file` is a usage error unless `--force` carries the "override `.loreignore`" meaning — see
  Open Questions.

## Open Questions

### Resolved During Planning

- **Does this update `last_ingested_commit`?** No. Single-file ingest is orthogonal to delta state
  (R2).
- **Does this run reconciliation?** No. Reconciliation is a walk-based concept (R3).
- **Respect `.loreignore`?** Yes, by default, with an override flag (R4).
- **Multiple files per invocation?** No. Single-only for v1 (Scope Boundaries).
- **MCP exposure?** No. MCP write tools already re-index on write (Scope Boundaries).
- **Must the file be inside `knowledge_dir`?** Yes. Canonicalised, rejected otherwise (R5).
- **Non-markdown extensions?** Rejected with a clear error (R5).
- **Write lock?** Yes (R6).
- **Does it work in non-git repositories?** Yes — single-file ingest does not consult git at all, so
  it works identically regardless of whether the knowledge directory is a git repository.

### Deferred to Implementation

- **Exact name of the `.loreignore` override flag.** Two candidates:
  1. Reuse `--force` to mean "override `.loreignore` when combined with `--file`". Rejected leaning:
     overloading `--force` muddles two distinct meanings (full re-walk vs. override filter).
  2. New flag, e.g. `--force-include` or `--no-ignore`. Cleaner but adds surface area. Decide during
     Unit 2. The default should be: if `--file` is passed with `--force`, treat it as the override.
     If `--force` alone, it is the existing full-walk behaviour. This keeps flag surface minimal and
     matches user intuition that `--force` means "do it anyway."
- **Debug-log format for the single-file path.** Mirror existing `lore_debug!` style from
  `index_single_file` call sites.
- **Exact wording of progress/error messages.** Finalise during Unit 2; follow the style already
  used in `cmd_ingest`.

## Implementation Units

- [x] **Unit 1: Add `IngestMode::SingleFile` and `ingest_single_file` entry point**

**Goal:** Introduce the public ingest entry point that indexes one file, with full validation and
result reporting, and without touching any walk-based state.

**Requirements:** R1, R2, R3, R4 (matcher check), R5, R6 (callers acquire lock), R8

**Dependencies:** None.

**Files:**

- Modify: `src/ingest.rs` (add variant, add `pub fn ingest_single_file`)
- Test: `src/ingest.rs` unit tests (alongside existing `index_single_file_respects_strategy`)

**Approach:**

- Add `IngestMode::SingleFile { path: String }` to the existing enum. The path is the
  knowledge-dir-relative path, same format as `source_file` in the database.
- New signature:
  ```
  pub fn ingest_single_file(
      db: &KnowledgeDB,
      embedder: &dyn Embedder,
      knowledge_dir: &Path,
      file_path: &Path,
      strategy: &str,
      force_override_ignore: bool,
      on_progress: &dyn Fn(&str),
  ) -> IngestResult
  ```
- Body sequence:
  1. Canonicalise `file_path` and verify it exists as a file.
  2. Verify extension is `md` or `markdown`. Otherwise: `result.errors.push(...)`, return.
  3. Call `validate_within_dir(knowledge_dir, &canonical)`. On error, push to `errors`.
  4. Derive `rel_path` relative to `knowledge_dir` (same pattern as `index_single_file`).
  5. Unless `force_override_ignore`, run `loreignore::load(knowledge_dir)` and
     `is_ignored(matcher, rel_path, false)`. If ignored, push a specific error
     (`"{rel} is excluded by .loreignore; pass --force to index anyway"`) and return.
  6. Call `index_single_file(db, embedder, knowledge_dir, &canonical, strategy)`.
  7. Populate `IngestResult` with `mode: IngestMode::SingleFile { path: rel_path }`,
     `files_processed: 1`, `chunks_created: chunks`, everything else zero.
- Emit progress messages in the same style as `process_change`:
  - `"Single-file ingest: {rel}"`
  - `"  {rel} → {chunks} chunks"` on success
- Debug-log via `lore_debug!` mirroring existing call sites.

**Patterns to follow:**

- `index_single_file` at `src/ingest.rs:900` — reuse verbatim for the actual DB work.
- `add_pattern`'s path validation path for the canonicalise + `validate_within_dir` dance.
- `process_change` at `src/ingest.rs:450` for progress-message style.

**Test scenarios:**

- **Happy path:** `ingest_single_file` on a markdown file inside `knowledge_dir` upserts chunks —
  both on first ingest (no prior chunks) and when the file was already indexed (old chunks replaced,
  not duplicated). Verify `result.chunks_created > 0`, `result.files_processed == 1`, `result.mode`
  is `SingleFile { path }` with the expected relative path.
- **Happy path:** Works on a file that is **not** committed to git (the entire point). Use a non-git
  tempdir or an untracked file in a git tempdir. No git calls should be made.
- **Edge case:** File extension is `.markdown` — accepted. File extension is `.txt` — rejected with
  an error mentioning the extension.
- **Edge case:** `META_LAST_COMMIT` and `META_LOREIGNORE_HASH` are unchanged after a single-file
  ingest, even when present beforehand. (Set fake values before, verify unchanged after.)
- **Error path:** File does not exist → error.
- **Error path:** File is outside `knowledge_dir` (absolute path pointing to `/etc/passwd` or
  `../outside.md`) → error from `validate_within_dir`, nothing written.
- **Error path:** File is inside `knowledge_dir` but matched by `.loreignore` and
  `force_override_ignore` is `false` → error, nothing written, existing chunks for that file (if
  any) untouched.
- **Happy path:** Same as above but `force_override_ignore` is `true` → file is indexed.
- **Edge case:** File is a directory path, not a file → error.

**Verification:**

- `cargo test --lib ingest::tests::ingest_single_file_*` passes.
- `just ci` clean.

---

- [x] **Unit 2: Wire `--file` into the `Ingest` CLI subcommand**

**Goal:** Expose single-file ingest on the CLI with proper flag wiring, mutual-exclusion validation,
write-lock acquisition, and summary output to stderr.

**Requirements:** R1, R6, R7, R8, R4 (override flag semantics)

**Dependencies:** Unit 1.

**Files:**

- Modify: `src/main.rs` (extend `Commands::Ingest`, extend `cmd_ingest`)

**Approach:**

- Extend `Commands::Ingest` with `file: Option<PathBuf>`. Keep `force: bool`.
- In `cmd_ingest`, branch on `file`:
  - `Some(path)` → single-file path.
  - `None` → existing full/delta path (unchanged).
- Single-file branch:
  1. Load configuration, open DB, create Ollama client — same as today.
  2. Acquire the write lock via `WriteLock::open` + `acquire` (identical to existing path).
  3. Decide `force_override_ignore`: `force` flag means "override `.loreignore`" when `--file` is
     present. This is the resolution of the deferred open question — a single existing flag covers
     both overrides (full walk vs. filter-bypass) without adding surface area.
  4. Canonicalise the user-supplied path against CWD first (so the user can pass a relative path
     like `./patterns/foo.md` from any working directory). Do not require it to be relative to
     `knowledge_dir` at the CLI layer; Unit 1 handles containment.
  5. Call `ingest::ingest_single_file(...)`.
  6. Match on `result.mode`:
     ```
     IngestMode::SingleFile { path } => eprintln!(
         "\nDone (single-file): {path} → {chunks} chunks",
         chunks = result.chunks_created,
     ),
     ```
  7. Errors tail is shared with existing `cmd_ingest` (loop + `eprintln!`). Exit code 1 when
     `result.errors` is non-empty (match existing behaviour).
- CLI help text for `--file`:
  `Index a single markdown file without requiring a git commit. Respects .loreignore unless --force is also passed.`
- Debug log on entry: `lore_debug!("ingest: dir={} mode=single-file path={}", ..., path)`.

**Patterns to follow:**

- Existing `cmd_ingest` body at `src/main.rs:256-345` — same write-lock dance, same error tail.
- CLI convention: stderr for human messages, exit 1 on runtime failure (per
  `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md`).

**Test scenarios:**

- **Happy path (e2e):** In a tempdir knowledge base with an uncommitted markdown file, invoke the
  CLI (or `cmd_ingest` helper) with `--file`. Afterwards, `db.source_files()` contains the new file
  and `db.search_fts("term from body")` returns it. No git commit required.
- **Happy path:** Re-running `--file` on the same file twice does not duplicate chunks (Unit 1
  covers the helper; this is the CLI-level smoke check).
- **Error path:** `--file` pointing outside `knowledge_dir` prints the error to stderr and exits
  with code 1.
- **Error path:** `--file` pointing at a `.txt` file prints an extension error and exits 1.
- **Error path:** `--file` pointing at a `.loreignore`-matched file without `--force` prints the
  override hint and exits 1.
- **Happy path:** `--file` with `--force` on a `.loreignore`-matched file succeeds and indexes the
  file.
- **Invariant:** After `--file`, `db.get_metadata(META_LAST_COMMIT)` is unchanged. Verify by setting
  it before the call (via a helper or by running `lore ingest` first on a seeded repository).

**Verification:**

- `just ci` clean. Manual: `lore ingest --file patterns/test.md` in the checked-out `lore-patterns`
  repository upserts a chunk for an uncommitted edit.

---

- [x] **Unit 3: Integration tests for single-file ingest**

**Goal:** Provide end-to-end coverage that exercises the full single-file flow through
`ingest::ingest_single_file` in a realistic tempdir, mirroring the style of `tests/loreignore.rs`.

**Requirements:** R1–R8

**Dependencies:** Unit 1, Unit 2.

**Files:**

- Create: `tests/single_file_ingest.rs`

**Approach:**

- Reuse the helper pattern from `tests/loreignore.rs`: `memory_db()`, `write_md`, and a lightweight
  `git_init` for the test that verifies the no-commit behaviour explicitly.
- Use `FakeEmbedder` for determinism.
- One test file, ~8 tests, each focused on a single requirement.

**Patterns to follow:**

- `tests/loreignore.rs` — helper layout, tempdir + in-memory DB + `FakeEmbedder` pattern.
- `src/ingest.rs` unit tests (`index_single_file_respects_strategy`) — reference for the smallest
  sensible setup.

**Test scenarios:**

- **Happy path:** `ingest_single_file_indexes_uncommitted_file` — create a markdown file in a
  non-git tempdir, call the helper, assert `db.source_files()` contains it and `search_fts` returns
  it.
- **Happy path:** `ingest_single_file_in_git_repo_without_commit` — create a git repo with one
  committed file, create a second uncommitted file, ingest only the uncommitted one via `--file`,
  assert both files are now indexed (the first from a prior full ingest, the second from
  single-file).
- **Invariant:** `ingest_single_file_does_not_touch_last_ingested_commit` — run full ingest first to
  record a SHA, run `ingest_single_file`, assert `META_LAST_COMMIT` is unchanged.
- **Happy path:** `ingest_single_file_replaces_existing_chunks` — ingest file, modify file on disk,
  single-file ingest again, assert old chunks are gone and new chunks match new content.
- **Error path:** `ingest_single_file_rejects_path_outside_knowledge_dir` — create a file in a
  sibling tempdir, call with an absolute path, assert error and DB untouched.
- **Error path:** `ingest_single_file_rejects_non_markdown_extension` — create `foo.txt`, call,
  assert error.
- **Error path:** `ingest_single_file_respects_loreignore` — write `.loreignore` with `draft.md`,
  call with `force_override_ignore=false`, assert error and file not indexed.
- **Happy path:** `ingest_single_file_force_overrides_loreignore` — same setup, call with
  `force_override_ignore=true`, assert file indexed.

**Verification:**

- `cargo test --test single_file_ingest` passes.
- `just ci` clean.

---

- [x] **Unit 4: Update pattern-authoring guide and close the learning**

**Goal:** Rewrite the Vocabulary Coverage Technique to use `lore ingest --file`, and supersede the
"delta ingest requires commit" learning.

**Requirements:** "Update the vocabulary coverage technique section in
`docs/pattern-authoring-guide.md` when shipped" (from ROADMAP).

**Dependencies:** Unit 2 (feature must actually work before we document it).

**Files:**

- Modify: `docs/pattern-authoring-guide.md` (section `## Vocabulary Coverage Technique` at lines
  160-189)
- Modify:
  `docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`
  (add a "Superseded by" note pointing at the feature, or mark resolved)
- Modify: `ROADMAP.md` (move single-file ingest from Up Next to Completed)
- Modify: `README.md` (mention `--file` in the CLI section if single-file is covered there)

**Approach:**

- New step 2 in the guide: `lore ingest --file patterns/my-new-pattern.md` — no commit required.
  Step 4's amend-and-re-ingest loop becomes simply "edit, `lore ingest --file`, search again."
- Leave the Pattern Review Checklist reference intact; it just gets a faster loop.
- In the learning doc, add a short "Resolution" or "Superseded" block pointing at
  `lore ingest --file` and the date. Do not delete the learning — the historical context is still
  useful for understanding _why_ the flag exists.
- ROADMAP move: delete from Up Next, add to Completed with date `2026-04-06` and a one-line note.

**Patterns to follow:**

- Existing Completed entries in `ROADMAP.md` for phrasing.
- Other superseded solution docs in `docs/solutions/` for the resolution-note style (if any exist;
  otherwise keep it simple).

**Test scenarios:**

- Test expectation: none — documentation-only unit. Verification is a manual re-read for accuracy
  and `dprint fmt` / `just ci` for markdown formatting.

**Verification:**

- `dprint fmt` clean. `just ci` clean (markdown checks).
- Manual: follow the updated Vocabulary Coverage Technique end-to-end against a real pattern in the
  checked-out `lore-patterns` repository and confirm the loop works without a commit.

## System-Wide Impact

- **Interaction graph:** Adds a fourth write path (after `lore ingest`, MCP `add_pattern`, MCP
  `update_pattern`/`append_to_pattern`). All four share the write lock, so contention is handled.
- **Error propagation:** `ingest_single_file` returns `IngestResult` with `errors: Vec<String>`,
  matching the existing shape. `cmd_ingest`'s existing error-loop tail handles it.
- **State lifecycle risks:** The deliberate _non_-update of `META_LAST_COMMIT` is the critical
  invariant. If single-file ingest ever wrote that, the next delta ingest would silently skip real
  changes. Test this explicitly.
- **API surface parity:** No new MCP tool. No new hook contract. CLI-only surface.
- **Integration coverage:** Unit 3 integration tests exercise the CLI → helper → DB path end-to-end
  with `FakeEmbedder`.
- **Unchanged invariants:**
  - `META_LAST_COMMIT` is only written by walk-based ingest.
  - `META_LOREIGNORE_HASH` is only written by reconciliation.
  - `index_single_file`'s contract is unchanged; it is still the single place that does chunk +
    embed + insert for one file.
  - Existing `lore ingest` (no flags) and `lore ingest --force` semantics are unchanged.

## Risks & Dependencies

| Risk                                                                                                                                                                        | Mitigation                                                                                                                                                                               |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--force` semantics become overloaded (full walk vs. filter override).                                                                                                      | Document in `--help` text and the guide. The combinations are orthogonal: `--force` alone = full walk, `--force --file` = override `.loreignore` for that file. Unit 2 test covers both. |
| Author runs `--file` on a file with a different path case (e.g. `README.md` vs `readme.md`) on a case-insensitive filesystem and ends up with duplicate `source_file` rows. | `index_single_file` uses `strip_prefix` on canonicalised paths, which normalises case on case-insensitive filesystems. Same behaviour as existing walk-based ingest — no regression.     |
| Author forgets `--force` and is confused why `.loreignore`-excluded files are not indexed.                                                                                  | Error message explicitly mentions `--force` as the override.                                                                                                                             |
| User passes a path outside `knowledge_dir` hoping to ingest patterns from another repository.                                                                               | Rejected with a clear error. Clarifies the mental model that `knowledge_dir` is the root.                                                                                                |
| Concurrent `lore ingest` and `lore ingest --file` from two shells.                                                                                                          | Write lock serialises them. Existing `lockfile.rs` tests cover the contention path.                                                                                                      |

## Documentation / Operational Notes

- Update `docs/pattern-authoring-guide.md:160-189` (Unit 4).
- Supersede
  `docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`
  (Unit 4).
- Update `ROADMAP.md`: move entry to Completed (Unit 4).
- Mention in `README.md` CLI section if single-file is listed there (Unit 4).
- Unblocks: **Pattern QA skill** (next Up Next item) — can now run the vocabulary coverage loop
  without commits.

## Completion Notes (2026-04-06)

Shipped as **attila/lore#31** in four commits. All four implementation units landed as planned; the
deviations and additions below were driven by ce-review feedback and the user's request for
robustness coverage ahead of the dogfooding cycle.

### Resolved deferred questions

- **`.loreignore` override flag.** Resolved as "reuse `--force`" per the plan's leaning. The
  rationale (minimal flag surface, intuitive "do it anyway" semantics) survived ce-review, with
  three reviewers nonetheless flagging the semantic overload as a P2 worth addressing later. The
  cleaner split into `--force` + `--override-ignore` is captured as a follow-up todo
  (`docs/todos/ingest-force-flag-semantic-overload.md`) rather than landed in this PR.

### What ce-review changed before merge

ce-review surfaced findings that were applied in the same PR as `safe_auto` fixes:

- **Empty-path "Done" line on error returns.** Three reviewers (correctness, adversarial,
  maintainability) independently flagged that `IngestMode::SingleFile.path` was initialised with
  `String::new()` and only overwritten on the success path, producing a contradictory
  `Done (single-file): → 0 chunks` line on every early-return error. Fixed by seeding the path
  upfront from `file_path.to_string_lossy()` and gating the "Done" summary on `errors.is_empty()`.
  Pinned by a regression test (`done_line_is_suppressed_when_single_file_ingest_fails`).
- **Composition cascade — adversarial review finding.** A specific user scenario the plan did not
  cover: `git rm`ing a file, recreating it in the working tree, single-file-ingesting it, then
  running `lore ingest`. The walk-based delta pass sees the file as `Deleted` in
  `git diff --name-status` against the prior `last_ingested_commit` and silently wipes the chunks
  single-file just upserted. Neither component is buggy in isolation; the failure lives in their
  interaction. Captured as an "Interaction hazard" block in `docs/pattern-authoring-guide.md` with
  the safe workflow prescription, and pinned by a hazard-test
  (`subsequent_delta_ingest_wipes_single_file_upsert_of_git_deleted_file`) so any future refactor
  that changes delta-wipe behaviour notices.
- **CWD context on `Cannot access` errors** — added a `(cwd: <path>)` hint so an agent passing a
  relative path from an unrelated working directory can self-diagnose.
- **Duplicate "Single-file ingest:" banner** — was being emitted twice (once by `dispatch_ingest`,
  once by the `on_progress` callback inside `ingest_single_file`). Removed the dispatch-side one.
- **`after_help` block on the `Ingest` subcommand** — added EXAMPLES (all four invocation shapes),
  EXIT CODES, and NOTES so `lore ingest --help` self-documents the matrix.

### Test additions beyond the plan

Plan Unit 3 specified library-level integration tests in `tests/single_file_ingest.rs`. Three
additional layers landed during execution:

- **CLI binary error-path tests** in `tests/smoke.rs` (4 tests). Exercise the `lore ingest --file`
  binary directly via `assert_cmd::cargo_bin` for the exit-1 paths (extension rejection, path
  outside `knowledge_dir`, missing file with CWD hint, `--help` content). Added in response to the
  testing reviewer flagging that the entire dispatch chain had no end-to-end test.
- **Symlink-escape regression test** (`#[cfg(unix)]`) — pins the property that symlinks inside
  `knowledge_dir` whose canonical target lies outside are rejected by `validate_within_dir`. Plan
  did not list this scenario; testing reviewer flagged the gap.
- **Directory-path rejection test** — plan listed it in the test scenarios for Unit 1 but the
  implementation did not include it. Added during ce-review.
- **Real-Ollama integration tests** in `tests/ollama_integration.rs` (2 tests, `#[ignore]`-gated):
  one library-level (`ingest_single_file_happy_path_with_real_embeddings`) proving the embedding
  path works against production `nomic-embed-text`, and one binary-level
  (`ingest_file_happy_path_via_binary`) proving the full clap → cmd_ingest → write-lock → dispatch →
  embed → summary → exit-0 chain wires correctly. Both verified locally against a running Ollama
  before push. Added at the user's request to maximise confidence ahead of the dogfooding cycle that
  this feature enables.

### Final test totals at merge

| Layer                                       | Test count                    |
| ------------------------------------------- | ----------------------------- |
| Library unit (`src/ingest.rs::tests`)       | 9 single-file tests added     |
| Library integration (FakeEmbedder)          | 12 in `single_file_ingest.rs` |
| CLI binary (assert_cmd, default suite)      | 4 in `smoke.rs`               |
| Library + binary (real Ollama, `#[ignore]`) | 2 in `ollama_integration.rs`  |

Default `just ci` is Ollama-free and runs in seconds; the real-Ollama tests opt in via
`cargo test -- --ignored`. All gates green (clippy, dprint, cargo-deny, cargo-doc).

### Residual follow-ups (captured as todos)

Six deferred items from ce-review live under `docs/todos/`:

- `ingest-force-flag-semantic-overload.md` (P2)
- `ingest-json-output-and-error-codes.md` (P2)
- `mcp-reindex-file-tool.md` (P2)
- `single-file-ingest-embedding-failure-rollback.md` (P2)
- `tests-common-helpers-extraction.md` (P3)
- `validate-within-dir-double-canonicalisation.md` (P3)

None block the merge or the dogfooding cycle.

## Sources & References

- ROADMAP.md: Up Next → "Single-file ingest"
- `docs/pattern-authoring-guide.md:160` — Vocabulary Coverage Technique
- `docs/solutions/best-practices/delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`
- `src/ingest.rs:900` — `index_single_file`
- `src/ingest.rs:939` — `validate_within_dir`
- `src/loreignore.rs` — `load`, `is_ignored`
- `src/lockfile.rs` — `WriteLock`, `lock_path_for`
- `src/main.rs:256-345` — existing `cmd_ingest`
- `tests/loreignore.rs` — integration test pattern
