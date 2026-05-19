---
date: 2026-05-18
topic: language-table-expansion
type: feat
origin: docs/brainstorms/2026-05-18-language-table-expansion-requirements.md
status: completed
---

# feat: Language Table Expansion

## Summary

Extend the shared `LANGUAGES` table in `src/engine/languages.rs` with 21 new `LanguageEntry`
instances (C, C++, C#, Swift, Kotlin, Shell, Objective-C, Scala, Elixir, Dart, Lua, Nix, Terraform,
Haskell, Clojure, Zig, Perl, Ruby, Java, Groovy, PHP) and back-fill the existing six entries with
missing version-pin markers and lockfiles, including the asymmetric `package-lock.json` on
TypeScript that PR #50 left out. Resolve R5 contested-signal handling at the data level (`.h` shared
between `clang` and `cpp` via R5 multi-entry; `.m` single-owner to `objectivec`). Extend the test
surface with entry-level signal tests, shared-signal multi-membership tests (Gradle three-way
java/kotlin/groovy, CMake two-way, CocoaPods/Xcode two-way, Node ecosystem two-way), and refreshes
to existing negative tests that currently pin `kotlin` as unknown. Four implementation units; no
schema bump, no `LanguageEntry` shape change, no engine code changes — pure data plus tests plus a
single-sentence CHANGELOG bullet and a ROADMAP move.

---

## Problem Frame

The shared `LANGUAGES` table introduced by PR #50 (architecture brainstorm
`docs/brainstorms/2026-05-13-language-detection-architecture-requirements.md`) covers six languages
today. The 2026-05-18 brainstorm settled capacity-building work for the Track 2 trace-accumulation
gap window: extend the table to twenty-seven entries (six existing plus twenty-one new) so that
future pattern-authoring passes have structural retrieval gating available for the new languages
whenever authors declare `language:` for them.

The work is mechanical w.r.t. detection mechanics — every helper in `languages.rs` and every
consumer in `query.rs` / `chunking.rs` / `server.rs` / `status.rs` already iterates the slice or
formats per-token. The slice's open seam is the test surface: the existing module pins canonical
tokens, shared-signal multi-membership, and contested-signal resolution explicitly, and each new
shared-signal claim earns its own pinned assertion under the composition-cascade discipline
(`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`).

A secondary surface is the parity back-fill of the existing six entries — version-pin markers
(`rust-toolchain.toml`, `.python-version`, `.go-version`, `.ruby-version`-style files) and the
asymmetric `package-lock.json` that the JavaScript entry has and the TypeScript entry does not.
Closing the PR #50 oversight is what AE4 in the origin pins down.

---

## Requirements Trace

All requirements from origin:
`docs/brainstorms/2026-05-18-language-table-expansion-requirements.md`.

| R-ID                                                                    | Covered by                                         | Notes                                                                                                                      |
| ----------------------------------------------------------------------- | -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| R1 (21 new `LanguageEntry` instances)                                   | U1                                                 | All 21 added in alphabetical-by-token order; entry data exactly mirrors origin R1 table                                    |
| R2 (parity back-fill of existing six entries)                           | U2                                                 | TypeScript `package-lock.json` fix plus version-pin + lockfile markers on rust/js/ts/python/go                             |
| R3 (contested signals: `.h` shared `clang`/`cpp`, `.m` to `objectivec`) | U1                                                 | Encoded in the new entries' extension lists; AE1/AE2 tests pin the resolution                                              |
| R4 (R5 multi-entry shared signals)                                      | U1 (new-only sets) + U2 (Node ecosystem extension) | Gradle three-way, CMake, CocoaPods/Xcode, `.h`, `clang` command keyword in objectivec; Node ecosystem shared markers in U2 |
| R5 (test surface)                                                       | U1 + U2 + U3                                       | Entry-level tests in U1, shared-signal tests split U1/U2, negative-test rewrites in U3                                     |
| R6 (single-PR delivery + ROADMAP + CHANGELOG)                           | U4                                                 | ROADMAP move and CHANGELOG bullet in the same diff                                                                         |

Acceptance Examples (origin):

| AE-ID                                                  | Covered by | Test location                                              |
| ------------------------------------------------------ | ---------- | ---------------------------------------------------------- |
| AE1 (`.h` → `{clang, cpp}`)                            | U1         | New shared-signal test in `src/engine/languages.rs::tests` |
| AE2 (`.m` → `objectivec`)                              | U1         | New contested-signal resolution test                       |
| AE3 (`gradle build` → `{java, kotlin, groovy}`)        | U1         | New three-way command-keyword test                         |
| AE4 (`package-lock.json` → `{javascript, typescript}`) | U2         | New shared-marker test (the PR #50 regression pin)         |
| AE5 (`Podfile` → `{swift, objectivec}`)                | U1         | New shared-marker test                                     |
| AE6 (`lib/foo.ex` → `elixir`)                          | U1         | Covered by `elixir_entry_has_expected_signals`             |

---

## System-Wide Impact

| Surface                                   | Change                                                                                                                                          | Risk                                                                                                                                |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `src/engine/languages.rs` (slice + tests) | 21 new struct literals; 6 existing entries gain markers; ~50 new tests across entry-level / shared-signal / contested-signal / parity-back-fill | None for callers — every helper iterates the slice                                                                                  |
| `src/engine/query.rs`                     | None                                                                                                                                            | Detection orchestration unchanged; already accumulates multi-language sets                                                          |
| `src/chunking.rs` (test fixture)          | Rewrite `parse_language_canonical_tokens_for_all_initial_six_languages` to iterate `LANGUAGES` rather than hardcode the six                     | Test rename / refactor; no production-code change                                                                                   |
| `src/status.rs` (test fixture)            | Update `format_languages_line_unknown_token_falls_back_to_raw_token` to use a still-unknown token                                               | Test fixture update; no production-code change                                                                                      |
| `src/server.rs`                           | None                                                                                                                                            | Consumes `is_known_token`; behaviour unchanged                                                                                      |
| `CHANGELOG.md`                            | One bullet under `[Unreleased]` `### Added`                                                                                                     | User-visible release-notes entry; convention-bound (one assertive-voice sentence ending in `(#N)` per `feedback_changelog_entries`) |
| `ROADMAP.md`                              | Move "Extend the shared language table" from `## Up Next` to `## Completed`                                                                     | Same-diff move per `feedback_roadmap_update_in_feature_pr`                                                                          |
| Schema                                    | No change                                                                                                                                       | v4 `language_json` from PR #50 unchanged                                                                                            |

---

## Implementation Units

### U1. Add 21 new LanguageEntry instances + entry-level tests + new-only shared-signal tests

**Goal:** Land all 21 new entries in the `LANGUAGES` slice with exact data from origin R1, plus the
entry-level signal tests, the three-way Gradle test, two-way CMake / CocoaPods / `.h` tests, and the
`.m` single-owner test.

**Requirements:** R1, R3 (encoded by `.h` on both `clang` and `cpp`; `.m` only on `objectivec`), R4
(Gradle three-way / CMake / CocoaPods / `clang` command shared on `objectivec`), R5 (entry-level

- shared-signal + contested-signal tests).

**Dependencies:** none.

**Files:**

- `src/engine/languages.rs` (slice and test module — `LANGUAGES` literal, all 21 new struct entries,
  all new test functions)

**Approach:**

- Append the 21 new struct literals to the existing `LANGUAGES` slice in alphabetical-by-token
  order: `bash`, `clang`, `clojure`, `cpp`, `csharp`, `dart`, `elixir`, `groovy`, `haskell`, `java`,
  `kotlin`, `lua`, `nix`, `objectivec`, `perl`, `php`, `ruby`, `scala`, `swift`, `terraform`, `zig`.
  The existing six entries keep their detection-order positions (R16 precedent). Each new struct
  mirrors the origin R1 table exactly.
- Tokens are lowercase ASCII, ≥3 characters, no English stop-words, no FTS5-special chars. The
  seed's pre-loaded special-character cases (`cpp`, `csharp`, `clang`, `objectivec`, `bash`) handled
  the high-risk tokens; the remaining 16 are unambiguously safe.
- Carry the `.h` shared-extension claim by listing `h` on both `clang` and `cpp` extension lists (R5
  multi-entry per origin R3). `objectivec` is not in the `.h` set; Obj-C patterns about C-family
  headers declare `language: [objectivec, clang]` or `[objectivec, cpp]` explicitly.
- Carry the `.m` single-owner claim by listing `m` only on `objectivec` (R5 single-owner per origin
  R3); MATLAB users hit R12's unknown-token-warn path indefinitely.
- Carry the three-way Gradle shared signals: `gradle` and `gradlew` command keywords plus
  `build.gradle` and `settings.gradle` markers list on `java`, `kotlin`, and `groovy`;
  `build.gradle.kts` and `settings.gradle.kts` markers list on `java` and `kotlin` only (Groovy does
  not use KTS).
- Carry the two-way CMake (`CMakeLists.txt` on `clang` and `cpp`), CocoaPods / Xcode (`Podfile`
  marker, `xcodebuild` command, `Pods` directory hint on `swift` and `objectivec`), and `clang`
  command keyword shared on both `clang` and `objectivec`.
- Drop the `.hcl` extension from `terraform` per origin Key Decisions (avoid R5 contestation with
  Packer, Vault, Consul, Boundary). Terraform extensions become `tf`, `tfvars`.
- Add tests in three blocks within `src/engine/languages.rs::tests`:
  1. **Entry-level signal tests** (one per new language). Idiom:
     `<token>_entry_has_expected_signals`. Each asserts the canonical token resolves, the display
     name matches, at least one extension is listed, and at least one marker (when the entry has
     one) is listed. Mirrors the existing `rust_entry_has_expected_signals` shape (line 211 in
     current `languages.rs`).
  2. **Shared-signal multi-membership tests** for the new sets:
     - `gradle_keyword_fires_for_java_kotlin_and_groovy` (three-way; pins AE3)
     - `gradlew_keyword_fires_for_java_kotlin_and_groovy`
     - `build_gradle_marker_fires_for_java_kotlin_and_groovy`
     - `settings_gradle_marker_fires_for_java_kotlin_and_groovy`
     - `build_gradle_kts_marker_fires_for_java_and_kotlin_only` (two-way; explicitly excludes Groovy
       per Groovy-does-not-use-KTS decision)
     - `settings_gradle_kts_marker_fires_for_java_and_kotlin_only`
     - `cmake_lists_marker_fires_for_clang_and_cpp` (pins R4 CMake two-way)
     - `h_extension_fires_for_clang_and_cpp` (pins R3 `.h` shared; covers AE1)
     - `xcodebuild_keyword_fires_for_swift_and_objectivec` (pins R4 CocoaPods/Xcode two-way)
     - `podfile_marker_fires_for_swift_and_objectivec` (covers AE5)
     - `pods_directory_hint_fires_for_swift_and_objectivec`
     - `clang_keyword_fires_for_clang_and_objectivec` (pins the R4 `clang` shared command keyword)
  3. **Contested-signal resolution tests**:
     - `m_extension_resolves_only_to_objectivec` (covers AE2; explicitly asserts `.m` is not in
       `clang`, `cpp`, or any other entry)
     - `h_extension_does_not_resolve_to_objectivec` (negative half of AE1; explicitly asserts Obj-C
       does not pick up `.h`)
- Extend the existing sweep tests in the same edit:
  - `is_known_token_accepts_canonical_tokens` (line 280) — add 21 new asserts, one per token
  - `display_name_for_resolves_known_tokens` (line 299) — add 21 new asserts mapping token → display
    name
- Reference the brainstorm's "no schema bump, no `LanguageEntry` shape change" boundary; this unit
  edits the slice's data and the test module only.

**Patterns to follow:**

- `src/engine/languages.rs:64-113` — existing six entries' struct shape
- `src/engine/languages.rs:211-218` — `rust_entry_has_expected_signals` (entry-level test idiom)
- `src/engine/languages.rs:232-250` — `npm_keyword_fires_for_both_javascript_and_typescript`,
  `package_json_marker_fires_for_both_javascript_and_typescript`,
  `node_modules_directory_hint_fires_for_both_javascript_and_typescript` (shared-signal idiom)
- `src/engine/languages.rs:253-262` — `cargo_toml_marker_is_case_sensitive`,
  `extension_lookup_is_case_insensitive` (contract pinning idiom)
- Test name discipline (verb + subject + condition snake_case): each test name accurately describes
  what it asserts — `gradle_keyword_fires_for_java_kotlin_and_groovy` is correct;
  `gradle_keyword_fires_for_all_jvm_languages` would be misleading because Scala is also JVM but not
  in the Gradle three-way set.

**Test scenarios:**

- **Entry-level (21 tests, one per new language):** Token registered in `is_known_token`,
  `display_name_for` resolves to expected name, at least one extension in `extensions` list, at
  least one command keyword (where applicable), at least one marker (where applicable). For the
  entries with directory hints (`swift`, `objectivec`, `terraform`), assert the hint is present.
  - Covers AE6 (elixir entry-level test asserts `.ex` resolves to `elixir`).
- **Shared-signal multi-membership (12 tests as listed above):** Each asserts the helper
  (`languages_for_*`) returns a `Vec` containing the expected language tokens; the order may vary
  but the set membership is what's pinned.
  - Covers AE1, AE3, AE5.
- **Contested-signal resolution (2 tests as listed above):** Explicitly assert both the positive
  membership and the negative non-membership (`.m` only resolves to `objectivec`, not `clang`,
  `cpp`, or any other).
  - Covers AE2.
- **Sweep extension (2 tests amended, 21 asserts added per test):**
  `is_known_token_accepts_canonical_tokens` and `display_name_for_resolves_known_tokens` extended
  with all 21 new tokens.

**Verification:**

- `cargo test -p lore --lib languages` — all entry-level, shared-signal, contested-signal, and sweep
  tests pass.
- `cargo build -p lore` — slice compiles cleanly; the increased entry count does not break the
  static-slice convention.
- Manual smoke is deferred to U4's verification (whole-PR smoke covers AE1–AE6 against a built
  binary).

---

### U2. Back-fill existing six entries + Node ecosystem shared-marker tests

**Goal:** Add the missing version-pin and lockfile markers to the existing six entries, close the
TypeScript `package-lock.json` oversight from PR #50, and pin the Node ecosystem shared-marker
extension with new multi-membership tests.

**Requirements:** R2 (full back-fill table), R4 (Node ecosystem shared-marker extension), R5
(parity-back-fill tests).

**Dependencies:** independent of U1; can land first if reviewers prefer that ordering, but
recommended to follow U1 so the slice file is touched twice in a contained sequence.

**Files:**

- `src/engine/languages.rs` (slice modifying existing entries' `marker_filenames` lists, plus new
  tests in the test module)

**Approach:**

- Modify the existing six entries' `marker_filenames` lists in place:

  | Token        | Markers added                                                                              | Notes                                                                                        |
  | ------------ | ------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------- |
  | `rust`       | `rust-toolchain.toml`, `rust-toolchain`                                                    | Both are rustup-imposed                                                                      |
  | `typescript` | `package-lock.json`, `.node-version`, `.nvmrc`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lockb` | `package-lock.json` is the PR #50 asymmetry fix; rest are R5 shared with `javascript`        |
  | `javascript` | `.node-version`, `.nvmrc`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lockb`                      | Five-marker set identical to TypeScript's additions; `package-lock.json` was already present |
  | `python`     | `.python-version`, `Pipfile.lock`, `poetry.lock`, `uv.lock`                                | Single-owner; covers pipenv, Poetry, uv                                                      |
  | `golang`     | `.go-version`, `go.work`                                                                   | `go.work` enables Go workspaces                                                              |
  | `yaml`       | (none)                                                                                     | No version-pin convention for YAML as a language                                             |

- All five new TypeScript / JavaScript markers list on both entries per R5 multi-entry — consistent
  with the pre-existing `package.json` + `node_modules` shape.
- Add tests in a new block within `src/engine/languages.rs::tests`:
  - `package_lock_json_marker_fires_for_both_javascript_and_typescript` (pins AE4 — the PR #50
    regression pin)
  - `node_version_marker_fires_for_both_javascript_and_typescript`
  - `nvmrc_marker_fires_for_both_javascript_and_typescript`
  - `yarn_lock_marker_fires_for_both_javascript_and_typescript`
  - `pnpm_lock_yaml_marker_fires_for_both_javascript_and_typescript`
  - `bun_lockb_marker_fires_for_both_javascript_and_typescript`
  - `python_version_marker_fires_for_python_only` (single-owner; explicitly asserts not in other
    entries)
  - `pipfile_lock_marker_fires_for_python_only`
  - `poetry_lock_marker_fires_for_python_only`
  - `uv_lock_marker_fires_for_python_only`
  - `go_version_marker_fires_for_golang_only`
  - `go_work_marker_fires_for_golang_only`
  - `rust_toolchain_toml_marker_fires_for_rust_only`
  - `rust_toolchain_marker_fires_for_rust_only`

**Patterns to follow:**

- `src/engine/languages.rs:64-113` — existing entries' marker_filenames list shape
- `src/engine/languages.rs:240-250` — `package_json_marker_fires_for_both_javascript_and_typescript`
  (the pre-existing two-way shared-marker test that the new ones parallel)
- `cargo_toml_marker_is_case_sensitive` style — single-owner negative half (explicitly empty for
  other tokens) when warranted

**Test scenarios:**

- **Two-way shared marker tests (6 new, all js/ts):** Each asserts `languages_for_marker_filename`
  returns a `Vec` containing both `javascript` and `typescript`. The AE4 test in particular pins the
  PR #50 regression — `package-lock.json` must resolve to both, not just `javascript`.
- **Single-owner marker tests (8 new, python/go/rust):** Each asserts the marker resolves only to
  the canonical owner; the negative half (other tokens not in the set) is implicit since the helper
  returns the full set.

**Verification:**

- `cargo test -p lore --lib languages` — all new shared-marker and single-owner tests pass.
- Manual smoke (deferred to U4) targets AE4 specifically: edit `package-lock.json` in a TS-project
  context, confirm a `language: typescript` pattern surfaces (whereas pre-fix it would only fire for
  `language: javascript`).

---

### U3. Refresh tests pinning `kotlin` as unknown / iterating the six-language list

**Goal:** Update every test in the crate that hardcoded `kotlin` as the unknown-token canary or that
iterated the original six canonical tokens, so the assertions remain coherent now that `kotlin` is a
known token.

**Requirements:** R5 (test surface — negative-test refresh and invariant test update).

**Dependencies:** U1 must land first so `kotlin` (and the other 20 new tokens) are actually
registered before existing negative assertions are rewritten.

**Files:**

- `src/engine/languages.rs` — rewrite `is_known_token_rejects_unknown_and_display_names` and
  `display_name_for_falls_back_to_raw_token_when_unknown`
- `src/status.rs` — rewrite `format_languages_line_unknown_token_falls_back_to_raw_token`
- `src/chunking.rs` — rewrite `parse_language_canonical_tokens_for_all_initial_six_languages` and
  any other test fixtures that hardcode `kotlin` as a canary

**Approach:**

- Pick `matlab` as the still-unknown canary token. Rationale: MATLAB is the deferred `.m`
  contestation owner (origin Key Decisions: `.m` to `objectivec` single-owner, MATLAB users hit the
  R12 unknown-token-warn path indefinitely); naming `matlab` as the unknown-canary keeps the test
  thematically connected to a known-deferred decision.
- In `src/engine/languages.rs::tests`:
  - `is_known_token_rejects_unknown_and_display_names` (current at line 290): replace
    `assert!(!is_known_token("kotlin"));` with `assert!(!is_known_token("matlab"));`. The other
    asserts (`rrust`, `Rust`, `Go`) are unchanged.
  - `display_name_for_falls_back_to_raw_token_when_unknown` (current at line 310): replace
    `assert_eq!(display_name_for("kotlin"), "kotlin");` with
    `assert_eq!(display_name_for("matlab"), "matlab");`. The empty-string assert is unchanged.
- In `src/status.rs::tests`:
  - `format_languages_line_unknown_token_falls_back_to_raw_token` (around line 124): replace any
    `kotlin` literal with `matlab` in both the input fixture and the expected-output check.
  - `format_languages_line_leaves_known_display_names_unchanged` (around line 146): extend the input
    fixture to include `cpp` and `csharp` alongside the existing tokens; assert the output contains
    the literal substrings `C# 1` and `C++ 1` (alphabetical: `C#` ASCII `0x23` sorts before `C+`
    ASCII `0x2B`, so `C#` comes first in the rendered list). This actually exercises the
    `+`/`#`-passthrough sanitiser contract the test was originally written to anticipate.
- In `src/chunking.rs::tests`:
  - `parse_language_canonical_tokens_for_all_initial_six_languages` (around line 2227): refactor to
    iterate `LANGUAGES.iter()` rather than hardcoding the six tokens — this future-proofs the test
    against subsequent expansions. Rename to `parse_language_canonical_tokens_for_all_languages`.
  - `parse_language_mixed_known_and_unknown` (at chunking.rs:2143): replace the `kotlin` canary in
    the fixture string (`language: [rust, kotlin]` → `language: [rust, matlab]`) and the two asserts
    (`langs == vec!["rust", "matlab"]`, `malformed[0].token == "matlab"`). The test's intent — one
    known + one unknown token, advisory captures the unknown — is preserved with `matlab` taking the
    canary role.
  - Any other `kotlin` literals in `chunking.rs` test fixtures: replace with `matlab` if the intent
    is "unknown token" (most likely), or with an existing known token if the intent is "known
    canonical". The grep verification step is the backstop for any further sites the plan did not
    enumerate.
- Grep verification: `grep -nR '"kotlin"' src/ tests/` after the unit lands; the only remaining hits
  should be the canonical `kotlin` entry in `LANGUAGES` and any test that specifically asserts
  behaviour for the `kotlin` token (entry-level test added in U1).

**Patterns to follow:**

- The hybrid test-module convention from PR #50 (entry-level tests in own block; sweep tests in own
  block; contract-pinning tests in own block). U3 only touches the contract-pinning and
  fixture-using tests.
- Test name discipline: `parse_language_canonical_tokens_for_all_initial_six_languages` →
  `parse_language_canonical_tokens_for_all_languages` matches the new iteration model
  (future-proofed name).

**Test scenarios:**

- The four updated tests pass with `matlab` in place of `kotlin`.
- The refactored `parse_language_canonical_tokens_for_all_languages` passes by iterating
  `LANGUAGES.iter().count()` (27 entries after U1 + U2) and asserting each token round-trips through
  the chunking frontmatter validator.
- Negative grep: `grep -nR 'kotlin' src/ tests/` returns only:
  1. The canonical `kotlin` entry in the `LANGUAGES` slice
  2. The `kotlin_entry_has_expected_signals` test added in U1
  3. Shared-signal tests that explicitly mention `kotlin` (Gradle three-way: `kotlin` is part of the
     asserted set)

**Verification:**

- `cargo test -p lore` — full suite passes (not just `--lib languages`); this unit touches three
  crates' worth of tests.
- `grep -nR '"kotlin"' src/ tests/ | grep -v languages.rs` returns no fixture-as-canary hits (only
  the canonical entry and test sites that legitimately reference the token).

---

### U4. CHANGELOG and ROADMAP update

**Goal:** Add the one-sentence CHANGELOG bullet under `[Unreleased]` `### Added` and move the
ROADMAP entry from `## Up Next` to `## Completed` in the same diff. Run manual smoke against
AE1–AE6.

**Requirements:** R6.

**Dependencies:** U1, U2, U3 must land first — the user-visible change must exist before the release
notes reflect it.

**Files:**

- `CHANGELOG.md`
- `ROADMAP.md`

**Approach:**

- **CHANGELOG.md.** Insert a new bullet under `[Unreleased]` `### Added` after the existing two
  entries (`#58` for the `lore status` Languages line, `#59` for trace logging). The exact text (per
  the origin brainstorm's R6 + Outstanding Questions draft):

  > `` `lore` detects 21 new languages: C, C++, C#, Swift, Kotlin, Shell, Objective-C, Scala, Elixir, Dart, Lua, Nix, Terraform, Haskell, Clojure, Zig, Perl, Ruby, Java, Groovy, and PHP. (#N) ``

  Substitute the actual PR number for `(#N)` at PR-open time. One assertive-voice sentence per
  `feedback_changelog_entries`. The parity back-fill is not separately surfaced — it's a fix to
  existing detection shape rather than a standalone new feature.

- **ROADMAP.md.** Two edits:
  1. Remove the current `- [ ] Extend the shared language table — ...` bullet from `## Up Next`
     (currently at lines 11–16 of `ROADMAP.md`).
  2. Add a new entry under `## Completed` matching the shape of the language-in-status (#58) and
     language-detection-architecture (#50) entries:

     ```
     - [x] Extend the shared language table — added 21 new entries (Ruby, Java, C/C++, C#, PHP,
           Swift, Kotlin, Shell, Objective-C, Scala, Elixir, Dart, Lua, Nix, Terraform, Haskell,
           Clojure, Zig, Perl, Groovy) and back-filled the existing six with missing version-pin
           markers and lockfiles, including the asymmetric `package-lock.json` on TypeScript that
           PR #50 left out. R5 contested signals resolved: `.h` shared between `clang` and `cpp`
           (R5 multi-entry), `.m` single-owner to `objectivec`. See
           `docs/plans/2026-05-18-001-feat-language-table-expansion-plan.md`.
     ```

- **Manual smoke.** After all four units land, run the manual smoke runbook before requesting
  review. The smoke procedure is the six Acceptance Examples:
  - AE1: Edit a `.h` file in a knowledge dir context, confirm pattern retrieval candidate set
    includes both `language: clang` and `language: cpp` patterns.
  - AE2: Edit a `.m` file, confirm only `language: objectivec` patterns surface.
  - AE3: Run `gradle build` in a Bash tool call **with no file_path in the CallContext** (pure
    command-keyword path, not conflated with marker/extension paths), confirm the inferred-language
    set is `{java, kotlin, groovy}` from the three-way command-keyword shared signal alone.
  - AE4: Edit `frontend/package-lock.json` in a TS-project context, confirm `language: typescript`
    patterns surface (post-fix; was missing pre-fix).
  - AE5: Edit `MyApp/Podfile`, confirm both `language: swift` and `language: objectivec` patterns
    surface.
  - AE6: Edit `lib/foo.ex`, confirm `language: elixir` patterns surface.
  - **Supplementary extension-path check** (not an AE; covers the bash entry's three-extension
    surface that no AE exercises directly): edit a `script.zsh` file, confirm `language: bash`
    patterns surface — verifies the multi-extension entry (`sh`/`bash`/`zsh` → `bash`) wires up
    end-to-end through the extension path.

**Patterns to follow:**

- `CHANGELOG.md` lines 13–22 — existing `[Unreleased]` `### Added` shape with `(#N)` PR reference
- `ROADMAP.md` lines 62–69 (language-in-status entry) and 77–85 (language-detection-architecture
  entry) — Completed-entry shape with plan-link suffix
- `feedback_changelog_entries.md` — one assertive-voice sentence, ending in `(#N)`, user-visible
  only
- `feedback_roadmap_update_in_feature_pr.md` — same-diff move, no separate "doc: roadmap tidy" PR
- `feedback_smoke_testing_discipline.md` — per-PR manual smoke is the standing discipline

**Test scenarios:**

Test expectation: none — this unit is mechanical documentation update plus per-PR manual smoke. The
unit's correctness is verified by the manual smoke runbook (AE1–AE6).

**Verification:**

- `git diff --name-only main` shows the five expected modified files (`src/engine/languages.rs`,
  `src/chunking.rs`, `src/status.rs`, `CHANGELOG.md`, `ROADMAP.md`).
- `cargo test -p lore` passes the full suite.
- Manual smoke executes AE1–AE6 with the built binary against a knowledge directory containing test
  patterns; each AE's expected detection set matches.

---

## Key Technical Decisions

- **No schema bump, no `LanguageEntry` shape change, no engine code change.** The slice is purely
  data plus tests plus release-notes. Origin Dependencies section is explicit: the helper APIs and
  schema-v4 `language_json` from PR #50 are stable consumed-as-is. Any pressure to reshape
  `LanguageEntry` mid-slice routes to a separate ticket under the schema-migration discipline
  (`docs/solutions/conventions/schema-migration-strategy-2026-05-14.md`).
- **Alphabetical-by-token ordering within new entries; existing six keep detection-order
  positions.** Reviewer-friendly without disturbing the R16 historical layout. Order has no
  detection consequence (`languages.rs` comment lines 17–20 is explicit on linear iteration with no
  semantic ordering).
- **Hybrid test module organisation.** Entry-level signal tests in one block (21 new), shared-signal
  multi-membership tests in one block (12 new for new-only sets in U1, 14 new for back-fill sets in
  U2), contested-signal resolution tests in one block (2 new). Matches the existing PR #50 module
  shape that already mixes these idioms.
- **`matlab` as the negative-token canary** in `is_known_token_rejects_unknown_and_display_names`
  and the analogous tests in `status.rs` and `chunking.rs`. MATLAB is the deferred `.m` contestation
  owner per origin Key Decisions; using `matlab` as the canary keeps the test thematically connected
  to a known deferral.
- **`chunking.rs` six-language test refactored to iterate `LANGUAGES`** rather than hardcoding six
  tokens. Future-proofs against drift on subsequent expansions; rename
  `parse_language_canonical_tokens_for_all_initial_six_languages` →
  `parse_language_canonical_tokens_for_all_languages`.
- **JS/TS shared-marker tests: extend the existing pattern + add fresh tests per new shared
  marker.** Per composition-cascade discipline
  (`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`),
  each three-way / two-way membership claim earns its own pinned assertion. AE4 (`package-lock.json`
  resolves to both `javascript` and `typescript`) is the PR #50 regression pin.
- **Single CHANGELOG bullet listing the 21 new languages; parity back-fill not separately
  surfaced.** The 21 new languages are the headline user-visible change; the parity back-fill is a
  fix to existing detection shape rather than a new standalone feature. Brainstorm-drafted text
  lands verbatim with `(#N)` substituted at PR-open time.
- **R5 multi-entry shared-signal listings: three-way Gradle (java/kotlin/groovy), two-way CMake
  (clang/cpp), two-way CocoaPods/Xcode (swift/objectivec), two-way `.h` (clang/cpp), shared `clang`
  command keyword on both `clang` and `objectivec`.** Gradle three-way is the first three-way set in
  the table; the existing `infer_languages` orchestrator (`src/engine/query.rs:121-150`) already
  accumulates multi-language sets via `extend_unique`, so three-way cardinality is supported with no
  engine change.
- **`.hcl` extension dropped from `terraform`.** Honours origin Key Decisions (avoid R5 contestation
  with Packer, Vault, Consul, Boundary). Non-Terraform `.hcl` edits fall through to R10's
  FTS-coincidence path. Broader HCL coverage is deferred to a future slice if traffic warrants.

---

## Dependencies / Assumptions

- The shared `LANGUAGES` slice, `LanguageEntry` struct, and helper APIs (`is_known_token`,
  `display_name_for`, `languages_for_extension`, `languages_for_command_keyword`,
  `languages_for_marker_filename`, `languages_for_directory_hint`) from PR #50 are stable; this
  slice consumes them unchanged.
- Architecture brainstorm R4 (FTS5-safe canonical tokens), R5 (signal-ownership:
  single-canonical-owner for contested signals plus multi-entry listing for shared signals), R6
  (three-test policy for markers / directory hints), R7 (marker > extension > directory hint
  priority chain), R10 (FTS-coincidence fallback for unlabelled patterns), and R12 (tier-2
  warn-and-proceed for unknown declared tokens) hold without modification — this slice consumes the
  architecture rather than altering it.
- Schema-v4 `language_json` column and the structural retrieval gate from PR #50 are unchanged; new
  tokens enter the existing query pipeline immediately on PR merge with no `lore ingest --force`
  required.
- `infer_languages` orchestrator in `src/engine/query.rs` already accumulates multi-language sets
  via `extend_unique`, so three-way Gradle (and any future N-way set) requires no engine code
  change.
- Per-PR manual smoke (`feedback_smoke_testing_discipline`) is the standing verification discipline;
  no CI smoke layer is added or assumed by this slice.
- CHANGELOG convention (`feedback_changelog_entries`): one assertive-voice sentence ending in
  `(#N)`, user-visible changes only.
- ROADMAP convention (`feedback_roadmap_update_in_feature_pr`): same-diff move from `## Up Next` to
  `## Completed`; no separate "doc: roadmap tidy" PR.

---

## Risks

- **Existing tests pinning `kotlin` as unknown break.** Mitigated by U3 (rewrite to use `matlab` in
  all four test sites: `languages.rs::is_known_token_rejects_*` and `display_name_for_falls_back_*`,
  `status.rs::format_languages_line_unknown_token_*`,
  `chunking.rs::parse_language_canonical_tokens_*`). Grep verification in U3 verifies no
  fixture-as-canary `kotlin` literals remain.
- **`chunking.rs:2227-2248` six-language invariant test breaks.** Mitigated by U3 (refactor to
  iterate `LANGUAGES.iter()` rather than hardcode the six tokens). The refactored test is
  future-proof against subsequent additions.
- **TypeScript pattern retrieval changes for existing tool calls hitting `package-lock.json`.**
  Pre-fix: only `language: javascript` patterns fire on `package-lock.json` edits in TS projects.
  Post-fix: both `language: javascript` and `language: typescript` patterns fire. Likely
  net-positive (closes PR #50 oversight) but is an observable behaviour change. Mitigated by U4's
  manual smoke targeting AE4 specifically: a `language: typescript` pattern in `lore-patterns/`
  whose body doesn't contain "typescript" should surface on `package-lock.json` edits post-fix and
  not pre-fix.
- **Three-way Gradle multi-entry might surface JVM patterns where pure-Kotlin or pure-Java contexts
  wouldn't want them.** Pre-fix: `gradle build` in a pure-Kotlin repo fires `{kotlin}` only (since
  Kotlin owned `gradle` keyword pre-this-slice — but actually pre-this-slice neither Java nor Kotlin
  nor Groovy was in `LANGUAGES`, so `gradle build` fired nothing). Post-fix: `gradle build` fires
  `{java, kotlin, groovy}`. Patterns about Java conventions in a pure-Kotlin repo are now in the
  candidate set — net-positive for cross-language patterns, possible noise for pattern authors who
  declared `language: kotlin` but didn't want Java-cross-talk. Accepted per origin AE3 and the
  Three-way Gradle Key Decision; no mitigation required beyond the AE3 smoke check.
- **Token-safety regression on one of the 16 non-special-character tokens.** Mitigated by inspection
  in U1's approach (every token is `[a-z0-9]+`, ≥3 chars, not in the 60-word stop list at
  `src/engine/query.rs:51-57`). The sweep test `is_known_token_accepts_canonical_tokens` would catch
  any token not in the slice; a token in the slice that failed FTS5-tokenisation would surface
  during U4's manual smoke (the smoke exercises the FTS-coincidence fallback path R10).
- **`chunking.rs` test fixtures other than the named six-language test mention `kotlin`.** Mitigated
  by grep verification in U3 (`grep -nR '"kotlin"' src/ tests/`). Any unexpected hits are reviewed
  and updated before declaring U3 done.

---

## Scope Boundaries

(Carried from origin; see
`docs/brainstorms/2026-05-18-language-table-expansion-requirements.md#scope-boundaries`.)

- Refactor of `LanguageEntry` shape — out of scope; the existing struct works.
- Changes to retrieval semantics, FTS5 schema, the `language_json` column, or the `language:`
  frontmatter validation logic — all settled in PR #50; not touched here.
- Other ecosystems deferred from this slice: F# (forces R5 multi-entry across the entire `dotnet`
  ecosystem for low marginal traffic), R (single-character token fails the 3-character minimum in
  `clean_terms`), MATLAB (no planned slice — `.m` is claimed for `objectivec`), broader HCL coverage
  (Packer, Vault, Consul, Boundary), Scala variants beyond mainstream Scala, Pascal / Ada / Fortran
  / COBOL, Crystal / Nim / Julia / Racket / Scheme, CoffeeScript, Dockerfile as a separate token,
  Bazel/BUILD as a separate ecosystem.
- Author-organisational directory hints (`src/main/java`, `app/`, `lib/`, `scripts/`) — fail R6
  (author-imposed rather than tool-imposed).
- Generic multi-ecosystem directories (`target/`, `vendor/`, `bin/`, `obj/`, `build/`, `dist/`) —
  fail R6 (no single canonical owner ecosystem).
- `.tool-versions` (asdf's universal version file) — fails R6 single-canonical-owner.
- Migration of the `lore-patterns` repository to use the new `language:` tokens — owner-driven
  authoring work, separate from the engine-side coverage shipped here.
- The `.t` extension for Perl — excluded from the `perl` entry because of historical ambiguity;
  `.pl`, `.pm`, `.pod` carry the Perl-dominant signal load.

### Deferred to Follow-Up Work

- A broader `hcl` entry covering Packer / Vault / Consul / Boundary — gated on traffic warranting
  it.
- Migration of `lore-patterns/` to declare `language:` for patterns about the new languages —
  authoring work, not engine work.

---

## Outstanding Questions

### Deferred to Implementation

- [Affects U3] [Technical] Exact list of `kotlin`-as-canary fixture sites in `chunking.rs`. U3's
  grep step (`grep -nR 'kotlin' src/`) enumerates them during implementation; the plan expects ≤3
  such fixture sites based on the research scan but does not commit to a count.
