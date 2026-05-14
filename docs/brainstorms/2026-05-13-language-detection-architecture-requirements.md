---
date: 2026-05-13
topic: language-detection-architecture
---

# Language Detection Architecture

## Summary

Restructure language detection around a single shared declarative table powering extension, command,
marker-filename, and directory-hint lookups, with a word-boundary bash matcher replacing the current
substring approach. Introduce an optional `language:` pattern frontmatter field as a structural
retrieval gate, falling back to today's FTS-token-coincidence behaviour when absent. The slice
delivers correctness fixes for the existing six languages and the structural primitive that lets
pattern authors retrieve without engineering their prose to contain canonical tokens.

---

## Problem Frame

Lore currently detects six languages on the file-extension side (Rust, TypeScript, JavaScript, YAML,
Python, Go) and three on the bash-command side (TypeScript via npm/npx/yarn/bun, Rust via cargo,
Python via pip/python). The two lookup paths live in separate `match` blocks with no shared source
of truth and have already drifted: Go and YAML have no bash signal; JavaScript and TypeScript are
conflated by toolchain overlap. Each new language requires editing both blocks and remembering they
exist.

The bash-command matcher uses `String::contains()` for matching, producing silent false positives:
`bundle install` matches `bun` (a TypeScript runtime); `pip` is a substring of `zipper` and
`gripper`. The bug rarely fires today because the covered set is narrow, but any future expansion of
the supported-language set makes it user-visible immediately.

The deeper retrieval issue is that the current FTS-token-coincidence approach makes pattern
retrieval depend on whether the pattern author happened to type the canonical token in prose. A
pattern about Rust coroutines that never literally says "rust" misses queries with a `rust` anchor.
For language-scoped patterns, language is a structural attribute, not an incidental word in the body
— but the current system treats it as the latter.

This slice ships three load-bearing wins for the existing six languages: a correctness fix for the
bash matcher's substring bug, a single source of truth for detection signals so the maps cannot
drift again, and the structural primitive (`language:` frontmatter) that lets pattern authors
retrieve correctly without keyword-stuffing. Expanding coverage to additional languages is
maintainer-incremental follow-on work using the new architecture, with no committed timeline or
contributor-pipeline assumption.

---

## Requirements

**Detection architecture**

- R1. A single shared declarative table is the source of truth for the language detection
  vocabulary. Each entry pairs one canonical language token with four signal lists: file extensions,
  command keywords, marker filenames, and directory hints. At least one of the four lists must be
  non-empty.
- R2. Command-side language inference matches whole tokens, not substrings — the command is split
  into shell-word-like tokens and each token is checked against the registered command-keyword set,
  so `bundle install` cannot match a registered `bun` keyword.
- R3. Adding a new language requires editing a single table entry in one file; all four lookup paths
  update simultaneously, and the data structure makes drift between them impossible by construction.
- R4. Canonical FTS5 tokens per language avoid English stop-words (e.g., `golang` instead of `go`)
  and FTS5-special characters (no `+`, `#`, `-`, or other FTS5-reserved punctuation in tokens).
- R5. Signal-ownership policy within entries: for genuinely contested signals (e.g., `.h` ambiguous
  between C and C++), the signal lives in a single canonical-owner entry, and pattern authors who
  need the alternative declare `language:` explicitly. For legitimately shared signals (e.g.,
  `.gradle.kts` for both Java and Kotlin), the signal appears in multiple entries, and detection
  accumulates the inferred languages as a set.
- R6. Marker filenames and directory hints qualify for inclusion only when all three of the
  following hold: (a) the name is tool-imposed, not author-chosen; (b) it has a single canonical
  owner ecosystem (or a small known multi-language set, e.g., Java/Kotlin for Gradle); (c) the
  contents serve that ecosystem's purposes (source, manifests, lockfiles, config). Generic build
  directories (`build/`, `dist/`, `bin/`), author-organisational choices (`src/`, `lib/`), and
  ambiguous multi-ecosystem names (`target/`, `vendor/`) do not qualify.
- R7. Detection signal priority within a file path: marker filename > extension > directory hint.
  The most-specific signal that fires determines the inferred language; lower-priority signals are
  not consulted once a higher-priority signal has produced a match.

**First-class language field**

- R8. A new optional `language:` frontmatter field declares the pattern's language as either a
  single canonical token (`language: rust`) or a YAML list of canonical tokens
  (`language: [javascript, typescript]`). Internally normalised to a list of tokens; the shared
  table is the only valid source for these tokens.
- R9. When a pattern declares `language:` and the query's inferred language is in the pattern's
  declared list, the pattern is eligible for retrieval regardless of whether the canonical token
  appears in the pattern body.
- R10. When a pattern does not declare `language:`, retrieval falls back to today's FTS-token
  behaviour: the pattern is eligible only if its body matches the FTS query `<lang> AND (terms)`. No
  existing pattern breaks.
- R11. When a tool call yields no inferred language at all, qualification reduces to terms-only FTS.
  Both labelled and unlabelled patterns are eligible; the structural label is not consulted because
  there is nothing to compare it against.
- R12. Ingest validates declared `language:` values against the shared table. Unknown tokens trigger
  a tier-2 warn-and-proceed: the pattern still ingests, a warning surfaces the unrecognised token
  through the existing per-pattern warning channel, and the structural retrieval path simply cannot
  match the unknown token (which falls through to the FTS fallback per R10). Warnings aggregate
  per-token (e.g., `Unknown language token rrust declared by 12 patterns`), not per-pattern, to keep
  ingest output legible at repo scale.
- R13. `lore ingest` emits a one-line tally indicating how many patterns declare `language:` vs fall
  back to FTS coincidence, surfacing migration progress without forcing it.

**Migration and compatibility**

- R14. The schema bump follows the established Universal-pattern migration precedent —
  backward-compatible upgrade with a friendly-advisory probe at startup if a pre-bump DB is
  detected, recommending `lore ingest --force` to rebuild.
- R15. The pattern authoring guide documents the `language:` field as encouraged (not required),
  covering: (a) the canonical token policy; (b) declaring a single token when the pattern is about
  one specific language; (c) declaring a multi-value list when the pattern's content applies to a
  small specific set of languages, even if the example uses one ecosystem (the list captures
  applicability, not provenance); (d) omitting the field when the pattern's content applies to too
  many languages or no clear-bounded set (retrieval falls back to FTS-coincidence on the pattern's
  terms); (e) how the existing `universal` tag / `applies_when` mechanism complements all three for
  cross-language always-inject patterns.

**Initial language coverage**

- R16. The shared table ships with the six currently-detected languages migrated into the new entry
  shape (Rust, TypeScript, JavaScript, YAML, Python, Go). Canonical tokens for these six match
  today's values: `rust`, `typescript`, `javascript`, `yaml`, `python`, `golang`. Initial
  marker-filename and directory-hint signals per language (all passing R6's three-test policy):
  - **Rust** — markers: `Cargo.toml`, `Cargo.lock`; directories: none qualifying (`target/` is
    ambiguous with Maven).
  - **TypeScript** — markers: `tsconfig.json`, plus shared with JavaScript: `package.json`;
    directories: `node_modules/` (shared with JavaScript via multi-entry listing).
  - **JavaScript** — markers: `package.json`, `package-lock.json`; directories: `node_modules/`
    (shared with TypeScript).
  - **YAML** — no marker filenames or directory hints qualify.
  - **Python** — markers: `pyproject.toml`, `requirements.txt`, `Pipfile`, `setup.py`; directories:
    `__pycache__/`, `.venv/`.
  - **Go** — markers: `go.mod`, `go.sum`; directories: none qualifying (`vendor/` is ambiguous
    across ecosystems).
- R17. No new languages are added in this slice. Expanding coverage to Ruby, Java, PHP, Kotlin,
  Swift, C-family, and any other ecosystem is maintainer-incremental follow-on work, sequenced after
  this slice lands and on a timing the maintainer chooses.

---

## Acceptance Examples

- AE1. **Covers R2.** Given a Bash tool call with command `bundle install`, when language inference
  runs, the matcher does not return `typescript` from a `bun` substring — whole-token matching
  tokenises the command and `bundle` is matched as `bundle`, not as containing `bun`.
- AE2. **Covers R7.** Given an Edit tool call with file path `node_modules/foo/Cargo.toml`, when
  signal inference runs, the result is `rust` — the marker-filename signal outranks the directory
  hint (`node_modules`, which would otherwise infer JS/TS). Note: this priority is intentional even
  when the file is inside a vendored-dependency tree. The agent is editing a Rust manifest; for that
  file the working context is Rust regardless of the surrounding project. Cross-context patterns
  (e.g., JS patterns the developer might also want) are not surfaced in this case — this is accepted
  as the trade for a simple priority rule.
- AE3. **Covers R9.** Given a pattern with `language: rust` in frontmatter and a tool call whose
  inferred language is `rust`, when retrieval runs, the pattern is eligible regardless of whether
  its body contains the word "rust".
- AE4. **Covers R9 (negative case).** Given a pattern with `language: rust` in frontmatter and a
  tool call whose inferred language is `python`, when retrieval runs, the pattern is not eligible —
  even if its body contains the word "python" — because the structural language gate excludes
  wrong-label patterns.
- AE5. **Covers R10.** Given a pattern with no `language:` field and a tool call with inferred
  language `rust`, when retrieval runs, the pattern is eligible only if its body matches the FTS
  query `rust AND (terms)` — today's behaviour preserved.
- AE6. **Covers R11.** Given a tool call with `mkdir build` and no file path, when the engine builds
  the query, no language anchor is included; both labelled and unlabelled patterns are eligible on
  terms alone.
- AE7. **Covers R12.** Given a pattern with `language: rrust` (typo) in frontmatter, when ingest
  runs, the pattern is still ingested but a warning is emitted naming the unrecognised token;
  subsequent retrieval treats the pattern as effectively unlabelled (FTS-coincidence path only). If
  twelve patterns share the same typo, the warning surfaces once with a count, not twelve times.
- AE8. **Covers R14.** Given a lore binary built with the new schema running against a database
  created by a **pre-v3** binary, when the binary starts, the established friendly advisory prints
  recommending `lore ingest --force` and the binary refuses to serve queries until the rebuild
  completes. (v3→v4 is silent in-place additive — no advisory fires for current users; this AE
  covers the legacy-version path only.)

---

## Success Criteria

- The existing six languages retrieve correctly under structural gating when patterns declare
  `language:`, and fall back to today's FTS behaviour when they do not — no regression for any
  current pattern in any consumer of lore.
- At least one concrete retrieval failure in the post-slice world is identified and resolved: a
  pattern that is structurally about one of the six languages but whose body lacks the canonical
  token, which today misses queries that should hit it, surfaces correctly after the slice when
  `language:` is declared.
- Adding a new language to the shared table is a single-tuple change in one file that updates
  extension, command-keyword, marker-filename, and directory-hint lookups simultaneously — no
  separate files, no duplicate edits, no drift risk.
- Pattern authors who declare `language:` rely on the retrieval gate; their pattern surfaces
  correctly without engineering the prose to contain the canonical token.
- `lore ingest` reports a coverage tally that makes the labelled-vs-fallback ratio visible at a
  glance.

---

## Scope Boundaries

- New language additions beyond the existing six. Out of scope for this slice;
  maintainer-incremental follow-on work using the new architecture.
- Migration of `lore-patterns` (or any other knowledge repository) to use the `language:` field.
  `lore-patterns` is an independent repository that may use the architecture to validate retrieval,
  but this slice does not depend on or migrate it; the architecture stands on its own.
- Runtime config-driven language table (user-supplied overrides). Deferrable; not a one-way door,
  can be added later without breaking changes.
- Synonym groups in query output (e.g., `(rust OR cargo)`). Deferred; layerable onto the shared
  table later if pattern-author UX reports justify it.
- Directory-as-fallback inference for _labelling_ (auto-deriving a pattern's `language:` from the
  directory it lives in). Rejected — couples lore behaviour to a specific repo layout. This is
  distinct from R6's directory-_hint_ signals, which are tool-imposed industry conventions, not
  repo-organisation choices.
- Mandatory `language:` field with friendly advisory. Rejected — "is this pattern language-scoped?"
  is a human judgement that cannot be cheaply automated.
- Content-sniffing (shebang, file header) for shell-script discrimination. Conflicts with the no-I/O
  contract of the query module.
- Walking up the path to find a manifest (e.g., "is there a `Cargo.toml` in some parent directory?")
  for ambient-language detection. Repo-inference, not per-path lookup; different design problem.
- `.gitignore` parsing to derive directory hints dynamically. Adds I/O and complexity for marginal
  gain over the static directory-hint list.
- Ranking boost for structural matches. The existing FTS column weights + vector similarity + RRF
  machinery handles ranking; structural admission is admission, not a thumb on the scale.
- Removing the FTS-coincidence fallback path. Premature for v0.x; can be deprecated later when
  `language:` adoption is high.

---

## Key Decisions

- **Chose `language:` as a structural retrieval gate** (rather than a tight match-block refactor
  with today's FTS semantics, or per-language FTS synonym groups). v0.x with a copy-the-author
  userbase makes the schema bump cheap; ingest-time downtime per upgrade is the trade — acceptable
  while the userbase is small, gets more expensive as adoption grows. Structural retrieval gates are
  the actual product differentiation. Synonym groups can be layered later if pattern-author UX
  reports justify it.
- **Chose optional `language:` field with FTS-coincidence fallback** (rather than mandatory
  declaration with a friendly advisory, or deriving the language from the pattern's directory).
  Mandatory cannot reliably distinguish language-scoped from agnostic patterns automatically;
  directory-derivation couples lore behaviour to a repo-layout convention that even sample knowledge
  repositories do not strictly follow.
- **Architecture-only scope for this slice.** Existing six languages migrated into the new shape;
  new languages are maintainer-incremental follow-on work. The architectural lever and the language
  additions ship as separate slices.
- **Bash matcher switches to word-boundary token matching.** Substring `.contains()` is already a
  latent bug (`bundle` contains `bun`, `pip` is substring of `zipper`); fixing it is non-negotiable
  for the architecture refactor.
- **Multi-value `language:` field accepted.** Single string or YAML list, both valid (matching the
  existing `tags:` precedent). Internal storage normalises to a list. Single ownership for genuinely
  contested signals; multi-entry listing for legitimately shared signals.
- **Tier-2 warn-and-proceed validation for declared language tokens.** Unknown values warn but
  accept, matching the CLI behaviour ladder convention. No binary-version coupling; typos remain
  debuggable; ingest is not blocked.
- **Three-test "obvious" policy for marker filenames and directory hints.** A signal qualifies when
  (a) tool-imposed, (b) single canonical owner ecosystem, (c) contents serve that ecosystem. Screens
  out generic build directories, ambiguous shared names, and author-organisational choices.
- **Signal priority within a file path: marker filename > extension > directory hint.**
  Most-specific wins; lower-priority signals are not consulted when a higher-priority signal
  matches. Vendored-dependency edge cases (Rust manifest inside a `node_modules/` tree) accept this
  priority as intentional rather than carving out a known-vendor-root rule.
- **No ranking boost for structural matches.** Structural admission gates entry to the candidate
  set; ranking is whatever FTS column weights + vector similarity + RRF produces. A separate boost
  would reward labelling regardless of content quality.
- **Runtime config-driven extension deferred.** Not a one-way door; can be added later without
  breaking changes. Keeps the data type internal for now.
- **Pattern authoring guide treats `language:` as encouraged, not required.** Forcing the judgement
  onto every author creates ceremony for the language-agnostic case, which has no reliable
  machine-decidable answer.

---

## Dependencies / Assumptions

- The Universal-pattern migration precedent (schema bump with backward-compatible startup advisory)
  is reusable here — same shape, same friendly-advisory pattern.
- The `universal` tag is in deprecation orbit, planned to be retired in favour of
  `applies_when`-only semantics. This feature is neutral to that consolidation: nothing in the
  design ties to `universal`'s continued existence, and the language-agnostic injection case is
  served by `applies_when` regardless of which tag survives.

---

## Outstanding Questions

### Deferred to Planning

- [Affects R14] [Technical] Exact schema migration shape (extra column on the patterns table vs a
  separate languages table) and how the friendly-advisory probe distinguishes schema versions. The
  Universal-pattern precedent describes two prior migration shapes (hard-bail with `--force`,
  in-place additive `ALTER TABLE`) and this slice's wording conflates them; planning picks one.
- [Affects R9, R10] [Technical] Query composition strategy — how the structural-gate path and the
  FTS-coincidence-fallback path compose in a single retrieval call (UNION, OR with CASE WHEN,
  ranking interaction with the existing FTS5 + vector + RRF machinery).
- [Affects R7] [Implementation note] Marker-filename and directory-hint extraction does not require
  new `CallContext` fields — existing `file_path` is sufficient via `Path::file_name()` and
  component iteration. Surfaced so planning does not add unnecessary surface area.
