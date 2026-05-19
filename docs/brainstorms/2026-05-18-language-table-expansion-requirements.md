---
date: 2026-05-18
topic: language-table-expansion
---

# Language Table Expansion

## Summary

Add 21 new `LanguageEntry` instances to the shared `LANGUAGES` table in `src/engine/languages.rs`
covering Ruby, Java, C, C++, C#, PHP, Swift, Kotlin, Shell, Objective-C, Scala, Elixir, Dart, Lua,
Nix, Terraform, Haskell, Clojure, Zig, Perl, and Groovy. Back-fill the existing six entries (Rust,
TypeScript, JavaScript, Python, Go, YAML) to parity granularity by adding missing version-pin
markers, lockfile coverage, and the asymmetric `package-lock.json` on TypeScript that PR #50 left
out. Ships in a single PR.

---

## Problem Frame

The shared `LANGUAGES` table introduced in PR #50 covers six languages today (Rust, TypeScript,
JavaScript, YAML, Python, Go). Outside that set, declared `language:` frontmatter triggers the R12
unknown-token-warn path, structural retrieval cannot gate on language for those patterns (per R10
they fall through to FTS-coincidence), and tool calls editing files in those ecosystems produce no
language signal at all.

The ROADMAP's `Extend the shared language table` entry names eight candidate languages (Ruby, Java,
C/C++, C#, PHP, Swift, Kotlin, shell scripts) as the next coverage tranche. The 2026-05-13 seed at
`docs/brainstorms/follow-on-language-pack-seed.md` pre-loaded canonical FTS5 tokens for nine
languages (covering the FTS5-safe special-character cases `cpp`, `csharp`, `clang`, and `bash`),
leaving Objective-C as an open question and deferring per-language marker/directory-hint decisions
to this brainstorm.

Two additional gaps surfaced during the brainstorm. First, the existing six entries are at coarser
marker granularity than the new entries naturally reach — TypeScript is missing `package-lock.json`
(an asymmetry left over from PR #50), and several entries are missing the `.<lang>-version` and
lockfile markers that are tool-imposed and single-canonical-owner per R6. Second, per-entry cost is
small enough (a struct literal plus 2-5 test lines and 3-5 minutes of smoke per language) that the
brief's eight-language framing is conservative for the carrying cost involved.

This expansion is gap-window capacity-building during the Track 2 trace-accumulation phase. The work
is mechanical and low-coordination — it extends the detection table now so that future
pattern-authoring passes have structural retrieval gating available for the new languages whenever
authors declare `language:` for them. No bleed-reduction claim rides on this slice: STATE.md's own
analysis names pattern-authoring debt (not engine primitives) as the dominant lever, and migration
of `lore-patterns` to declare the new tokens is explicitly out of scope here.

---

## Requirements

**New language entries**

- R1. Add 21 new `LanguageEntry` instances to the `LANGUAGES` slice. Each entry conforms to the
  architecture brainstorm's R4 token policy (FTS5-safe, no English stop-words, at least three
  characters) and architecture R6's three-test policy for marker filenames and directory hints
  (tool-imposed, single canonical owner ecosystem or small known multi-language set, contents serve
  that ecosystem). Order within the slice is not semantically significant per the existing comment
  in `languages.rs`; readable ordering is a planning decision.

  | Token        | Display     | Extensions                            | Command keywords                                        | Markers                                                                                 | Directory hints |
  | ------------ | ----------- | ------------------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------------- | --------------- |
  | `ruby`       | Ruby        | `rb`                                  | `ruby`, `bundle`, `rake`, `gem`                         | `Gemfile`, `Gemfile.lock`, `Rakefile`, `.ruby-version`                                  | —               |
  | `java`       | Java        | `java`                                | `java`, `javac`, `mvn`, `gradle`, `gradlew`             | `pom.xml`, `build.gradle`, `build.gradle.kts`, `settings.gradle`, `settings.gradle.kts` | —               |
  | `clang`      | C           | `c`, `h`                              | `clang`, `gcc`                                          | `CMakeLists.txt`                                                                        | —               |
  | `cpp`        | C++         | `cpp`, `cxx`, `cc`, `h`, `hpp`, `hxx` | `clang++`, `g++`                                        | `CMakeLists.txt`                                                                        | —               |
  | `csharp`     | C#          | `cs`, `csx`, `csproj`, `sln`          | `dotnet`                                                | `global.json`, `nuget.config`                                                           | —               |
  | `php`        | PHP         | `php`                                 | `php`, `composer`                                       | `composer.json`, `composer.lock`, `.php-version`                                        | —               |
  | `swift`      | Swift       | `swift`                               | `swift`, `xcodebuild`                                   | `Package.swift`, `Podfile`                                                              | `Pods`          |
  | `kotlin`     | Kotlin      | `kt`, `kts`                           | `kotlin`, `kotlinc`, `gradle`, `gradlew`                | `build.gradle.kts`, `settings.gradle.kts`, `build.gradle`, `settings.gradle`            | —               |
  | `bash`       | Shell       | `sh`, `bash`, `zsh`                   | `bash`, `sh`, `zsh`                                     | —                                                                                       | —               |
  | `objectivec` | Objective-C | `m`, `mm`                             | `pod`, `xcodebuild`, `clang`                            | `Podfile`                                                                               | `Pods`          |
  | `scala`      | Scala       | `scala`, `sc`                         | `scala`, `scalac`, `sbt`                                | `build.sbt`                                                                             | —               |
  | `elixir`     | Elixir      | `ex`, `exs`                           | `mix`, `iex`, `elixir`, `elixirc`                       | `mix.exs`, `mix.lock`                                                                   | —               |
  | `dart`       | Dart        | `dart`                                | `dart`, `flutter`, `pub`                                | `pubspec.yaml`, `pubspec.lock`                                                          | —               |
  | `lua`        | Lua         | `lua`, `rockspec`                     | `lua`, `luac`, `luarocks`                               | `.luarc.json`                                                                           | —               |
  | `nix`        | Nix         | `nix`                                 | `nix`, `nix-shell`, `nix-build`, `nix-env`              | `flake.nix`, `flake.lock`, `default.nix`, `shell.nix`                                   | —               |
  | `terraform`  | Terraform   | `tf`, `tfvars`                        | `terraform`, `tofu`, `tflint`                           | `terraform.tfvars`, `.terraform.lock.hcl`                                               | `.terraform`    |
  | `haskell`    | Haskell     | `hs`, `lhs`, `cabal`                  | `ghc`, `ghci`, `cabal`, `stack`, `runghc`, `runhaskell` | `stack.yaml`, `cabal.project`                                                           | —               |
  | `clojure`    | Clojure     | `clj`, `cljs`, `cljc`, `edn`          | `clj`, `clojure`, `lein`                                | `project.clj`, `deps.edn`                                                               | —               |
  | `zig`        | Zig         | `zig`, `zon`                          | `zig`                                                   | `build.zig`, `build.zig.zon`                                                            | —               |
  | `perl`       | Perl        | `pl`, `pm`, `pod`                     | `perl`, `cpan`, `cpanm`                                 | `cpanfile`, `Makefile.PL`                                                               | —               |
  | `groovy`     | Groovy      | `groovy`, `gvy`, `gradle`             | `groovy`, `groovyc`, `gradle`, `gradlew`                | `build.gradle`, `settings.gradle`                                                       | —               |

**Parity back-fill of existing entries**

- R2. Add the missing version-pin and lockfile markers to the existing six entries so the table
  reaches uniform granularity. The `package-lock.json` addition to `typescript` corrects an
  oversight from PR #50 — current behaviour silently misses TypeScript patterns when an agent edits
  a TypeScript repo's lockfile.

  | Token        | Markers added                                                                              | Notes                                                                                                                        |
  | ------------ | ------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
  | `rust`       | `rust-toolchain.toml`, `rust-toolchain`                                                    | `rust-toolchain` is the legacy plain-text form; both are rustup-imposed                                                      |
  | `typescript` | `package-lock.json`, `.node-version`, `.nvmrc`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lockb` | `package-lock.json` fixes the PR #50 asymmetry; lockfiles + version-pin shared with `javascript`                             |
  | `javascript` | `.node-version`, `.nvmrc`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lockb`                      | Identical five-marker set to TypeScript, all R5 multi-entry shared (TypeScript also gains `package-lock.json` per row above) |
  | `python`     | `.python-version`, `Pipfile.lock`, `poetry.lock`, `uv.lock`                                | Lockfiles cover pipenv, Poetry, and uv tooling                                                                               |
  | `golang`     | `.go-version`, `go.work`                                                                   | `go.work` enables Go workspaces                                                                                              |
  | `yaml`       | —                                                                                          | No version-pin convention exists for YAML as a language                                                                      |

**Contested-signal and shared-signal handling**

- R3. Resolve contested signals per the architecture brainstorm's R5. The `.h` extension is listed
  on both `clang` and `cpp` as a legitimately-shared signal (R5 multi-entry — modern C++ codebases
  overwhelmingly use `.h` for headers, so single-ownership would silently lose C++ pattern
  eligibility on every header edit); Objective-C patterns about headers declare `language:`
  explicitly alongside `clang` or `cpp` when retrieval gating on the Obj-C surface is required. The
  `.m` extension is owned by `objectivec` as a single-canonical-owner R5 claim; MATLAB patterns
  route through R12's unknown-token-warn path indefinitely (MATLAB is not a planned addition).

- R4. Shared signals (per the architecture brainstorm's R5 multi-entry rule) across the expanded
  table:
  - **Gradle ecosystem (three-way: java / kotlin / groovy).** Command keywords `gradle`, `gradlew`
    and markers `build.gradle`, `settings.gradle` list on all three entries. Markers
    `build.gradle.kts`, `settings.gradle.kts` list on `java` and `kotlin` (Groovy does not use KTS).
    This is the first three-way set in the table; the existing R5 detection-set accumulation already
    supports arbitrary cardinality.
  - **CocoaPods / Xcode (swift / objectivec).** Command keyword `xcodebuild`, marker `Podfile`, and
    directory hint `Pods` list on both entries.
  - **CMake (clang / cpp).** Marker `CMakeLists.txt` lists on both entries.
  - **C-family headers (clang / cpp).** Extension `.h` lists on both `clang` and `cpp`. Modern C++
    codebases (LLVM, Chromium, V8, large parts of Google's C++) overwhelmingly use `.h` for headers;
    single-ownership would silently lose C++ retrieval on every header edit.
  - **clang command keyword (clang / objectivec).** The `clang` command keyword lists on
    `objectivec` too because Obj-C is genuinely compiled with `clang` — a legitimately shared signal
    per R5, not a contestation.
  - **Node ecosystem (javascript / typescript).** Pre-existing two-way pattern extended with the
    back-filled markers (`package-lock.json` now on both, plus `.node-version`, `.nvmrc`,
    `yarn.lock`, `pnpm-lock.yaml`, `bun.lockb`).

**Test surface**

- R5. Extend the test coverage in `src/engine/languages.rs::tests`:
  - Add each of the 21 new tokens to `is_known_token_accepts_canonical_tokens` and
    `display_name_for_resolves_known_tokens`.
  - Rewrite the negative tests `is_known_token_rejects_unknown_and_display_names` and
    `display_name_for_falls_back_to_raw_token_when_unknown` to pin a still-unknown token (e.g.,
    `matlab`, since MATLAB is the deferred `.m` contestation owner).
  - Add one entry-level signal test per new language (parallel to `rust_entry_has_expected_signals`)
    pinning at least the canonical token, one extension, and any single marker the entry carries.
  - Add multi-membership tests for the new shared-signal sets: three-way for Gradle (`gradle`
    keyword and `build.gradle` marker resolve to `{java, kotlin, groovy}`); two-way for
    CocoaPods/Xcode (`Podfile` and `Pods` resolve to `{swift, objectivec}`); two-way for CMake
    (`CMakeLists.txt` resolves to `{clang, cpp}`).
  - Add contested-signal resolution tests: `.h` extension resolves only to `clang`; `.m` extension
    resolves only to `objectivec`.

**Delivery**

- R6. Ship as a single PR with the standard repository workflow (GPG-signed commits,
  `feat/<descriptive>` branch name, draft PR first, manual smoke before merge). The ROADMAP entry
  `Extend the shared language table` moves from `## Up Next` to `## Completed` in the same diff per
  `feedback_roadmap_update_in_feature_pr`. A single CHANGELOG bullet lands under `[Unreleased]`
  `### Added` in one assertive-voice sentence ending in `(#N)` per `feedback_changelog_entries` —
  this is a user-visible expansion of declared-language coverage.

---

## Acceptance Examples

- AE1. **Covers R3 + R4 (.h shared signal).** Given an Edit tool call with file path `src/foo.h`,
  when language inference runs, the result is the set `{clang, cpp}` — Objective-C is not in the set
  (Obj-C patterns about headers declare `language:` explicitly).
- AE2. **Covers R3 (.m ownership).** Given an Edit tool call with file path `Classes/Foo.m`, when
  language inference runs, the result is `objectivec` only.
- AE3. **Covers R4 (three-way Gradle).** Given a Bash tool call with command `gradle build`, when
  language inference runs, the result is the set `{java, kotlin, groovy}`.
- AE4. **Covers R4 + R2 (Node ecosystem parity).** Given an Edit tool call with file path
  `frontend/package-lock.json`, when language inference runs, the result is the set
  `{javascript, typescript}` — fixing the prior asymmetry where the marker fired only on
  `javascript`.
- AE5. **Covers R4 (CocoaPods shared).** Given an Edit tool call with file path `MyApp/Podfile`,
  when language inference runs, the result is the set `{swift, objectivec}`.
- AE6. **Covers R1 (entry-level signal).** Given an Edit tool call with file path `lib/foo.ex`, when
  language inference runs, the result is `elixir`.

---

## Success Criteria

- All 21 new `LanguageEntry` instances are added; their canonical tokens are recognised by
  `is_known_token`, and declared `language: <token>` frontmatter passes ingest validation without
  triggering an R12 unknown-token-warn.
- Patterns that declare `language:` for any of the 21 new tokens reach structural retrieval coverage
  — a pattern whose body lacks the canonical token surfaces correctly on matching tool calls.
- TypeScript pattern retrieval fires on `package-lock.json` edits (the PR #50 oversight is closed);
  analogous parity holds for the other back-fill markers.
- All six Acceptance Examples (AE1-AE6) pass in manual smoke before merge.
- The follow-on `ce-plan` consumes this document and produces a single-PR implementation plan
  without inventing token choices, signal lists, shared-signal listings, or test patterns.

---

## Scope Boundaries

- Refactor of `LanguageEntry` shape — out of scope; the existing struct works.
- Changes to retrieval semantics, FTS5 schema, the `language_json` column, or the `language:`
  frontmatter validation logic — all settled in PR #50; not touched here.
- Other ecosystems deferred from this slice and listed for context: F# (forces R5 multi-entry across
  the entire `dotnet` ecosystem for low marginal traffic); R (single-character token fails the
  3-character minimum in `clean_terms`, no clean `clang`-equivalent shorthand exists); MATLAB (no
  planned slice — `.m` is claimed for `objectivec`, MATLAB-pattern authors hit the R12
  unknown-token-warn path indefinitely); broader HCL coverage (Packer, Vault, Consul, Boundary —
  `.hcl` is dropped from the `terraform` entry rather than claimed multi-ecosystem; a future `hcl`
  entry could cover these if traffic warrants); Scala variants beyond mainstream Scala; Pascal, Ada,
  Fortran, COBOL; Crystal, Nim, Julia, Racket, Scheme; Coffee Script; Dockerfile as a separate
  token; Bazel/BUILD as a separate ecosystem.
- Author-organisational directory hints (`src/main/java`, `src/main/kotlin`, `app/`, `lib/`,
  `scripts/`) — fail R6 (a).
- Generic multi-ecosystem directories (`target/`, `vendor/`, `bin/`, `obj/`, `build/`, `dist/`) —
  fail R6 (b).
- `.tool-versions` (asdf's universal version file) — fails R6 (b) single-canonical-owner because it
  spans every ecosystem asdf supports.
- Migration of the `lore-patterns` repository to use the new `language:` tokens — owner-driven
  authoring work, separate from the engine-side coverage shipped here.
- The `.t` extension for Perl — excluded from the `perl` entry because of historical ambiguity (Roff
  troff, niche-language tests); `.pl`, `.pm`, `.pod` carry the Perl-dominant signal load.

---

## Key Decisions

- **FTS5-tokenisability audit collapses to during-PR test extension.** The seed's FTS5-safe
  canonical tokens (`cpp`, `csharp`, `clang`, `objectivec`, `bash`) already address the
  special-character risk the ROADMAP flagged. Command keywords, markers, and directory hints do not
  pass through FTS5 — they match against `Path::extension()`, `split_whitespace`-tokenised Bash,
  basenames, and path components respectively. The residual audit collapses to extending the
  existing `is_known_token` and `display_name_for` test pattern; no separate prep PR is earned.
- **Single PR for the full 21 new entries plus the parity back-fill.** Per-entry diff is one struct
  literal plus 2-5 test lines; smoke discipline (per-PR manual verification) scales with signal
  density rather than entry count, putting the total at roughly 30-45 minutes for the whole sweep.
  Multiple PRs would pay review and ROADMAP-tidy overhead without reducing smoke cost.
- **Maximal R6-clean expansion rather than the brief's eight-language framing.** Once per-entry cost
  is visibly small, the natural cut-off is R6-cleanliness plus token-safety plus non-trivial
  coding-agent presence, not a tight entry count. F#, R, and the long-tail niches are explicitly out
  for stated reasons rather than absent by default.
- **C and C++ ship as separate entries (`clang` and `cpp`), not a combined C-family entry.** Aligns
  with the seed's pre-loaded tokens and the architecture brainstorm's R5 framing of `.h` as a
  contested signal. Combining would lose the ability for a pattern to be declared "about C++ but not
  C" or vice versa.
- **Include Objective-C as the tenth original-batch entry; claim `.m` single-owner.** Coexists with
  Swift via R5 multi-entry on `Podfile`, `xcodebuild`, `Pods`, and lists `clang` as a shared command
  keyword (Obj-C is compiled with clang). MATLAB users are not a target population for lore in any
  planned slice; the `.m` single-owner claim makes Objective-C signal correctness the design
  priority and accepts the unknown-token-warn behaviour for any incidental MATLAB-pattern authors.
- **Three-way R5 multi-entry for the Gradle ecosystem (java / kotlin / groovy).** First three-way
  set in the table; the existing detection-set accumulation supports arbitrary cardinality, but the
  test surface gains one explicit three-way membership test asserting the cardinality is preserved.
- **`.h` extension shared between `clang` and `cpp` (R5 multi-entry).** Modern C++ codebases
  overwhelmingly use `.h` for headers (LLVM, Chromium, V8, large parts of Google's C++ style);
  single-ownership to `clang` would silently lose C++ retrieval on every `.h` edit. Objective-C is
  excluded from the `.h` set because Obj-C source files are typically `.m`/`.mm`; Obj-C patterns
  about C-family headers declare `language: [objectivec, clang]` or `[objectivec, cpp]` explicitly.
- **Terraform extensions narrowed to `.tf` and `.tfvars`; `.hcl` extension dropped.** HCL (HashiCorp
  Configuration Language) is used by Packer, Vault, Consul, and Boundary in addition to Terraform,
  so claiming `.hcl` exclusively for `terraform` would mis-classify non-Terraform HCL files. A
  broader `hcl` entry covering those ecosystems is deferred to a future slice if traffic warrants;
  for this slice, non-Terraform `.hcl` edits fall through to R10's FTS-coincidence path.
- **Display name `Shell` for token `bash`.** Coherent with the existing single-word display names
  (`Rust`, `Python`, `Go`, `JavaScript`). The seed's "shell scripts" reads as a language description
  rather than a display name. `bash` remains the canonical FTS5-safe token because it is the
  dominant shell in agent-coded scripts and is unambiguous.
- **C# uses extensions for project metadata (`csproj`, `sln`) rather than basename markers.**
  `.csproj` and `.sln` filenames are user-named (`MyProject.csproj`, `MyProject.sln`); only the
  extension is canonical. The existing `LanguageEntry` shape accommodates project-metadata
  extensions alongside source-file extensions without modification.
- **Terraform `.terraform` directory hint included; OpenTofu accommodated via `tofu` command
  keyword.** `.terraform` is `terraform init`'s provider cache, tool-imposed and single-owner.
  OpenTofu shares the entry as the canonical fork; both `terraform` and `tofu` commands resolve to
  the same entry.
- **Perl `.t` extension excluded.** Modern coding-agent usage of `.t` is dominantly Perl test files
  in the Test::More tradition, but the extension has historical ambiguity (Roff troff, niche
  languages). `.pl`, `.pm`, `.pod` carry the Perl-dominant signal load without that ambiguity.

---

## Dependencies / Assumptions

- The shared `LANGUAGES` table and its helper APIs (`is_known_token`, `display_name_for`,
  `languages_for_extension`, `languages_for_command_keyword`, `languages_for_marker_filename`,
  `languages_for_directory_hint`) shipped in PR #50 are stable; this slice adds entries to the slice
  without changing the struct shape or the helper surface.
- R4, R5, R6, R7, R10, and R12 from
  `docs/brainstorms/2026-05-13-language-detection-architecture-requirements.md` hold without
  modification — this slice consumes the architecture rather than altering it.
- The `feedback_changelog_entries`, `feedback_roadmap_update_in_feature_pr`, and
  `feedback_smoke_testing_discipline` memory rules apply to delivery: one user-facing CHANGELOG
  sentence, ROADMAP move in the same diff, manual smoke before merge.
- Per-PR manual smoke remains the standing verification discipline; no CI smoke layer (the parked
  three-tier-pyramid Tier 1) is added or assumed by this slice.

---

## Outstanding Questions

### Deferred to Planning

- [Affects R1] [Technical] Stable ordering of the 21 entries within the `LANGUAGES` slice.
  Alphabetical by canonical token, or grouped by ecosystem family? Order has no detection
  consequence per the existing `languages.rs` comment; readability for reviewers is the only
  tie-breaker.
- [Affects R5] [Technical] Test module organisation — one block of asserts per new entry (parallel
  to `rust_entry_has_expected_signals`), or grouped by signal type (one block for all extension
  tests, one for all command-keyword tests, etc.). The existing module mixes both styles; planning
  picks one for the new tests.
- [Affects R2] [Technical] Whether to extend the existing JS/TS multi-entry tests
  (`npm_keyword_fires_for_both_javascript_and_typescript`, `package_json_marker_fires_for_both_*`,
  `node_modules_directory_hint_fires_for_both_*`) to cover the newly back-filled shared markers, or
  add fresh tests. Either way is correct; planning picks.
- [Affects R6] [Mechanical] CHANGELOG draft text (per `feedback_changelog_entries`):
  `` `lore` detects 21 new languages: C, C++, C#, Swift, Kotlin, Shell, Objective-C, Scala, Elixir, Dart, Lua, Nix, Terraform, Haskell, Clojure, Zig, Perl, Ruby, Java, Groovy, and PHP. (#N) ``
  — planning lands this verbatim under `[Unreleased]` `### Added` with `(#N)` substituted at PR-open
  time.
