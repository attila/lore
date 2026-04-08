---
date: 2026-04-08
topic: edge-case-handling
---

# Edge Case Handling

## Problem Frame

The roadmap carries a bullet — **"Edge case handling (empty knowledge dir, non-git dir, duplicate
titles, unicode filenames)"** — that has been deferred since the first pass on ingest landed. With
release process prep (`cargo-zigbuild`, prebuilt binaries) coming up next, this is the natural
moment to close it: users installing lore from a prebuilt binary will hit these edges in setups the
original dogfooding loop did not exercise, and a binary release that crashes or misleads on a fresh
`git init` or a title with non-ASCII characters is a bad first impression.

The four bullets span a wide severity range. A code scan against today's `src/ingest.rs` and
`src/git.rs` (commit `6037cf1`, branch `main`) shows:

- **Empty knowledge dir** — already works. `ingest_empty_directory_returns_zero` in `src/ingest.rs`
  covers it; `full_ingest` returns zero results with no errors.
- **Non-git dir** — mostly handled by PR #30. `ingest` falls back to full mode via
  `git::is_git_repo`, write ops return `CommitStatus::NotCommitted`, and inbox-prefix writes
  correctly error. `is_git_repo` also returns `false` when the `git` binary itself is missing
  (implicit via `Command::new("git").output()` returning `Err`). One remaining rough edge: on a
  fresh `git init` with no commits, `full_ingest`'s post-run HEAD-recording block at
  `src/ingest.rs:674-680` silently does nothing — the `let Ok(head) = git::head_commit(...)`
  short-circuits the let-chain when HEAD does not resolve. The ingest succeeds, nothing is recorded,
  and every subsequent ingest falls through to the "No previous ingest recorded — running full
  ingest" branch at `src/ingest.rs:130-133`, which re-runs full mode until the user makes their
  first commit. The code is _correct_ — the user is not misled by a spurious warning, and delta mode
  kicks in naturally after the first commit — but the "No previous ingest recorded" progress line is
  misleading on repeat runs, since the real reason is "you haven't committed yet", not "we lost our
  state".
- **Duplicate titles** — genuine correctness bug. Two distinct titles that slugify to the same
  filename (e.g. "API: Notes" and "API/Notes" both → `api-notes.md`) trigger the
  `File already exists. Use update_pattern instead.` error from `add_pattern`. The message is
  misleading in the collision case: the user wanted a new pattern, not an update.
- **Unicode filenames** — genuine correctness bug. `slugify` uses `char::is_alphanumeric`
  (Unicode-aware) and applies no Unicode normalisation, which means visually identical titles
  produce different slugs depending on encoding. `café` typed with a precomposed `é` (NFC, one
  codepoint U+00E9) slugifies to `café`, because U+00E9 passes `is_alphanumeric`. `café` typed with
  a combining acute (NFD, `e` + U+0301) slugifies to `cafe`, because U+0301 is a combining mark
  (Unicode category Mn) and the `is_alphanumeric` filter drops it. Users who expect visually
  identical titles to behave identically get either a divergent filename or — if both encodings are
  used for the same pattern in different sessions — two patterns they cannot tell apart in a
  listing.

This brainstorm scopes a **pre-release robustness pass** focused on user-visible correctness. It
fixes the two real bugs (slug collisions, Unicode normalisation), replaces the misleading no-HEAD
progress line with one that explains the state, and adds regression tests for all four roadmap
bullets plus the two sub-cases (no-HEAD, missing git binary) so release artefacts cannot regress
them. Speculative hardening and UX polish beyond the single no-HEAD line are out of scope; they can
be addressed opportunistically later.

## Requirements

**Slug collisions**

- R1. `add_pattern` must distinguish a **slug collision** (two distinct titles that slugify to the
  same filename) from an **intentional re-use** (the same title is already present as a pattern).
  Today both cases hit the same `File already exists. Use update_pattern instead.` message, which is
  correct only for the re-use case.
- R2. When a collision is detected, `add_pattern` returns an `anyhow::Error` whose message names the
  colliding slug, the existing file, and (if extractable) the existing file's title — for example:
  `Slug "api-notes" already used by api-notes.md (title: "API Notes"). Choose a
  different title or call update_pattern to modify the existing file.`
  When the existing file has no extractable title, the parenthetical becomes `(no title heading)`
  instead of repeating the filename stem. The "choose a different title" phrasing makes clear that
  `update_pattern` is the fallback for the re-use case, not the fix for the collision case.
- R3. Collision detection reuses the existing `file_path.exists()` check in `add_pattern`
  (`src/ingest.rs:885`) — it does not scan the knowledge directory. When the check fires, the
  implementation reads the conflicting file and calls the existing `extract_title` helper (in
  `src/chunking.rs`, already used by `update_pattern` at `src/ingest.rs:930`) to recover the
  existing file's title. If `extract_title` returns `None` (frontmatter-only file, H2/H3-only file,
  no `#` heading), the error uses the wording `no title heading` rather than repeating the filename
  stem as if it were a title. The check does not consult the database index, so it works in a fresh
  session that has not yet ingested.
- R4. The existing re-use path — title slugifies to an existing file whose title matches — keeps its
  current message, adjusted only so that collision and re-use use different, unambiguous wording. No
  behaviour change for the re-use case.

**Unicode filenames**

- R5. Add a dependency on the `unicode-normalization` crate. This is the minimal option in the Rust
  ecosystem — a single-purpose crate with UCD tables baked in, no transitive data providers.
  `icu_normalizer` is explicitly not the lighter alternative it was first framed as: it is part of
  the ICU4X family and requires an `icu_provider` data backend, which materially increases binary
  size. The crate choice is not a deferred decision.
- R6. `slugify` NFC-normalises its input before producing the slug. The normalisation is applied
  once, to the entire title, before the `char::is_alphanumeric` filter runs. After this change,
  `café` (NFC) and `café` (NFD) produce identical slugs and therefore collide deterministically via
  R1–R4 instead of silently clobbering each other on the filesystem.
- R7. Full Unicode is preserved in slugs; no ASCII-folding or transliteration. `Café Tip` still
  produces `café-tip.md`, `日本語` still produces a non-empty slug, emoji and other non-alphanumeric
  codepoints still collapse to `-` exactly as today.
- R8. During directory walk (`discover_md_files` in `src/ingest.rs`), detect when
  `to_string_lossy()` actually lost data — `Cow::Owned` rather than `Cow::Borrowed` — and push the
  file onto `IngestResult::errors` with a warning rather than silently indexing a corrupted path.
  This converts the rare non-UTF-8 filename case from invisible data loss into an auditable error.
  Files with valid UTF-8 paths are unaffected and pay no runtime cost.

  Implementing this requires changing the signature of `walk_md_files` / `discover_md_files` to
  propagate warnings back to the caller: either add a `&mut Vec<String>` errors accumulator
  parameter, or change the return type to include a `Vec<String>` of warnings alongside the paths.
  The latter is the cleaner choice given `walk_md_files` is already a pure helper returning a tuple;
  planning should pick one explicitly. The test in R11.10 is `#[cfg(unix)]`-gated because
  `OsStr::from_bytes` is a Unix-only extension; Windows builds will compile but not exercise the
  assertion. R8 is acknowledged as Linux-dominant in practice because APFS and HFS+ enforce UTF-8 at
  the filesystem layer — on macOS the code path can compile and run but is unlikely to fire in
  normal use.

**Non-git dir (no-HEAD case)**

- R9. On a git repository with no HEAD (fresh `git init`, zero commits), `full_ingest` and the
  delta-ingest entry point both emit a clear, one-shot progress line explaining the state — for
  example `No commits yet — HEAD will be recorded after your first commit.` The message replaces the
  misleading `No previous ingest recorded — running full ingest` line _only_ in the no-HEAD case;
  any other reason for falling through to full mode keeps its existing wording. Discrimination uses
  `git symbolic-ref --quiet HEAD` to detect the unborn-branch state specifically (HEAD exists as a
  symbolic ref pointing to an unborn branch), distinguishing it from other `head_commit` failures
  like corrupted packed-refs or permission errors, which must still surface as warnings.

  This requirement deliberately crosses the "no UX copy changes" scope boundary set below, because
  the no-HEAD case is the only user-visible symptom of the non-git bullet that actually needs a fix,
  and it needs one at the copy layer. Without R9 the document would ship zero work on the non-git
  bullet — which is defensible, but the roadmap line promises something.
- R10. Once the first real commit lands in the knowledge dir, the next `ingest` call falls through
  to `full_ingest` via the existing "No previous ingest recorded" path, which now records HEAD and
  enables delta mode on the run after. No code change is required for this behaviour — it already
  works; R10 is a regression-test contract only. R11 covers it.

**Regression tests**

- R11. Add or confirm regression tests for each of the following cases in `src/ingest.rs` tests or a
  new `tests/edge_cases.rs` integration file:
  1. **Empty knowledge directory** returns zero results with no errors (**already covered** by
     `ingest_empty_directory_returns_zero`; confirm it still runs under the new code paths).
  2. **Non-git knowledge directory** ingests successfully and `try_commit` returns `NotCommitted`
     (**already covered** by existing `add_pattern` tests; confirm).
  3. **Fresh `git init` with no commits** (R9): ingest succeeds, the no-HEAD progress line fires
     exactly once, and `META_LAST_COMMIT` is not yet written.
  4. **No-HEAD → commit → re-ingest transition** (R10): sequence is (a) `git init`, (b) write
     markdown, (c) `ingest()`, (d) `git add` + `git commit`, (e) `ingest()` again, (f) assert the
     second call recorded HEAD and the third call enters delta mode. This must exercise the
     top-level `ingest()` entry point, not `full_ingest()` directly, because the transition is the
     load-bearing behaviour.
  5. **Missing `git` binary on PATH**: simulated via `assert_cmd` spawning the `lore` binary as a
     subprocess with `.env_clear().env("PATH", "")` — not via in-process `set_var`, which would race
     with every other test that invokes git. `is_git_repo` must return false, ingest must fall
     through to full mode, write ops must return `NotCommitted`.
  6. **Slug collision** — two distinct titles colliding into the same slug: the second `add_pattern`
     returns a collision-specific error naming the existing file and its title.
  7. **Slug collision with no-heading existing file**: the conflicting file has only frontmatter and
     body (no `# Heading`), so `extract_title` returns `None`; the error message uses
     `(no title heading)` rather than repeating the filename stem.
  8. **NFC/NFD slug convergence**: the title `café` typed with a combining acute (NFD)
     post-normalisation produces slug `café` (the four-codepoint NFC form), not `cafe`. This test
     specifically guards against the pre-fix behaviour where the combining mark was stripped by
     `is_alphanumeric`.
  9. **Empty-after-normalisation slug**: a title composed solely of combining marks or
     non-alphanumeric codepoints still triggers the existing
     `Title must contain at least one
     alphanumeric character` error after normalisation. Guards
     against NFC unexpectedly turning a previously-valid title into an empty slug (or vice versa).
  10. **Non-UTF-8 filename on disk** (R8): constructed via `OsStr::from_bytes` inside a
      `#[cfg(unix)]`-gated test, surfaces in `IngestResult::errors` as a lossy-conversion warning
      rather than being silently indexed.

## Success Criteria

- The two correctness bugs are fixed. Running `lore ingest` against the NFC/NFD edge case no longer
  produces divergent slugs, and calling `add_pattern` with two distinct titles that collide to the
  same slug no longer produces the misleading `Use update_pattern instead` message.
- A user running `lore ingest` on a freshly `git init`-ed directory sees a clear, one-shot progress
  line explaining that HEAD will be recorded after their first commit, and subsequent ingests
  transition into delta mode once a real commit exists.
- The regression test suite covers the ten concrete scenarios listed in R11 so CI catches any future
  regression.
- User-facing copy changes are bounded to: the two error messages in R2 (slug collision and the
  no-heading variant), the new lossy-path warning in R8, and the no-HEAD progress line in R9. No
  changes to any other progress line, help text, or documentation beyond what is needed to reference
  the new wording.

## Scope Boundaries

- **No friendlier ingest progress copy.** `Found 0 markdown files` stays as it is. Any polish on
  empty-directory or empty-database UX is out of scope and can be handled opportunistically.
- **No `lore search` empty-DB hint.** Today a search against an empty index returns
  `No results
  found.` That is accurate; adding a "run `lore ingest` first" hint is deferred.
- **No ASCII-folding or transliteration.** R7 commits to keeping full Unicode in slugs. Users who
  need strictly ASCII filenames can choose ASCII titles.
- **No broader edge-case audit.** This pass is scoped to the four roadmap bullets plus the no-HEAD
  and missing-git sub-cases surfaced during the scan. Other paths (search on empty query, database
  corruption, concurrent ingest, partial writes during crash) are not in scope. If any are
  discovered to be user-reachable during implementation, they should be split into their own
  brainstorms rather than silently absorbed.
- **No changes to the inbox-branch workflow.** PR #30 already handles the non-git error case for
  inbox writes and there is no known bug there.
- **No changes to the `slugify` signature or call sites beyond the normalisation pass.** `slugify`
  remains an internal helper inside `src/ingest.rs`; there is no public API surface to preserve.

**Known limitations (not fixed by this pass)**

- **Collision detection treats any pre-existing `.md` file at `knowledge_dir` root as a potential
  pattern.** If a user has a `README.md` in their pattern repo (from a legacy dotfile setup, for
  instance) and runs `add_pattern(title = "README")`, R2's collision message will name the file even
  though it was never authored by lore. This is not a regression — today's `file_path.exists()`
  check already triggers the same conflict, just with a less precise error. A real fix requires
  either a "managed by lore" marker in the file format or a pattern-shape sniff, which is out of
  scope. The scoped fix (R2's improved error text) is still a strict improvement over today.
- **NFC alone does not cover case-insensitive filesystem collisions.** On case-insensitive APFS (the
  macOS factory default), two titles whose slugs differ only in case, or whose Rust `to_lowercase()`
  mapping diverges from the OS's case-folding rules (Turkish dotted/dotless i, German sharp s,
  certain Greek letters), can still collide at the filesystem layer. This pass catches NFC/NFD
  identity collisions — the common case — but not locale-sensitive case-folding edges. Users hitting
  the uncovered case will still see the current `Use update_pattern instead` error rather than the
  cleaner R2 message. Full Unicode case-folding via `icu_casemap` or a similar dependency is out of
  scope and deferred to a followup if a real user report justifies it.

## Key Decisions

- **Distinct error for slug collisions, not auto-suffix.** Auto-suffixing (`api-notes-2.md`,
  `-3.md`, etc. modelled on `generate_branch_name` in `src/git.rs`) is ergonomic but encourages
  silent divergence: users don't notice the suffix and end up with two near-identical patterns. The
  distinct-error path forces an intentional choice and matches the "loud failures over silent
  recovery" tone the rest of the CLI already takes. The error message explicitly names
  `update_pattern` as the escape hatch so the user is not left stuck.
- **NFC normalisation, keep full Unicode.** NFC is the Unicode standard's canonical composition form
  and is the cheapest way to make visually identical strings produce deterministic slugs. Stripping
  to ASCII would punish non-English pattern authors and lose information from titles in non-Latin
  scripts. The `unicode-normalization` crate is the established minimal dependency.
- **Slug collision detection (R1–R4) depends on NFC normalisation (R5–R7) landing first.** Without
  NFC, R3's single-file collision check will still misroute NFD-encoded titles because the
  pre-normalisation slugs are different (`café` vs `cafe`). Planning must sequence the work so the
  two land together — either in one commit or with the collision-detection commit gated on the NFC
  commit. A reviewer approving a partial split that contains R1–R4 without R5–R7 would ship an
  incomplete fix.
- **No-HEAD case fixed at the progress-line layer, not by suppressing a phantom warning.** The
  original framing of this work was wrong: there is no spurious warning today, only a misleading
  `No previous ingest recorded — running full ingest` progress line that fires every run until the
  user makes their first commit. The fix is a targeted, one-shot progress message that explains the
  state, gated on `git symbolic-ref --quiet HEAD` returning an unborn branch. Other `head_commit`
  failure modes (corrupted packed-refs, permission errors, detached HEAD with a broken ref) still
  surface with the existing wording — the discriminator is specifically the unborn-branch state, not
  "any head_commit failure".
- **Lossy path conversion is an ingest error, not a panic or a silent pass.** Pushing lossy
  conversions onto `IngestResult::errors` matches the existing batch-error collection pattern in
  `full_ingest` and lets the rest of the ingest proceed. Treating them as panics would be
  user-hostile; treating them as silent successes would let corrupted filenames enter the index.
- **Minimal UX polish, one deliberate exception.** The pre-release robustness pass deliberately
  stops at correctness bugs and their regression tests. Friendlier empty-directory copy, empty-DB
  search hints, and similar polish are real opportunities but carry ongoing test cost; they are
  deferred until a concrete user report justifies them. The exception is R9's no-HEAD progress line,
  which crosses the boundary because the no-HEAD state is the only user-visible symptom of the
  non-git bullet that benefits from action, and the action is a copy change.
- **`add_pattern` is human-first; MCP agents must call `search_patterns` before writing.** R2's
  error text is written for humans. The primary caller of `add_pattern` in 2026 is an MCP agent, but
  the v1 position is that the agent should call `search_patterns` (or read the knowledge dir) before
  writing to avoid collisions in the first place, and should retry with a disambiguated title on the
  loud failure. A structured error type (e.g. `SlugCollisionError` with `existing_path` and
  `existing_title` fields for downcast) is a reasonable followup once a real agent-loop use case has
  surfaced; it is not in scope for this pass. The error message remains the sole contract.

## Dependencies / Assumptions

- The `unicode-normalization` crate (or an equivalent minimal alternative) is added to `Cargo.toml`.
  No other crate additions are expected.
- `char::is_alphanumeric`'s Unicode coverage matches the expectations encoded in R6 and R7 —
  verified by the Rust standard library documentation and already exercised by the existing
  `slugify_special_characters` test.
- Git tests use the same `git_init` helper already present in `src/ingest.rs` tests. The no-HEAD
  test adds a new helper that initialises a repo without making an initial commit.
- The missing-git-binary test runs `assert_cmd` against the compiled `lore` binary with
  `.env_clear().env("PATH", "")` applied to the child process only. Mutating the current test
  process's `PATH` via `std::env::set_var` is explicitly ruled out: the 2024 edition marks `set_var`
  `unsafe` and unit tests run in parallel, so mutating the shared `PATH` would race with every other
  test that invokes `git` (and there are many). `assert_cmd` is already in `dev-dependencies`.
- The no-HEAD discriminator uses `git symbolic-ref --quiet HEAD`, which returns the target ref name
  on success and exits non-zero when HEAD is detached or missing. Combined with a follow-up
  `git rev-parse --verify <ref>`, this cleanly distinguishes "HEAD points to an unborn branch" from
  "HEAD is detached" and "HEAD is corrupted".

## Outstanding Questions

### Resolve Before Planning

_(none)_

### Deferred to Planning

- [Affects R11] Whether the edge-case tests live alongside the existing `ingest.rs` unit tests or in
  a new `tests/edge_cases.rs` integration file. Either is acceptable; planning should pick based on
  which path is cheaper to keep green under the delta-ingest helper surface.

## Next Steps

→ `/ce:plan` for structured implementation planning. This work is independent of every other open
roadmap item, so planning can proceed directly and the resulting pull request can ship ahead of the
release process work.
