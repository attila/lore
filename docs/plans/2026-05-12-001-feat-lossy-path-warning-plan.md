---
title: "feat: Lossy-path warning during directory walk (Slice D)"
type: feat
status: completed
created: 2026-05-12
origin: docs/brainstorms/2026-04-08-edge-case-handling-requirements.md
---

# feat: Lossy-path warning during directory walk (Slice D)

Slice D of the edge-case-handling brainstorm. Lossy non-UTF-8 paths encountered during
`walk_md_files` are currently silently routed into the keeper list via `to_string_lossy()`, indexing
under a substituted-byte path. This plan converts the rare case from invisible data loss into an
auditable warning on `IngestResult::errors`, while leaving the file unindexed.

Slice D is the last unshipped slice from the edge-case-handling brainstorm (A/B/C/E all merged).
Closing it completes the roadmap line.

---

## Summary

`Path::to_string_lossy()` returns a `Cow<str>`: `Cow::Borrowed` when the underlying `OsStr` was
already valid UTF-8, `Cow::Owned` when bytes had to be substituted (U+FFFD). The keeper-list
construction at `src/ingest.rs:652-668` currently consumes the lossy string unconditionally and
treats it as a clean relative path, indexing the file under a substituted-byte path — silently
corrupting the index.

R8 inverts this: when `to_string_lossy()` lossy-converted, skip the file from the keeper list and
surface the path on a warnings channel that `full_ingest` folds into `IngestResult::errors`. R11.9
pins the behaviour with a `#[cfg(unix)]`-gated test that plants a non-UTF-8 filename via
`OsStr::from_bytes`.

Tier-2 per the CLI behaviour ladder (warn but proceed; exit 0). Same emission channel as the
existing per-file errors `full_ingest` already collects (`result.errors.push(...)`).

---

## Problem Frame

`walk_md_files` (`src/ingest.rs:634`) constructs the relative-path string for each candidate via:

```rust
let rel = path.strip_prefix(knowledge_dir).ok()?.to_string_lossy().to_string();
```

`to_string_lossy()` is a defensible default for ordinary display paths, but in this position the
relative string becomes the indexed `source_file` key in the database. A non-UTF-8 filename produces
a `Cow::Owned` string with U+FFFD substitutions; the file is then indexed under that substituted key
and is unfindable from the original filename. The user gets no signal.

The brainstorm explicitly classifies this as ingest-error severity rather than panic-or-silent-pass
(see `_Key Decisions_` under "Lossy path conversion is an ingest error"). The existing
`IngestResult::errors` channel matches that severity: per-file batch errors that don't abort the
run.

---

## Requirements Trace

| Origin ID | Requirement                                                               | Where addressed |
| --------- | ------------------------------------------------------------------------- | --------------- |
| R8        | Detect lossy `to_string_lossy()` via `Cow::Owned`, push to errors, skip   | U1, U2          |
| R11.9     | `#[cfg(unix)]` regression test: non-UTF-8 filename → warning, not indexed | U2              |

R8 spans two units only because the detection (U1) and the wiring into `IngestResult::errors` (U2)
cross the `walk_md_files` / `full_ingest` seam. The signature change in U1 has no behaviour-visible
effect on its own; U2 turns the warnings into the user-visible contract R8 specifies.

Other slices and out-of-scope requirements (R1–R7, R9–R11.1, R11.4) all shipped previously; this
plan does not touch them.

---

## Scope Boundaries

### In scope

- Detection inside `walk_md_files` (`src/ingest.rs`).
- Propagation through `discover_md_files` into `full_ingest`'s `IngestResult::errors`.
- Inline regression test in `src/ingest.rs::tests`, `#[cfg(unix)]`-gated.
- `CHANGELOG.md` `[Unreleased]/Added` entry.
- `ROADMAP.md` — flip the edge-case-handling line to Completed.

### Out of scope

- Delta-ingest paths. `delta_ingest` consumes `git::FileChange` entries produced by
  `git diff --name-status`; those paths come from git as already-validated strings rather than from
  a filesystem walk, so the R8 detection point does not apply on the delta path. The brainstorm
  scopes R8 specifically to the directory walk.
- Single-file ingest (`lore ingest --file <path>`). The path comes from a CLI argument already
  constrained to `String`; non-UTF-8 input fails at argument-parse time, not in our code.
- macOS/Windows-specific test variants. APFS/HFS+ enforce UTF-8 at the filesystem layer (per the
  brainstorm's "Linux-dominant in practice" note); `OsStr::from_bytes` is Unix-only, so the test is
  `#[cfg(unix)]`-gated. Windows builds compile but do not exercise the assertion. No
  Windows-specific path is added.
- Any new CLI flag, structured error type, or programmatic API for the warning. The brainstorm
  explicitly leaves structured agent-facing error types as deferred follow-up; the warning is a
  `String` on `IngestResult::errors`, same shape as every other per-file batch error.

### Deferred to Follow-Up Work

None.

---

## Key Technical Decisions

### Signature: return type carries warnings (option 2 from the brainstorm)

The brainstorm flagged two signature options for propagating warnings out of `walk_md_files`:

1. `&mut Vec<String>` accumulator parameter.
2. Change the return type to include warnings alongside paths.

Option 2 is picked. `walk_md_files` already returns `(Vec<(String, PathBuf)>, usize)`, so the change
is mechanical: add a third tuple element `Vec<String>` of lossy-path warnings. The function has
three call sites today:

- `discover_md_files` (`src/ingest.rs:680`) — feeds `full_ingest`; needs the warnings.
- `effective_scan_state` (`src/ingest.rs:748`) — feeds `handle_lore_status` and the empty-scan-set
  warning; needs the warning count (to correctly classify the empty state — see Shadow-path:
  `effective_scan_state` misclassification below).
- `reconcile_ignored` (`src/ingest.rs:458`) — the `.loreignore` reconciliation pass during delta
  ingest; needs the warnings (a lossy file encountered during reconciliation should surface on
  `IngestResult::errors` for the same reason it does in `full_ingest`, or the delta path silently
  swallows the diagnostic).

Rationale:

- The helper is already a tuple-returning pure function; option 1 would add mutable state for a
  strictly-additive output, which is the less Rusty choice.
- All three callers need either the warnings themselves or the count, so the in-tuple return is
  strictly cheaper than threading an accumulator into three separate places.
- Future callers that need warnings get them without a follow-up signature change.

### Closure rewrite: `for` loop over `filter_map`

`walk_md_files`'s current keeper-construction is a `filter_map` over the walked paths
(`src/ingest.rs:652-668`). After the change the closure must produce one of three outcomes per path
(kept / ignore-skipped / lossy-warned), which fits a `for` loop with two named accumulators more
naturally than a `filter_map` over a partition enum. `RefCell` is not needed and `partition_map`
works but reads worse than a `for` loop for a three-way fork with a `lore_debug!` side effect on the
ignore branch.

Pick the `for` loop. The fold and `partition_map` alternatives are viable but should not be the
implementer's first reach.

### Shadow-path: `discover_md_files` progress message

`discover_md_files` currently emits `Found {kept} markdown files ({N}
excluded by .loreignore)`
derived from `walked_count - kept.len()`. After this change, lossy paths drop out of `kept` but stay
counted in `walked_count`, so the delta would silently include lossy files and misattribute them to
`.loreignore` — a user with one lossy file and zero ignore rules would see
`Found 0 markdown files (1 excluded by
.loreignore)`, which is factually wrong and contradicts the
lossy warning emitted elsewhere on stderr.

Fix in this slice: compute `ignored = walked_count - kept.len() - lossy_warnings.len()` and only
emit the `(N excluded by .loreignore)` suffix when `ignored > 0`. The lossy paths are surfaced via
`IngestResult::errors`; they don't need a duplicate accounting on the progress line.

### Shadow-path: `effective_scan_state` misclassification

`effective_scan_state` classifies a directory as `AllIgnored` when `kept.is_empty()` and
`walked_count > 0` — which after this change would include the "all files have non-UTF-8 names"
case. The user would then see
`Warning: .loreignore matched every markdown file; nothing will be
indexed` (via
`empty_warning_message` at `src/ingest.rs:807-808`), naming the wrong cause and pointing at the
wrong recovery action.

Fix in this slice: subtract the lossy count from `walked_count` before deriving the state, i.e.
`let effective_walked = walked_count - lossy_warnings.len();` then discriminate `Populated` /
`FilesystemEmpty` / `AllIgnored` against `effective_walked` and `kept.len()` instead of the raw
walk. An all-lossy directory routes to `FilesystemEmpty`, which emits
`Warning: knowledge
directory is empty — add at least one .md file under <dir>`. The wording isn't
perfect for the all-lossy case (the directory isn't empty in the literal sense), but it's strictly
better than blaming `.loreignore`, and the user already has the per-file lossy warning on stderr
naming the real cause. Introducing a fourth `EffectiveScanState::AllLossy` variant to get bespoke
wording is out of scope; the lossy warnings on `IngestResult::errors` carry the diagnostic.

### Detection: pattern-match on the `Cow` discriminant, not byte equality

The detection check uses `matches!(cow, Cow::Owned(_))`. The brainstorm spells out the contract:
`to_string_lossy()` returns `Cow::Borrowed` when the underlying `OsStr` is already valid UTF-8 and
pays no allocation; `Cow::Owned` is the unambiguous "data was lost" signal. Pattern-matching the
discriminant is cheaper than re-validating bytes and is the documented Rust idiom for this exact
question.

Files with valid UTF-8 paths take the `Cow::Borrowed` path and bear no runtime cost beyond what they
pay today.

### Skip-not-index policy

A lossy path is **not** added to the keeper list. It surfaces as a warning string only. Indexing a
substituted-byte path key would be the exact failure mode R8 exists to prevent — a half-corrupted
index that hides the underlying problem. The user-visible recovery action is "rename the file to a
UTF-8 name and re-ingest"; that recovery is meaningless if the corrupted version is silently indexed
first.

### Warning message: name the path lossily but flag the substitution

The warning message is constructed as:

```
Skipped {relative path with U+FFFD substitutions}: filename is not valid UTF-8 (file not indexed)
```

The lossy form is the only printable representation we have; including it lets the user grep the
disk for similar filenames. The trailing clause names the consequence so the warning is
self-contained. This matches the existing `IngestResult::errors` voice ("Failed to index {}: {e}",
"Failed to delete {path}: {e}").

### Tests live alongside the existing `src/ingest.rs::tests`, not in a new `tests/edge_cases.rs`

The brainstorm flagged this as a deferred-to-planning question. Resolved in favour of inline tests
in `src/ingest.rs::tests`:

- All other shipped edge-case slices (A, B, C, E) placed their regression tests inline alongside the
  surface they covered. Consistency wins unless there's a specific reason to break the pattern.
- The R11.9 test needs `walk_md_files` visibility (currently `pub(crate)` / module-private). Inline
  placement avoids exposing more than necessary; an integration test would force a wider visibility
  change.
- Test runtime is identical either way.

---

## Implementation Units

### U1. Detect lossy paths in `walk_md_files`; extend return type

**Goal:** Convert `walk_md_files` from silently lossy-converting non-UTF-8 paths into the keeper
list to skipping them and surfacing a warning string.

**Requirements:** R8 (detection half).

**Dependencies:** none.

**Files:**

- Modify: `src/ingest.rs` — `walk_md_files` (signature + body), `discover_md_files` (destructure +
  return-type update + progress message fix), `effective_scan_state` (destructure + state-derivation
  fix), `reconcile_ignored` (destructure + warning propagation).

**Approach:**

- Change `walk_md_files`'s return type from `(Vec<(String, PathBuf)>, usize)` to
  `(Vec<(String, PathBuf)>, usize, Vec<String>)`.
- Rewrite the keeper-construction body from `filter_map` to a `for` loop over the walked paths (see
  Key Technical Decisions → Closure rewrite). The loop maintains two named accumulators (`kept` and
  `lossy_warnings`) and writes to either `kept`, `lossy_warnings`, or neither (the loreignore-skip
  branch) per path. Per-path logic:
  - Compute `rel_cow = path.strip_prefix(knowledge_dir).ok()?.to_string_lossy()`.
  - `Cow::Owned(_)` → push
    `format!("Skipped {}: filename is not valid UTF-8 (file not indexed)", rel_cow)` onto
    `lossy_warnings`. Do **not** add to `kept`.
  - `Cow::Borrowed(_)` → if matched by the `.loreignore` matcher, `lore_debug!` and continue
    (existing behaviour). Otherwise push `(rel.into_owned(), path)` onto `kept`.
  - `Cow::Borrowed(_).into_owned()` allocates a `String` from the `&str`; this matches the cost the
    current `.to_string()` already pays for valid-UTF-8 paths.
- Sort `kept` (preserve the existing `kept.sort_by` call); return
  `(kept, walked_count, lossy_warnings)`.
- Update **all three** call sites:
  - `discover_md_files` (`src/ingest.rs:680`):
    `let (kept, walked_count, lossy_warnings) = walk_md_files(...)`. Change return type to
    `(Vec<PathBuf>, Vec<String>)`. Compute
    `ignored = walked_count - kept.len() - lossy_warnings.len()` and fix the progress message
    accounting (see Shadow-path: `discover_md_files` progress message above).
  - `effective_scan_state` (`src/ingest.rs:748`):
    `let (kept, walked_count, lossy_warnings) = walk_md_files(...)`. Derive state from
    `let effective_walked = walked_count - lossy_warnings.len();` (see Shadow-path:
    `effective_scan_state` misclassification above). Lossy warnings themselves are discarded here —
    `effective_scan_state` is a synchronous scan-set probe with no caller plumbing for warnings; the
    production lossy emission still fires from `discover_md_files` → `full_ingest`.
  - `reconcile_ignored` (`src/ingest.rs:458`):
    `let (disk_files, _, lossy_warnings) = walk_md_files(...)`. Thread `lossy_warnings` back through
    `reconcile_ignored`'s return value (currently `Result<ReconcileStats>`) so the caller in
    `ingest()` can fold them onto `result.errors` in the same way `full_ingest` does. The minimal
    carrier is a new field on `ReconcileStats`: `pub lossy_warnings: Vec<String>`. Mirrors the U2
    pattern in `full_ingest`.

**Patterns to follow:**

- `loreignore::is_ignored` already returns a `bool` consumed inside the same `filter_map`. The lossy
  check sits at a similar layer: a per-file decision that either keeps or drops the entry.
- `RefCell<Vec<String>>` collector pattern was introduced in slice C (`src/ingest.rs::tests`
  `ingest_emits_no_head_progress_line_on_fresh_git_init`) for analogous "capture progress strings
  inside a closure" needs. If the iterator-chain form fights the borrow checker, expand to a `for`
  loop with a plain `Vec<String>` rather than a `RefCell`.

**Test scenarios:**

This unit changes signatures across four functions. The user-visible behaviour pins in U2 (which has
the full suite of shadow-path tests). U1 itself relies on U2's coverage rather than adding redundant
inline walk-level tests — the U2 tests exercise the full chain through `full_ingest`,
`discover_md_files`, `effective_scan_state`, and `reconcile_ignored`, so any signature regression in
U1 surfaces there first. If implementation reveals a gap that the U2 tests don't cover (e.g., an
internal invariant of `walk_md_files` itself), add a focused inline test at that time.

**Verification:** `cargo check --features test-support` compiles after all four signature changes;
existing tests that touched the previous two-tuple shape (search
`let (kept, walked_count) = walk_md_files`) have been updated to the three-tuple form; no test that
previously passed against `walk_md_files` regresses.

### U2. Wire warnings into `full_ingest`'s `IngestResult::errors`; add R11.9 regression test

**Goal:** Surface the lossy-path warnings from `walk_md_files` on the existing per-file error
channel so `lore ingest` users see them on stderr without changing exit status.

**Requirements:** R8 (wiring half), R11.9.

**Dependencies:** U1.

**Files:**

- Modify: `src/ingest.rs` — `full_ingest`, plus test module additions.
- Test: `src/ingest.rs::tests` — new `#[cfg(unix)]`-gated test.

**Approach:**

- In `full_ingest`, destructure the new return shape:
  `let (md_files, lossy_warnings) = discover_md_files(...)`. After the call, fold the warnings onto
  `result.errors`:
  ```text
  for warning in lossy_warnings {
      result.errors.push(warning);
  }
  ```
- Mirror the same fold in `ingest()` for the reconcile path: when `reconcile_ignored` returns its
  `ReconcileStats { lossy_warnings, .. }`, drain `lossy_warnings` onto `result.errors` before
  continuing the delta pass. Keeps the diagnostic symmetric across full and delta entry points;
  without it, a lossy filename appearing during `.loreignore` reconciliation would be silently
  swallowed by the delta path even though the same file would warn on full ingest.
- Channel choice: lossy warnings go on `result.errors` per R8's "warning rather than silently
  indexing" framing. They share the channel with `Failed to index …` entries — same severity, same
  downstream rendering. The brainstorm specifically pins this on R8 ("push the file onto
  `IngestResult::errors` with a warning rather than silently indexing a corrupted path").
- HEAD-record gate interaction (intentional, documented): `full_ingest` at `src/ingest.rs:872`
  suppresses `META_LAST_COMMIT` recording when `result.errors` is non-empty. Lossy paths land on
  `result.errors`, so a knowledge dir with even one lossy filename will not advance HEAD —
  subsequent `lore ingest` runs fall through to full ingest again until the user renames the file.
  This is the conservative default consistent with existing per-file error handling. The user sees
  the lossy warning loudly on every run, so the recovery signal is unmistakable. The CHANGELOG entry
  calls the trade-off out explicitly so users understand `lore ingest --force`-equivalent
  performance persists until the bad filename is fixed.
- No change to `delta_ingest` itself or `single_file_ingest`. The delta path's main file enumeration
  uses `git diff --name-status` output, not the walker; only the reconciliation pass goes through
  `walk_md_files`.

**Patterns to follow:**

- `IngestResult::errors.push(format!("Failed to index {}: {e}", …))` already in `full_ingest` at
  `src/ingest.rs:866`. Same emission shape.

**Test scenarios:**

`#[cfg(unix)]`-gated regression tests covering R11.9, plus shadow-path coverage for the Key
Technical Decisions above:

- `full_ingest_warns_and_skips_non_utf8_filename` (R11.9 happy path, `#[cfg(unix)]`). Arrange: build
  a tempdir containing one valid-UTF-8 markdown file (`good.md`, valid body) and one non-UTF-8
  markdown filename constructed via `OsStr::from_bytes(&[0xFF, b'.', b'm',
  b'd'])`-style bytes
  (the exact bad-byte sequence is implementation- time; pick one that is unambiguously non-UTF-8 and
  ends in `.md`). Act: run `full_ingest` against the dir with an in-memory DB + fake embedder.
  Assert:
  - `result.errors.len() == 1`.
  - The single error string contains the substring "not valid UTF-8" and the substring "file not
    indexed".
  - `result.chunks_created` reflects only `good.md`'s chunks.
  - `result.files_processed == 1`.
  - The on-disk database has exactly one source row.

- `full_ingest_with_only_non_utf8_files_routes_to_empty_warning` (shadow-path for
  `effective_scan_state` misclassification, `#[cfg(unix)]`). Arrange: tempdir with a single
  non-UTF-8 `.md` filename and no valid-UTF-8 markdown files. Act: call
  `empty_warning_message(dir)`. Assert: returns a message containing "knowledge directory is empty"
  (i.e. `FilesystemEmpty` routing) — **not** the `.loreignore` wording. This guards against the
  misclassification that would surface ".loreignore matched every markdown file" for an all-lossy
  directory.

- `discover_md_files_progress_line_does_not_misattribute_lossy_to_loreignore` (shadow-path for
  `discover_md_files` accounting, `#[cfg(unix)]`). Arrange: tempdir with one valid `.md` file, one
  non-UTF-8 `.md` file, no `.loreignore`. Act: call `discover_md_files` with a stub `on_progress`
  that collects emitted strings. Assert: the `Found N
  markdown files` progress string does **not**
  contain "excluded by .loreignore" — there are zero loreignore-skipped files.

- `delta_reconcile_surfaces_lossy_warning_from_walk` (shadow-path for the reconcile path,
  `#[cfg(unix)]`). Arrange: tempdir with one valid-UTF-8 markdown file, one non-UTF-8 markdown
  filename, and a `.loreignore` that changes between two ingests (to force the reconciliation pass
  on the second run). Act: run `ingest()` twice — first to seed state, then modify `.loreignore` and
  run again. Assert: the second run's `result.errors` contains the lossy-path warning for the
  non-UTF-8 file (proof that reconcile-path warnings propagate, not just full-ingest warnings).

- `full_ingest_passes_valid_utf8_unicode_paths_unchanged` (negative control, not Unix-gated).
  Arrange: tempdir with one markdown file whose name is valid UTF-8 but non-ASCII (e.g., `café.md`
  in NFC). Act: run `full_ingest`. Assert: `result.errors` is empty, the file is indexed normally.
  This guards against a future regression where the `Cow::Owned` check is mistakenly tightened to
  `Cow::Borrowed` only on ASCII.

  Why a negative control: `to_string_lossy()` returns `Cow::Borrowed` for any valid-UTF-8 path
  including non-ASCII. The test prevents an implementer from mistakenly checking byte-only ASCII.

**Verification:**

- The two new tests pass; the four pre-existing sandbox-only test failures noted in
  `project_sandbox_grants.md` remain the only failures.
- `just ci` is green (fmt + clippy + test + deny + doc).
- Running `lore ingest` manually against a tempdir with a planted non-UTF-8 filename surfaces the
  warning on stderr and exits 0 (smoke-test-time confirmation, optional).

### U3. Documentation: CHANGELOG + ROADMAP

**Goal:** Record the user-visible change (Slice D shipping closes the edge-case-handling roadmap
line entirely).

**Requirements:** none.

**Dependencies:** U2.

**Files:**

- Modify: `CHANGELOG.md` — add `[Unreleased]/Added` bullet.
- Modify: `ROADMAP.md` — move the edge-case-handling line from `Up Next` to `Completed`; reference
  the plan path.
- Modify: this plan's frontmatter — flip `status: active` to `status: completed`.

**Approach:**

`CHANGELOG` entry under `[Unreleased]/Added`, voiced per project convention
(`feedback_changelog_user_facing_only.md`):

```text
- **Lossy-path warning during `lore ingest`** — files whose on-disk
  filename is not valid UTF-8 now surface a warning on stderr naming
  the file (in its U+FFFD-substituted printable form) and are skipped
  from indexing instead of being silently indexed under a substituted
  path. Tier-2 per the CLI behaviour ladder; no exit-status change.
  Side effect: a lossy filename lands on `IngestResult::errors` and
  therefore blocks HEAD recording on that run, so `lore ingest` stays
  in full-ingest mode until the file is renamed to a UTF-8 name. The
  warning fires loudly on every run while the bad name persists, so
  the recovery signal is unmistakable. Linux-dominant in practice —
  APFS and HFS+ enforce UTF-8 at the filesystem layer, so this rarely
  fires on macOS. Slice D of the edge-case-handling brainstorm; closes
  that roadmap line.
```

`ROADMAP` edit: drop the edge-case-handling entry from `Up Next` (it currently reads "one remaining
slice: lossy-path warning"), add a `Completed` entry naming the plan file path.

**Patterns to follow:**

- Slice C's CHANGELOG bullet (`[Unreleased]/Changed` for no-HEAD wording) is the closest precedent
  for voice and length.

**Test scenarios:** none — pure docs.

**Verification:**

- `dprint check` is clean on both files (markdown formatting).
- ROADMAP no longer lists edge-case-handling under `Up Next`.
- Plan frontmatter shows `status: completed` at commit time.

---

## Risks

- **Closure-rewrite ergonomics.** The current `filter_map` shape doesn't cleanly express a three-way
  partition (kept / ignored / lossy). U1's Approach pins the `for` loop as the chosen shape;
  `partition_map` and `fold` are viable alternates that read worse for this case. The risk is an
  implementer reaching for `RefCell` to keep the iterator chain; the Approach explicitly forecloses
  that.
- **Non-UTF-8 filename construction inside the test.** `OsStr::from_bytes` is the standard Unix
  path; building a `Path` that can then be created via `std::fs::File::create` is straightforward
  but worth verifying early. If `File::create` on the synthesized path fails on the host filesystem
  (e.g., due to a stricter mount option), gate the test `ignore`-style with a runtime probe rather
  than landing a flaky assertion.
- **HEAD-record interaction — performance penalty until rename.** `full_ingest` declines to record
  `META_LAST_COMMIT` when `result.errors` is non-empty. After this change, a lossy filename blocks
  HEAD recording on that run AND on every subsequent run, so delta mode is unavailable until the
  user renames the file. On a large knowledge dir this means each `lore ingest` invocation pays the
  full-walk cost (seconds-to-minutes) instead of the delta cost (sub-second). The lossy warning
  fires on every run so the diagnostic stays loud, but the performance penalty is real. Intentional
  behaviour for this slice: silently advancing HEAD past a known-broken file would defeat the
  diagnostic the slice is introducing. The CHANGELOG entry calls the trade-off out so users
  understand the recovery action (rename to UTF-8) restores delta mode. A future slice could
  special-case the gate (record HEAD when the only entries on `result.errors` are lossy-path
  warnings), but that's out of scope here.
- **Reconcile-path symmetry.** `reconcile_ignored` calls `walk_md_files` during `.loreignore`
  reconciliation; without the `ReconcileStats.lossy_warnings` plumbing (U1), a lossy filename
  encountered during reconciliation would surface only as a `lore_debug!` log line and never reach
  `IngestResult::errors`. The shadow-path test `delta_reconcile_surfaces_lossy_warning_from_walk`
  (U2) is the regression guard.

---

## System-Wide Impact

- **CLI exit status:** unchanged. R8 is Tier-2, same as the other `IngestResult::errors` entries —
  `lore ingest` exits 0 even with a lossy path warning. Existing per-file errors
  (`Failed to index …`) already do this.
- **MCP `lore_status` tool:** unaffected. The lossy filename never reaches the database, so it never
  appears in scan-set state, source counts, or any structured field.
- **Search:** unaffected — the lossy file is not indexed.
- **Delta ingest:** unaffected (paths come from `git diff`, not the walker). If a non-UTF-8 file is
  later renamed to a valid UTF-8 name via `git mv`, the delta path picks it up normally.
- **Windows:** unaffected at runtime (NTFS allows non-UTF-8 names in theory, but the brainstorm
  explicitly scopes R8 as Linux-dominant and the test is Unix-gated; the production code path runs
  on Windows but the `Cow::Owned` discriminator is the right answer regardless of OS).

---

## Verification

Slice D is complete when:

1. `walk_md_files` returns a `Vec<String>` of lossy-path warnings as its third tuple element; lossy
   files are absent from the keeper list.
2. All three call sites (`discover_md_files`, `effective_scan_state`, `reconcile_ignored`)
   destructure the new return shape.
3. `discover_md_files`'s "Found N markdown files" progress message subtracts the lossy count from
   the `(N excluded by .loreignore)` suffix so lossy files are no longer misattributed to
   `.loreignore`.
4. `effective_scan_state` derives state from `walked_count -
   lossy_warnings.len()` so an
   all-lossy directory routes to `FilesystemEmpty` rather than `AllIgnored`.
5. `reconcile_ignored` propagates lossy warnings to the caller via a new
   `ReconcileStats.lossy_warnings` field; `ingest()` folds them onto `result.errors` on the delta
   path.
6. `full_ingest` folds the warnings into `IngestResult::errors` verbatim.
7. The R11.9 inline test plants a non-UTF-8 `.md` filename, runs `full_ingest`, and asserts (a) one
   error string containing "not valid UTF-8", (b) the file was not indexed, (c) the valid-UTF-8
   sibling was indexed normally.
8. The four shadow-path tests pass: `full_ingest_with_only_non_utf8_files_routes_to_empty_warning`,
   `discover_md_files_progress_line_does_not_misattribute_lossy_to_loreignore`,
   `delta_reconcile_surfaces_lossy_warning_from_walk`,
   `full_ingest_passes_valid_utf8_unicode_paths_unchanged`.
9. `just ci` is green.
10. `CHANGELOG.md` and `ROADMAP.md` reflect the shipping; the CHANGELOG entry names the HEAD-record
    trade-off explicitly.
11. Plan frontmatter flips to `status: completed` in the final commit.

---

## Assumptions

- The `RefCell<Vec<String>>` collector pattern from slice C is portable to `walk_md_files`'s
  filter_map closure. If borrow-checker friction forces an iterator → `for` rewrite, that is an
  acceptable implementation-time fallback (still scoped to `walk_md_files`).
- `OsStr::from_bytes(&[0xFF, b'.', b'm', b'd'])` produces a path that the host tmpfs accepts via
  `std::fs::File::create`. If it doesn't (unusually strict mount option), the test gracefully
  degrades to marking itself `#[ignore]` at runtime rather than landing a flaky assertion. Confirmed
  working pattern in Rust's std-lib tests on standard Linux tmpfs.
- The four pre-existing sandbox-only test failures noted in `project_sandbox_grants.md` remain the
  only test-suite failures; the new tests are not affected by sandboxing (no network, no git binary,
  pure filesystem + in-memory DB).
- `IngestResult::errors` is the right severity channel. The brainstorm pins this explicitly; the
  user memory `feedback_changelog_user_facing_only.md` confirms it's user-visible enough to warrant
  a CHANGELOG entry.
