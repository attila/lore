---
title: "feat: Unicode NFC normalisation and slug collision detection"
type: feat
status: active
date: 2026-05-10
origin: docs/brainstorms/2026-04-08-edge-case-handling-requirements.md
---

# Unicode NFC Normalisation and Slug Collision Detection

## Summary

NFC-normalise `slugify`'s input so visually identical titles (NFC vs. NFD `café`) produce identical
slugs, then teach `add_pattern` to discriminate a slug collision (two distinct titles sharing a
slug) from an intentional re-use (the same title written twice) and surface a tier-1 hard-fail error
naming the colliding slug, the existing file, and its title. Single PR, two commits, sequenced A
before B per the brainstorm.

---

## Problem Frame

`slugify` runs `char::is_alphanumeric` over the lowercased title without normalising Unicode, so NFD
input silently strips combining marks (`café` typed with a combining acute → `cafe`); and
`add_pattern`'s collision check returns the same `Use update_pattern instead.` message for a genuine
collision as for an intentional overwrite. Both are user-visible correctness bugs that would land
badly with the upcoming prebuilt-binary release. See origin document for the full analysis against
`src/ingest.rs` (commit `6037cf1`).

---

## Requirements

Carried verbatim from origin (`docs/brainstorms/2026-04-08-edge-case-handling-requirements.md`),
scoped to Slices A and B only.

**Slice A — Unicode NFC normalisation**

- R5. Add `unicode-normalization` crate dependency.
- R6. `slugify` NFC-normalises input before the `char::is_alphanumeric` filter; post-fix, NFC and
  NFD `café` produce identical slugs.
- R7. Full Unicode preserved in slugs; no ASCII-folding or transliteration.

**Slice B — Slug collision detection**

- R1. `add_pattern` distinguishes a slug collision from an intentional re-use (today both hit the
  same `Use update_pattern instead.` message).
- R2. On collision, return an `anyhow::Error` whose message names the colliding slug, the existing
  file, and the existing file's title (or `(no title heading)` if none extractable).
- R3. Detection reuses the existing `file_path.exists()` check (`src/ingest.rs:1069`); reads the
  conflicting file and calls `extract_title` (`src/chunking.rs:329`) — no scan of the knowledge
  directory, no database consultation.
- R4. The re-use path keeps its current behaviour, with wording adjusted only so collision and
  re-use are unambiguously different messages.

**Regression tests (R11 subset)**

- R11.5. Two distinct titles colliding into the same slug → second `add_pattern` returns the
  collision-specific error naming the existing file and its title.
- R11.6. Collision with a no-heading existing file → error message uses `(no title heading)`.
- R11.7. NFC/NFD slug convergence — `café` typed with a combining acute, post-normalisation, slugs
  to the four-codepoint NFC form, not `cafe`.
- R11.8. Empty-after-normalisation slug — a title made solely of combining marks or non-alphanumeric
  codepoints still triggers the existing `Title must contain at least one alphanumeric character`
  error.

---

## Scope Boundaries

- **No structured error type.** R2's contract is the error message text. A `SlugCollisionError` with
  downcastable `existing_path` / `existing_title` fields is deferred per origin's Key Decisions
  until a real agent-loop use case surfaces.
- **No ASCII-folding or transliteration.** R7 commits to keeping full Unicode in slugs.
- **No auto-suffixing recovery.** Origin's Key Decisions explicitly reject silent
  `api-notes-2.md`-style recovery; the loud failure forces an intentional choice.
- **No case-folding edge handling.** NFC alone does not cover case-insensitive APFS collisions or
  locale-sensitive case-folding (Turkish dotted/dotless i, German sharp s, certain Greek letters);
  origin lists these as known limitations.
- **No NFD-on-disk vs NFC-incoming directory scan.** A knowledge directory synced from a Mac
  filesystem that stores filenames as NFD onto a Linux byte-comparing filesystem ends up with
  `cafe\u{0301}.md` on disk; an `add_pattern(title = "café")` call on Linux slugs to NFC and writes
  to a different inode (`café.md`), creating two parallel files for the same logical pattern.
  Surfaced by adversarial review (R5, code-review run `20260510-194040-rev1`). Same family as the
  case-folding limitation above — Unicode-encoding-vs-filesystem mismatch — and out of scope for
  this slice. Fixing it requires enumerating the knowledge directory on every `add_pattern` to look
  for NFC-equivalent existing filenames; cost is non-trivial and benefit is marginal until a real
  user reports the case. Deferred until a concrete report justifies it.
- **No collision detection on the inbox-branch write path.** Origin scopes the fix to direct-write
  `add_pattern`; the inbox-prefix branch in `add_pattern` (`src/ingest.rs:1048-1065`) is unchanged.
- **No MCP server-level changes.** `src/server.rs::handle_add` wraps errors with the prefix
  `Failed to add pattern: {e}` (`src/server.rs:790`), so the user-visible message via MCP is
  `Failed to add pattern: Slug "..." already used by ...` — the prefix is pre-existing and
  acceptable. No `handle_add` code changes; no new server tests.
- **No documentation or README changes** beyond a `CHANGELOG.md` entry. Origin's Success Criteria
  bound user-facing copy changes to the two error messages and no further doc work.

### Deferred to Follow-Up Work

- **Slice C — No-HEAD progress line** (R9, R10 + R11.2, R11.3): separate later PR on the same branch
  family. Independent of A and B.
- **Slice D — Lossy-path warning** (R8 + R11.9): separate later PR; Unix-only test gating.
- **Slice E — Missing-git regression test** (R11.1, R11.4): separate later PR; test-only.

---

## Context & Research

### Relevant Code and Patterns

- `src/ingest.rs:1618` — `slugify` (target of Slice A's normalisation pass).
- `src/ingest.rs:1028` — `add_pattern` (target of Slice B's collision discrimination at line 1069's
  `file_path.exists()` check).
- `src/ingest.rs:1128` — existing `extract_title` use site in `update_pattern`; mirror that pattern
  in Slice B for the collision branch (read existing file, extract title, fall back when `None`).
- `src/chunking.rs:329` — `pub fn extract_title(content: &str) -> Option<String>`. Already public,
  no API change needed.
- `src/ingest.rs:1731-1743` — existing `slugify_*` unit tests; new R11.7/R11.8 tests slot in
  alongside.
- `src/ingest.rs:2069-2081` — existing `add_pattern_rejects_existing_file` test; new R11.5/R11.6
  tests slot in alongside.

### Institutional Learnings

- `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` — Slice B's collision is tier-1
  (hard fail) per the ladder; the worked example in that doc cites `src/ingest.rs:add_pattern`
  directly.
- Origin document is the authoritative scope artefact; the brainstorm itself stays as workspace
  scratch and will not land on the implementation PR.

### External References

- `unicode-normalization` crate (https://docs.rs/unicode-normalization). The crate's `nfc()`
  iterator adapter is the call site; UCD tables are baked into the crate, no runtime data provider.
  Origin's Key Decisions explicitly rule out `icu_normalizer` on binary-size grounds.

---

## Key Technical Decisions

- **Two units, one PR, separate commits.** Slice A and Slice B ship together (the brainstorm warns
  against shipping B without A), but each lands as its own commit so the dependency ordering is
  visible in review. Squash-merge to `main` collapses them at merge time.
- **Tests inline in `src/ingest.rs::tests`, not `tests/edge_cases.rs`.** Resolves origin's
  deferred-to-planning question. `tests/edge_cases.rs` is reserved for CLI-spawn integration tests;
  `add_pattern` has no CLI subcommand, and the MCP layer is a transparent passthrough that adds
  nothing testworthy on top.
- **Re-use detection via title round-trip, not slug equality.** Slice B's discriminator: read the
  existing file, extract its title, and compare it (post-NFC) to the incoming title. If both match,
  it is a re-use; otherwise it is a collision. Compares titles after NFC normalisation so an NFD/NFC
  pair of the same visual title classifies as re-use, not collision. Origin's R2 wording assumes
  this shape implicitly.
- **Manually-edited headings classify as collision, not re-use.** A side-effect of the title
  round-trip: a lore-managed file whose `# heading` was edited away from what `slugify` originally
  produced will, on a follow-up `add_pattern` call with the original title, read as collision. This
  is the correct tier-1 behaviour per the CLI ladder — treating it as re-use would silently
  overwrite the user's manual edit. The R2 error message names `update_pattern` as the escape hatch,
  so the user has a clear one-step recovery.
- **No-heading fallback uses `(no title heading)`, not the filename stem.** Origin's R3 is explicit
  on this; the existing file_stem-based fallback in `update_pattern` (`src/ingest.rs:1128`) is a
  different concern and stays as it is.
- **CHANGELOG entry combines both slices.** Mirrors how the empty-knowledge-dir feature landed; a
  single `[Unreleased]/Added` bullet under "Edge case handling" covering both behaviours, written
  for the next release.

---

## Open Questions

### Resolved During Planning

- **Test file location** (deferred-to-planning in origin): inline in `src/ingest.rs::tests`. See Key
  Technical Decisions.
- **Whether to extend the MCP server's `handle_add` tests**: no. `handle_add` wraps with the
  pre-existing `Failed to add pattern:` prefix but does not transform the inner ingest error string;
  the ingest-layer test on the inner error is the contract. The U2 manual-smoke verification will
  see the prefix — that is expected, not a regression.

### Deferred to Implementation

- **Exact wording of the collision error message variant for the `(no title heading)` case.**
  Origin's R2 sketches `Slug "X" already used by foo.md (title: "...").`; the no-heading variant
  becomes `Slug "X" already used by foo.md (no title heading).` The implementer should verify the
  final wording reads cleanly when surfaced through the MCP `add_pattern` tool response by checking
  one happy and one sad path manually.
- **Whether the existing `slugify_*` test asserts slug content match shape that would over-constrain
  the NFC change.** Implementer should run the existing slugify tests after the change; if any
  pre-fix assumption was implicitly NFC (the strings are ASCII so this is unlikely to surface),
  document the diff in the commit body.

---

## Implementation Units

### U1. Add `unicode-normalization` dependency and NFC-normalise `slugify`

**Goal:** `slugify` produces the same slug for visually identical titles regardless of input
normalisation form. Single function change plus one dependency.

**Requirements:** R5, R6, R7

**Dependencies:** None.

**Files:**

- Modify: `Cargo.toml` — add `unicode-normalization = "0.1"` to `[dependencies]`.
- Modify: `src/ingest.rs` — `slugify` (line 1618).
- Test: `src/ingest.rs::tests` — extend the `// -- slugify ---` block at line 1728.

**Approach:**

- In `slugify`, NFC-normalise the title once before the existing `to_lowercase().chars()` pipeline.
  The `unicode-normalization` crate's `UnicodeNormalization` trait provides `.nfc()` on both `&str`
  and `Chars<'_>`, returning a `Recompositions<I>` char iterator. Pin the call shape:
  `title.nfc().collect::<String>().to_lowercase().chars()...` — the intermediate `String` is the
  cleanest way to keep the existing `to_lowercase()` call site, and slugify is not hot path (titles
  are short, allocation cost is negligible).
- The lowercase / `is_alphanumeric` / dash-collapse logic stays unchanged. Combining marks (NFD)
  that survive into the lowercase stage now arrive as their NFC composed forms and pass
  `is_alphanumeric`.
- Origin's R7 invariant is preserved by construction: NFC neither folds nor transliterates, it only
  re-composes.

**Patterns to follow:**

- The existing `slugify` function shape — pure function, no allocation surprises, returns `String`.
  Keep the change additive.

**Test scenarios:**

- Happy path: `slugify("Café Tip")` (NFC) → `"café-tip"` — confirms full Unicode preservation per
  R7.
- Edge case (R11.7): `slugify("cafe\u{0301}")` (NFD `café`, `e` + combining acute) → `"café"` — the
  four-codepoint NFC form. Pre-fix this slugged to `"cafe"` because the combining mark was stripped
  by `is_alphanumeric`.
- Edge case: `slugify("café")` (NFC) → `"café"` — confirms NFC input passes through unchanged.
- Edge case (R11.8): `slugify("\u{0301}\u{0301}")` (combining marks only) → empty string; the
  caller's `slug.is_empty()` check still fires
  `Title must contain at least one alphanumeric
  character` in `add_pattern` (verified by an
  `add_pattern`-level assertion in U2's test list, or inline here at the slugify level —
  implementer's call).
- Happy path: `slugify("日本語")` → non-empty CJK slug — confirms non-Latin scripts survive.
- Pre-existing tests `slugify_basic`, `slugify_special_characters`,
  `slugify_leading_trailing_dashes` remain green unchanged.

**Verification:**

- `cargo test --lib slugify` covers the new and existing slugify tests.
- `cargo build` succeeds with the new dependency.
- Binary size delta documented in the commit body if non-trivial (the crate is small, but worth
  recording for the binary-size investigation parked in workspace memory).

---

### U2. Slug collision detection in `add_pattern`

**Goal:** `add_pattern` returns a distinct, helpful error when a new title slugifies to an existing
file with a different title; the intentional re-use path keeps its existing wording, adjusted only
for unambiguous distinction.

**Requirements:** R1, R2, R3, R4

**Dependencies:** U1.

**Files:**

- Modify: `src/ingest.rs` — `add_pattern` (line 1028) at the `file_path.exists()` branch (line
  1069).
- Test: `src/ingest.rs::tests` — extend the `// -- add_pattern ---` block at line 2037.
- Modify: `CHANGELOG.md` — single `[Unreleased]/Added` entry combining U1 and U2.

**Approach:**

- At the existing `file_path.exists()` branch, replace the unconditional `bail!` with a
  discriminator:
  1. Read the existing file at `file_path` via `std::fs::read_to_string` and propagate any read
     error verbatim with `?` (e.g. permission denied). The case is rare — `exists()` returned true
     so the path is real — and a generic IO error is more honest than misclassifying as
     no-heading-collision.
  2. Call `extract_title` on the contents.
  3. If `Some(existing_title)` and the **NFC-normalised** existing title equals the incoming title
     (also NFC-normalised), this is a re-use: keep current behaviour with wording adjusted to make
     the distinction unambiguous.
  4. Otherwise this is a collision: build the error per R2's wording, naming the colliding slug, the
     existing file's basename, and the existing title (or `(no title heading)` when `extract_title`
     returned `None`).
- Origin's R4 invariant: re-use behaviour unchanged in shape, wording adjusted minimally so
  collision and re-use are unambiguously different messages. Keep the re-use message phrased around
  `update_pattern`; rephrase only enough that the collision message can be visibly different (e.g.,
  the collision message instructs the user to "choose a different title or call `update_pattern`",
  whereas re-use says "use `update_pattern` to modify"). Final wording is Deferred to
  Implementation.
- The discriminator is single-file: it does not scan the knowledge directory and does not consult
  the database. Works in a fresh session pre-ingest.

**Patterns to follow:**

- `update_pattern` already calls `extract_title` and falls back to `file_stem` on `None`
  (`src/ingest.rs:1128`). Do **not** copy the `file_stem` fallback here — origin's R3 explicitly
  requires `(no title heading)` instead of repeating the filename stem.
- Existing `add_pattern_rejects_existing_file` test (`src/ingest.rs:2069`) is the shape to mirror
  for new collision tests.

**Test scenarios:**

- Happy path / regression: existing `add_pattern_rejects_existing_file` re-use scenario remains
  green with adjusted wording (the test asserts the message contains `already exists` today; if the
  new re-use wording drops that token, update the assertion to match the new wording). Title appears
  verbatim in both files; second call hits the re-use branch.
- Edge case (R11.5): two distinct titles colliding to the same slug — write a file with title
  `"API Notes"` (slug `api-notes`), then call `add_pattern(title = "API: Notes")` (slug
  `api-notes`). Assert the error message contains the slug `api-notes`, the filename `api-notes.md`,
  and the existing title `API Notes`.
- Edge case (R11.6): collision with no-heading existing file — write a file at `api-notes.md`
  containing only frontmatter and a body of plain prose with no `#` line at any position (so
  `extract_title`'s line-by-line scan returns `None`), then call `add_pattern(title = "API Notes")`.
  Assert the error message contains `(no title heading)` rather than `api-notes` repeated as a
  title.
- Edge case: NFC/NFD round-trip re-use — write a file with title `café` (NFC, four codepoints), then
  call `add_pattern(title = "cafe\u{0301}")` (NFD, five codepoints). Both slug to `café`; the
  discriminator's NFC-normalised title comparison classifies this as re-use, not collision.
- Error path (R11.8 follow-up): `add_pattern(title = "\u{0301}\u{0301}")` returns the existing
  `Title must contain at least one alphanumeric character` error. Asserts U1's normalisation does
  not unexpectedly change the empty-slug error contract.
- Error path: `add_pattern(title = "")` still returns
  `Title must contain at least one
  alphanumeric character`. Pre-existing behaviour, kept as a
  regression guard.

**Verification:**

- `cargo test --lib add_pattern` covers the new and existing add_pattern tests.
- Manual smoke through the MCP `add_pattern` tool against a knowledge directory containing one
  pre-existing `api-notes.md` to confirm the collision message reads cleanly when surfaced through
  the MCP response. Do this once for the with-title case and once for the no-heading case.
- `CHANGELOG.md` entry compiles under `[Unreleased]/Added` mirroring the empty-knowledge-dir entry.

---

## System-Wide Impact

- **Interaction graph:** `add_pattern` is invoked from `src/server.rs::handle_add` (MCP tool
  surface) and from in-process callers in tests. The MCP layer is a transparent passthrough — the
  new error string propagates as-is into the tool response's error channel. No transformation layer
  to update.
- **Error propagation:** Both new error variants flow through `anyhow::Result` exactly like the
  current `Use update_pattern instead.` message. No new error type; no `Display`/`Debug` work; no
  `?` chain disturbance.
- **State lifecycle risks:** None. The collision discriminator runs before `std::fs::write`, so a
  failed discrimination cannot leave a half-written file. The re-use branch is unchanged in
  ordering.
- **API surface parity:** `slugify` is a private function; no parity work. `add_pattern` is
  `pub
  fn`, but the only contractual change is the error message text — no signature change, no
  new public types.
- **Integration coverage:** None needed. No callbacks, middleware, or multi-layer interactions. The
  MCP test suite exercises `add_pattern` end-to-end already; no new server-level test is required
  since the discriminator's contract is the message text and a unit test covers that.
- **Unchanged invariants:** `slugify`'s public shape (private function, takes `&str`, returns
  `String`) is preserved. The `inbox_branch_prefix` branch of `add_pattern`
  (`src/ingest.rs:1048-1065`) is untouched — collision detection on the inbox path is explicitly out
  of scope. The `update_pattern` flow (`src/ingest.rs:1107`) is untouched.

---

## Risks & Dependencies

| Risk                                                                                                                                                                        | Mitigation                                                                                                                                                                                                                                                                                    |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `unicode-normalization` adds non-trivial binary size                                                                                                                        | Crate is single-purpose with baked-in UCD tables; origin's Key Decisions verified `icu_normalizer` is heavier. Document the binary-size delta in the commit body so the workspace's parked binary-size investigation has the data point.                                                      |
| Re-use discriminator misclassifies an NFC/NFD title pair as collision (wrong direction of the bug)                                                                          | The discriminator NFC-normalises both the existing title and the incoming title before comparison. Test scenario explicitly covers this round-trip case.                                                                                                                                      |
| Existing `add_pattern_rejects_existing_file` test brittle on the wording change                                                                                             | Update the assertion to match the new wording in the same commit as the message change. The test's intent (re-use is rejected) is preserved; only the matched substring shifts.                                                                                                               |
| Collision detection trips on a user-authored `README.md` or other unmanaged `.md` file at the knowledge-dir root                                                            | Origin lists this as a known limitation, not a regression — today's `file_path.exists()` already triggers the same conflict with a less precise error. The new error is strictly more informative. A "managed by lore" marker is out of scope.                                                |
| `extract_title` returns `None` for files lore did write but where the user manually edited the heading away                                                                 | Falls into the `(no title heading)` branch — correct per R3. No bug.                                                                                                                                                                                                                          |
| User has manually rewritten the heading of a lore-managed file and re-runs `add_pattern` with the original title — discriminator classifies as collision rather than re-use | Documented as the correct tier-1 behaviour in Key Technical Decisions. The R2 error names `update_pattern` as the recovery path, so the user has a clear one-step fix. Treating it as re-use would silently overwrite the user's manual edit, which is the data-loss tier-1 protects against. |

---

## Documentation / Operational Notes

- `CHANGELOG.md` — single `[Unreleased]/Added` bullet covering both slices; mirror the
  empty-knowledge-dir entry's voice.
- No README, no `docs/configuration.md`, no `docs/solutions/` updates required.
- The brainstorm at `docs/brainstorms/2026-04-08-edge-case-handling-requirements.md` is workspace
  scratch and will not land on the PR. Excluding it from the merged history is a PR-prep concern,
  handled outside this plan's implementation units (e.g., by branching plan + impl off `main` fresh,
  or by removing the brainstorm file from the branch before opening the PR).

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-04-08-edge-case-handling-requirements.md` (Slices A
  and B; brainstorm itself stays as workspace scratch, does not land on PR).
- **CLI behaviour ladder:** `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` —
  authoritative reference for tier-1 classification of the collision case.
- **Companion plan (shipped in PR #41):**
  `docs/plans/2026-05-04-001-feat-empty-knowledge-dir-validation-plan.md` — pattern reference for
  CHANGELOG voice and inline-test placement.
- **Related code:** `src/ingest.rs:1028` (`add_pattern`), `src/ingest.rs:1618` (`slugify`),
  `src/chunking.rs:329` (`extract_title`).
- **External docs:** https://docs.rs/unicode-normalization (NFC iterator adapters, `is_nfc_quick`).
