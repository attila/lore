---
date: 2026-05-15
status: completed
type: feat
scope: lightweight
origin: docs/brainstorms/2026-05-15-language-in-status-requirements.md
---

# feat: Surface language coverage in `lore status`

## Summary

Add a per-language breakdown to the `lore status` output. After the existing `Sources:` line in
`cmd_status`, emit a `Languages:` line listing each declared language (rendered using its
`display_name` from the `LANGUAGES` table) with its source-count, plus a tail `undeclared` bucket.
Counts come from a new read-only aggregation against the existing `language_json` column shipped in
PR #50; no schema, no ingestion, and no gate behaviour change.

## Problem Frame

Operators have no CLI signal for how much of their knowledge base actually participates in the
structural language gate. PR #50 added `language:` frontmatter and the `language_json` storage
column, but `lore status` is silent on it — so there is no way to see, without poking the database
directly, whether the gate has data to work with or whether most patterns fall through to the
undeclared fallback.

See origin: `docs/brainstorms/2026-05-15-language-in-status-requirements.md`.

## Requirements

Carried forward from the origin brainstorm:

- **R1.** New `Languages:` line in `lore status`, positioned after `Sources:`.
- **R2.** Counts are over distinct **source files**, not chunks. A source contributes to each
  language it declares (`language_json` is a JSON array, so multi-language sources count in multiple
  buckets — the bucket sum can exceed `Sources:`). Sources with null `language_json` contribute only
  to `undeclared`.
- **R3.** Each declared language is rendered using its `display_name` from `LANGUAGES`
  (`src/engine/languages.rs`). Tokens not in the table fall back to the raw token so the operator
  still sees something meaningful.
- **R4.** Ordering: declared languages by count descending, alphabetical tiebreak on the rendered
  display name; `undeclared` always last.
- **R5.** Suppression: the entire `Languages:` line is suppressed when the database has no sources
  (consistent with the existing `db.stats()` guard). When every source is undeclared, the line still
  emits as `undeclared N`.
- **R6.** Writes to stderr alongside the other status lines.

## Key Technical Decisions

- **D1. New `db.language_counts()` method, not a `DBStats` field.** The current `DBStats` is a
  scalar-only struct (`chunks: usize`, `sources:
  usize`) asserted against in six unit tests;
  extending it with a `Vec` field bloats those assertions for callers that don't need the new data.
  A dedicated `language_counts()` keeps `stats()` lean, gives the new aggregate its own focused
  tests, and leaves room for a future `lore list --by-lang` consumer.
- **D2. Aggregation in SQL via `json_each`.** The codebase already uses `json_each(c.language_json)`
  in `search_fts_structural` and `structural_admits`. Reusing that pattern keeps the aggregation in
  one query rather than fetching `(source_file, language_json)` pairs and walking them in Rust. Two
  queries total: one `GROUP BY` against `json_each` for declared counts, one
  `COUNT(DISTINCT source_file) WHERE
  language_json IS NULL` for the undeclared bucket.
- **D3. Display-name resolution is a `Vec` scan over `LANGUAGES`, not a `HashMap`.** Six entries
  today, growing slowly. Linear scan is faster than hashing for a slice this small and keeps the
  call site dependency-free.
- **D4. No wrapping yet.** Worst-case rendered line with the current six languages plus `undeclared`
  is ~75 characters at three-digit counts — fits inside an 80-column terminal. Wrapping is deferred
  until the `LANGUAGES` table grows past the threshold; reopen this decision when the "Extend the
  shared language table" ROADMAP item lands.

## Implementation Units

### U1. Add `db.language_counts()` aggregation

- **Goal:** Expose a single read-only call that returns the declared per-language source counts and
  the undeclared source count.
- **Requirements:** R2, R3 (token surface), R4 (sort happens at the call site, but the data shape
  must support it), R5.
- **Dependencies:** none.
- **Files:**
  - `src/database.rs` — new `LanguageCounts` struct and `KnowledgeDB::language_counts()` method.
  - `src/database.rs` (test module at the bottom of the same file) — new unit tests alongside the
    existing `stats()` coverage.
- **Approach:**
  - New struct `LanguageCounts { declared: Vec<(String, usize)>,
    undeclared: usize }`.
    `declared` holds `(token, count)` pairs as returned by SQL — sorting and display-name resolution
    happen at the call site in U2.
  - Two SQL statements inside one method:
    - Declared:
      `SELECT je.value AS token, COUNT(DISTINCT c.source_file)
      FROM chunks c, json_each(c.language_json) je WHERE c.language_json IS
      NOT NULL GROUP BY je.value`.
    - Undeclared:
      `SELECT COUNT(DISTINCT source_file) FROM chunks WHERE
      language_json IS NULL`.
  - Returns `LanguageCounts { declared: [], undeclared: 0 }` on an empty database without erroring.
- **Patterns to follow:**
  - `KnowledgeDB::stats()` for method shape and error handling (`src/database.rs:664`).
  - `search_fts_structural` for the `json_each` join pattern (`src/database.rs:394`).
- **Test scenarios:**
  - Empty database — returns `declared` empty and `undeclared` 0.
  - Single-language sources only — declared count matches per-language source count; `undeclared`
    is 0.
  - All-undeclared sources only — `declared` is empty; `undeclared` equals distinct source count.
  - Multi-language source (`language: [rust, typescript]`) is counted once in each declared bucket;
    bucket sum exceeds `sources`.
  - Mixed: declared + undeclared sources in the same database produce the expected split.
  - Multiple chunks of the same source with the same declared language count the source once, not
    once per chunk (`COUNT(DISTINCT
    source_file)` correctness).
- **Verification:** `cargo test database::tests` passes including the new scenarios; manual
  inspection confirms the helper's behaviour matches the brainstorm's R2/R5.

### U2. Render `Languages:` line in `cmd_status`

- **Goal:** Emit the new line in the expected position with display-name resolution, R4 ordering,
  and R5 suppression.
- **Requirements:** R1, R3 (display-name resolution + unknown-token fallback), R4 (sort), R5
  (suppression rule), R6 (stderr).
- **Dependencies:** U1.
- **Files:**
  - `src/main.rs` — extend the `cmd_status` block that already calls `db.stats()`.
  - `src/engine/languages.rs` — small `pub fn display_name_for(token: &str)
    -> &str` helper that
    scans `LANGUAGES` and falls back to the input token. Lives next to the `LANGUAGES` slice so
    future consumers find it.
  - `tests/` — add a unit test for the formatting helper if extracted; see Approach.
- **Approach:**
  - Call `db.language_counts()` immediately after the existing `db.stats()` call (inside the same
    `if let Ok(...)` guard, so it shares the empty-database suppression).
  - Extract a small pure helper, e.g.
    `format_languages_line(counts:
    &LanguageCounts) -> Option<String>`, that:
    - Returns `None` when `declared` is empty AND `undeclared` is 0 (R5 suppression).
    - Resolves each declared token to its `display_name` via the new `display_name_for` helper.
    - Sorts declared entries by count descending, then alphabetical on display name (R4 tiebreak
      applies to the rendered name, not the token).
    - Joins as `"Display N"` segments with `", "` and appends `undeclared
      N` last when
      `undeclared > 0`.
  - `eprintln!("  Languages:    {}", line)` in `cmd_status` using the same label-column indent as
    the surrounding lines.
- **Patterns to follow:**
  - Existing `cmd_status` formatting (`src/main.rs:737-786`) for label alignment and stderr
    discipline.
  - `cli-output-conventions` — status output is diagnostic, stderr only.
- **Test scenarios:**
  - `format_languages_line` with empty counts returns `None`.
  - All declared, no undeclared: line ends with the last language, no trailing `undeclared` bucket.
  - All undeclared, no declared: line is exactly `undeclared N`.
  - Mixed declared + undeclared: ordering follows count desc, alphabetical tiebreak on display name,
    `undeclared` is last.
  - Tiebreak: two languages with equal counts render in alphabetical order of `display_name` (e.g.
    `Rust 5, TypeScript 5` — alphabetical wins because counts tie).
  - Display-name resolution: `rust` renders as `Rust`, `typescript` as `TypeScript`, `yaml` as
    `YAML`.
  - Unknown token fallback: a token absent from `LANGUAGES` renders as its raw form.
- **Verification:** `cargo test` passes including the new helper tests; `cargo run -- status`
  against the dogfood database emits the new line in the expected position and ordering.

## Scope Boundaries

- **In scope:** the new aggregation method, the new `cmd_status` line, the display-name helper, and
  tests for both units.
- **Out of scope (carried from origin):**
  - Listing the static supported-language table from `src/engine/languages.rs` in status output.
    Belongs in `--help` or documentation.
  - JSON output for `lore status`. The command is not structured today and a one-line followup is
    not the place to introduce it.
  - Any change to the language gate, schema, ingestion path, or `language_json` population logic.

### Deferred to Follow-Up Work

- Line wrapping for the `Languages:` value column. Deferred per D4 until the `LANGUAGES` table grows
  beyond what an 80-column line can comfortably hold. Pair with the "Extend the shared language
  table" ROADMAP item.

## System-Wide Impact

- **CLI surface:** one new stderr line in `lore status` output. No flag change, no exit-code change.
- **Database surface:** read-only access to the existing `language_json` column. No migration, no
  schema bump.
- **Public API surface:** new `LanguageCounts` struct and `KnowledgeDB::language_counts()` method
  exposed from `src/database.rs`. `display_name_for` is new public surface in
  `src/engine/languages.rs`. Both are additive; no existing callers change.

## Risks

- **Low — multi-language counting confusion.** Operators may expect the bucket sum to equal
  `Sources:` and be confused when it exceeds it. Mitigation: the brainstorm decided in favour of
  multi-bucket counting so the gate's actual coverage is visible. If complaints arrive, a brief note
  in `--help` or the README is the cheapest fix.

## Open Questions

None blocking. The wrap-width threshold is deferred per D4.
