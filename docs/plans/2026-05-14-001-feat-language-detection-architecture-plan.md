---
date: 2026-05-14
topic: language-detection-architecture
type: feat
origin: docs/brainstorms/2026-05-13-language-detection-architecture-requirements.md
status: complete
completed: 2026-05-14
pr: https://github.com/attila/lore/pull/50
---

# feat: Language Detection Architecture

## Summary

Restructure language detection around a single shared declarative table powering four signal types
(extensions, command keywords, marker filenames, directory hints), with a word-boundary bash matcher
replacing the current substring approach. Introduce an optional `language:` pattern frontmatter
field as a structural retrieval gate. Retrieval composes as three independently-ranked candidate
lists fused by RRF: an FTS-fallback path for unlabelled patterns (today's `<lang> AND (terms)`
shape), an FTS-structural path for declared-language patterns (terms-only FTS gated by a JSON-column
membership check), and the existing vector-search path adjusted to oversample-and-filter so
structurally-mismatched patterns can't sneak through semantic similarity. Schema bump v3→v4 follows
the established Universal-pattern migration precedent. Seven implementation units; no new languages
added in this slice.

---

## Problem Frame

Lore currently detects six languages on the file-extension side and three on the bash-command side;
the two `match` blocks in `src/engine/query.rs` have drifted (Go and YAML have no bash signal). The
bash matcher uses `String::contains()` for matching, producing silent false positives that any
future expansion of the covered set makes user-visible. The current retrieval also makes patterns
dependent on whether the author typed the canonical language token in prose — a structural attribute
treated as an incidental word. This slice ships the architectural lever (shared table +
word-boundary matcher + structural retrieval gate) for the existing six languages without expanding
language coverage; expansion is maintainer-incremental follow-on work (see origin).

---

## Requirements Trace

All requirements from origin:
`docs/brainstorms/2026-05-13-language-detection-architecture-requirements.md`.

| R-ID                                                       | Covered by                | Notes                                                                                                  |
| ---------------------------------------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------ |
| R1 (shared declarative table)                              | U1                        | `const &[LanguageEntry]` literal; ~6 entries; linear iteration                                         |
| R2 (whole-token bash matching)                             | U1                        | `command.split_whitespace()` + token-set membership                                                    |
| R3 (single-tuple new-language PR)                          | U1                        | Demonstrable by adding a hypothetical entry in tests                                                   |
| R4 (FTS5-safe canonical tokens)                            | U1                        | Initial 6 already conform                                                                              |
| R5 (intra-entry signal ownership)                          | U1                        | Encoded by allowing signals to appear in multiple entries                                              |
| R6 (three-test "obvious" policy)                           | U1 (initial signals list) | Initial signals per language passed the policy in brainstorm                                           |
| R7 (signal priority: marker > ext > dir)                   | U1                        | First-signal-wins chain in `extract_query`                                                             |
| R8 (optional `language:` field)                            | U3                        | New parser; scalar-or-list shape                                                                       |
| R9 (structural gate when declared)                         | U5                        | FTS-structural ranked list via `json_each()` membership                                                |
| R10 (FTS fallback when absent)                             | U5                        | FTS-fallback ranked list preserves today's behaviour                                                   |
| R11 (no inferred language → terms-only)                    | U1, U5                    | When inferred-language set is empty, fallback collapses to terms-only and structural branch is skipped |
| R12 (tier-2 warn on unknown token + per-token aggregation) | U3, U4                    | `MalformedLanguageEntry` channel mirroring `MalformedPredicateEntry`                                   |
| R13 (ingest coverage tally)                                | U4                        | One-line tally at end of ingest                                                                        |
| R14 (schema bump with friendly advisory)                   | U2                        | Additive `ALTER TABLE`; mirrors v2→v3 (`applies_when_json`)                                            |
| R15 (authoring guide section)                              | U6                        | New `## Pattern language declaration` section                                                          |
| R16 (initial coverage: 6 languages migrated)               | U1                        | Table literal populated with the 6 with marker filenames / directory hints                             |
| R17 (no new languages this slice)                          | scope boundary            | Out of scope; follow-on                                                                                |

Acceptance Examples AE1–AE9 from origin map to test scenarios in U1, U2, U3, U5 as noted per-unit.

---

## High-Level Technical Design

The subsystem has three logical seams operating on three different modules. Each unit operates on
one seam at a time:

```
AUTHORING               INGEST                       RETRIEVAL
─────────               ──────                       ─────────
pattern.md                                                                            
  ┌────────────┐   parse_frontmatter_           upsert_pattern_in_tx
  │ ---        │     language_list                ┌──────────────────┐
  │ language:  │ ──►   (chunking.rs)  ──────►     │ patterns +       │
  │   rust     │       (U3)            (U4)       │ chunks tables    │ ──┐
  │ ---        │                                  │ language_json    │   │
  └────────────┘                                  │ TEXT NULL        │   │
                                                  └──────────────────┘   │
                                                     (U2, schema v4)     │
                                                                         │
                        DETECTION (tool call →)                          │
                        inferred languages: Vec<String>                  │
                                                                         ▼
                                                ┌────────────────────────┐
                                                │ Three ranked lists →   │
                                                │ RRF fusion             │
                                                │                        │
                                                │ • FTS-fallback:        │
                                                │   MATCH "<lang> AND    │
                                                │   (terms)"             │
                                                │   + language_json      │
                                                │     IS NULL            │
                                                │                        │
                                                │ • FTS-structural:      │
                                                │   MATCH "(terms)"      │
                                                │   + EXISTS json_each   │
                                                │     WHERE value IN     │
                                                │     (?inferred...)     │
                                                │                        │
                                                │ • Vector:              │
                                                │   oversample N*k by    │
                                                │   similarity, filter   │
                                                │   to NULL OR member,   │
                                                │   take top k           │
                                                └────────────────────────┘
                                                     (U5, retrieval gate)
```

**Detection (U1)** is its own seam, independent of the schema bump and retrieval changes. It
produces a set of inferred language tokens (`Vec<String>` — may be empty, singular, or multi-valued
when shared signals fire) from a tool call's file path or bash command using the shared table.
Pure-refactor of `src/engine/query.rs` — no DB changes.

**Storage (U2)** is the additive schema bump: one `language_json TEXT NULL` column on `patterns` and
on `chunks`, populated by the ingest path, consumed by the retrieval gate. JSON column (not separate
join table) matches the existing `applies_when_json` precedent.

**Authoring path (U3–U4)** is the new frontmatter parser plus the ingest plumbing that writes the
parsed value into the new column, with per-token aggregated warnings for unknown tokens.

**Retrieval (U5)** composes as three independent ranked candidate lists fed to RRF:

1. **FTS-fallback** — `MATCH "<lang> AND (terms)"` with `WHERE language_json IS NULL`. For patterns
   that haven't declared a language: today's body-anchor behaviour preserved.
2. **FTS-structural** — `MATCH "(terms)"` with
   `WHERE EXISTS (SELECT 1 FROM
   json_each(c.language_json) WHERE value IN (?inferred_langs...))`.
   For patterns that declared a language matching the inferred set: body-anchor requirement waived,
   set intersection between inferred and declared admits the pattern.
3. **Vector** — `vec0 MATCH ?embedding AND k = N*?top_k ORDER BY distance` returns `N`-times-`k`
   nearest neighbours; result set is filtered by the same `IS NULL OR EXISTS
   json_each`
   predicate, then truncated to `top_k`. Preserves AE4 (wrong-language structural mismatch can't
   sneak in via semantic similarity).

The two FTS branches are disjoint by their `language_json` predicates (one requires NULL, the other
requires membership) — no pattern double-counts. RRF takes positional rank from each list and merges
with reciprocal-rank scoring; each list's internal BM25 weighting is internally consistent because
each branch uses its own MATCH terms.

When the inferred-language set is empty (e.g., `mkdir build` with no file path), the FTS-fallback
collapses to `MATCH "(terms)"` only, the FTS-structural branch is skipped entirely (nothing to gate
against), and the vector path returns top-k by similarity without the IS-NULL-OR-member filter —
terms-only retrieval, both labelled and unlabelled patterns eligible per R11.

This illustrates the intended approach and is directional guidance for review, not implementation
specification.

---

## Output Structure

No new directories. All changes land in existing or new files within existing modules:

- `src/engine/query.rs` — refactored detection (U1)
- `src/engine/languages.rs` _(new)_ — shared declarative table (U1)
- `src/engine/mod.rs` — re-export updates (U1, U5)
- `src/database.rs` — schema bump and retrieval (U2, U5)
- `src/chunking.rs` — frontmatter parsing and validation (U3)
- `src/ingest.rs` and CLI command — plumbing + tally (U4)
- `src/hook.rs` — search_hybrid invocation site and public extract_query shim (U5)
- `src/server.rs` — MCP search_patterns language extraction and parameter plumbing (U5)
- `docs/pattern-authoring-guide.md` — new section (U6)
- `docs/search-mechanics.md` — retrieval pipeline section updated (U7)
- `docs/hook-pipeline-reference.md` — structural gate note (U7)
- `docs/architecture.md` — engine module layout update (U7)
- `docs/configuration.md` — `language:` field documented (U7)
- `CHANGELOG.md` — feature entry at PR-merge (U7)
- `docs/solutions/conventions/schema-migration-strategy-2026-05-14.md` _(new)_ — captures the
  derived-artefact principle, the two operational strategies (silent additive vs hard-bail), and the
  code/UX/testing contracts for future schema bumps (U7)

---

## Implementation Units

### U1. Shared language table and word-boundary bash matcher

**Goal.** Replace the two existing `match` blocks in `src/engine/query.rs` with iteration over a
shared declarative table; add marker-filename and directory-hint signal types; fix the bash
matcher's substring bug by switching to whole-token matching.

**Requirements.** R1, R2, R3, R4, R5, R6, R7, R11 (no-inferred-language branch unchanged), R16
(initial coverage). Covers AE1, AE2.

**Dependencies.** None — pure detection refactor, no DB changes.

**Files.**

- `src/engine/languages.rs` _(new)_ — shared table module with `LanguageEntry` struct and
  `static LANGUAGES: &[LanguageEntry]` literal
- `src/engine/query.rs` — refactor `language_from_extension` and `language_from_bash` to iterate the
  table; add `language_from_marker_filename` and `language_from_directory_hint`; update
  `extract_query` to chain signals by priority and return inferred-language set
- `src/engine/mod.rs` — expose the new `languages` module
- `src/engine/query.rs` test module — new and updated unit tests
- `tests/edge_cases.rs` — integration test for `bundle install` regression

**Approach.**

The shared table is a `const &[LanguageEntry]` literal — no `OnceLock`, no `phf_map`, no
`lazy_static`. Six entries today, low double digits long-term; linear iteration is appropriate. Each
entry pairs a canonical FTS-safe token with four `&'static [&'static str]` signal lists:

```rust
// directional sketch — not the final shape
pub struct LanguageEntry {
    pub token: &'static str,        // canonical FTS5 token: "rust", "golang"
    pub display_name: &'static str, // human-readable: "Rust", "Go", "TypeScript"
    pub extensions: &'static [&'static str],
    pub command_keywords: &'static [&'static str],
    pub marker_filenames: &'static [&'static str],
    pub directory_hints: &'static [&'static str],
}
```

The `display_name` field exists to support human-facing surfaces: the authoring guide table (U6),
future CLI subcommands that list supported languages, and any error or help message that names
languages in prose. The token-vs-display asymmetry is real for `golang` (display: "Go" — the FTS
token avoids the English stop-word) and will grow when the follow-on language pack lands `cpp`→C++,
`csharp`→C#, and `clang`→C. Establishing the field now keeps the data colocated with the rest of the
entry rather than scattered across a separate static mapping.

Initial table (per origin R16, all signals pass R6's three-test policy):

| Token        | Display    | Extensions | Command keywords          | Marker filenames                                    | Directory hints    |
| ------------ | ---------- | ---------- | ------------------------- | --------------------------------------------------- | ------------------ |
| `rust`       | Rust       | rs         | cargo                     | Cargo.toml, Cargo.lock                              | —                  |
| `typescript` | TypeScript | ts, tsx    | npm, npx, yarn, bun, pnpm | tsconfig.json, package.json                         | node_modules       |
| `javascript` | JavaScript | js, jsx    | npm, npx, yarn, bun, pnpm | package.json, package-lock.json                     | node_modules       |
| `yaml`       | YAML       | yml, yaml  | —                         | —                                                   | —                  |
| `python`     | Python     | py         | pip, python, python3      | pyproject.toml, requirements.txt, Pipfile, setup.py | **pycache**, .venv |
| `golang`     | Go         | go         | go                        | go.mod, go.sum                                      | —                  |

The `package.json` marker, the `npm`/`yarn`/`bun`/`pnpm`/`npx` command keywords, and the
`node_modules` directory hint appear in both `javascript` and `typescript` entries (legitimately
shared per R5); detection accumulates these as a set when matches fire.

Bash tokenisation: `command.split_whitespace()` over the lowercased command, then filter out
`KEY=VAL`-shaped tokens (env-prefix handling: split on `=`, check left side matches
`[A-Z_][A-Z0-9_]*`), then check each surviving token against the command-keyword set, accumulating
language tokens from every entry whose keyword is present. Naturally handles
`env FOO=bar cargo build` (yields `["env", "cargo", "build"]` after filtering, `cargo` matches
Rust). Avoids `bundle install` matching `bun` because tokenisation is whole-token, not substring.
`npm test` accumulates `{javascript, typescript}` because both entries register `npm`.

Signal priority order in `extract_query`: marker filename > extension > directory hint within a file
path; bash signals contribute independently when present. First-priority signal that produces a
match wins for that file path; lower-priority signals are not consulted. When file-path and bash
signals both fire, their inferred-language sets union.

**`extract_query` return shape.** Returns `(Vec<String>, Vec<String>)` — the first slot is the set
of inferred languages (empty when none, singular when one match, multi-valued when shared signals
fire), the second slot is the cleaned enrichment terms. The caller (hook adapter, MCP server) plumbs
both through to retrieval.

**Patterns to follow.**

- `STOP_WORDS` const at `src/engine/query.rs:35-41` for the static-slice convention.
- `command_matches_with_wrappers` at `src/engine/predicate.rs:135` for whole-token shell matching
  via `split_whitespace`. Note: do NOT reuse the wrapper-stripping head-only logic —
  `language_from_bash` wants any-token-in-set matching, not effective-command-head matching.
- Existing first-signal-wins shape at `src/engine/query.rs:65-69` for `if language.is_none()`
  guarding.

**Test scenarios.**

- Each of the six canonical tokens detects correctly from its primary extension (six parametrised
  cases). **Covers AE2** partially (the extension fallback half).
- Each registered command keyword matches when present as a whole token (e.g., `cargo`,
  `cargo build`, `cargo test --lib` all detect rust).
- `bundle install` does NOT detect as `typescript` — regression test for the substring bug. **Covers
  AE1.**
- `env FOO=bar cargo build` detects as `{rust}` (env-prefix tokens filtered out, `cargo` matched).
- `pip install` and `python script.py` both detect as `{python}`.
- `npm test` detects as `{javascript, typescript}` — multi-language accumulation via shared command
  keyword. **Covers AE9.**
- Marker filename `Cargo.toml` detects as `{rust}` regardless of containing directory.
- Marker filename `package.json` detects as `{javascript, typescript}` (multi-set).
- Directory hint `node_modules/anything.js` (no marker, no recognisable extension on `anything`)
  detects as `{javascript, typescript}`.
- Marker filename outranks extension: `node_modules/foo/Cargo.toml` detects as `{rust}`, not as
  JS/TS. **Covers AE2.**
- Edit on `README.md` with no path-recognisable signal and no bash → empty inferred set;
  `extract_query` returns the terms-only path. **Covers AE6.**
- Shared-signal entry (`package.json`, `node_modules`) accumulates languages without duplicating
  queries.
- Adding a new entry to the table is a single struct literal (verify by adding a fake test-only
  entry and asserting all four lookups light up — proves R3).

**Verification.**

- All existing `engine/query.rs` tests pass after the refactor (return-type change rippled through
  assertions).
- `cargo test --lib engine::query` covers the new test cases.
- The new bash-matcher test asserts no false positive for `bundle install`.

---

### U2. Schema bump v3→v4 — additive `language_json` column

**Goal.** Add `language_json TEXT NULL` to both `patterns` and `chunks` tables. Backward-compatible
migration mirrors the v2→v3 (`applies_when_json`) shape: in-place additive `ALTER TABLE` with
`column_exists` idempotency, friendly-advisory probe for pre-v3 databases (which already fall
through to the hard-bail path), and the established remedy-completion test pattern.

**Requirements.** R14. Covers AE8.

**Dependencies.** None (schema work is independent of detection refactor).

**Files.**

- `src/database.rs` — bump `SCHEMA_VERSION` to 4; append `language_json TEXT` to `CHUNKS_DDL`
  (line 640) and `PATTERNS_DDL` (line 678); add `if version == 3 { ... }` migration branch mirroring
  the existing v2→v3 block (lines 777-805); update `check_schema_compatibility` and history comments
- `src/database.rs` tests — new migration test for v3→v4 mirroring the v2→v3 test fixtures at lines
  2336-2475

**Approach.**

Mirror the v2→v3 additive migration block at `database.rs:777-805` exactly:

1. Inside the existing version-dispatch ladder, add `if version == 3 { ... }`.
2. Use `column_exists()` (line 732) to guard each `ALTER TABLE` for idempotency.
3. Wrap in `BEGIN IMMEDIATE` / `COMMIT` via `execute_batch`.
4. `PRAGMA user_version = 4` at the end of the batch.

DDL constants (`CHUNKS_DDL`, `PATTERNS_DDL`) get one additional column line: `language_json TEXT` —
nullable, no default (NULL means "no `language:` declared"). History comments at lines 626-633 get a
new entry documenting v3→v4.

The hard-bail branch at lines 819-825 does not need a new arm — pre-v3 databases (very old) still
fall through to the bail-with-`--force` path. New v3 databases hit the additive branch.

**Migration UX paths.** The two paths produce materially different user-visible behaviour; an
implementer should preserve this asymmetry explicitly:

- **v3 → v4 (silent in-place additive).** The common path — every current user with a working
  install is at v3. The additive `ALTER TABLE` runs at binary startup, takes milliseconds, prints
  nothing. Existing patterns continue working via the FTS-fallback path per R10 because their
  `language_json` is NULL. Zero downtime, zero user action. No advisory fires.
- **pre-v3 → v4 (hard-bail with friendly advisory).** Legacy users only. The existing hard-bail
  branch at lines 819-825 fires unchanged — the established advisory wording prints, binary refuses
  to serve queries until `lore ingest --force` rebuilds. AE8 describes this path specifically; v3
  users do NOT see this message.

**Composition-cascade audit** (per
`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`):

- `init` and `clear_all` share `CHUNKS_DDL` and `PATTERNS_DDL` — adding the column to the DDL
  constants automatically propagates. Verify.
- `clear_all` must DROP+CREATE (not DELETE FROM), per the prior retrospective. Verify the current
  `clear_all` uses the right shape; if not, fix.
- `open_skipping_schema_check` variant for the `--force` remedy path — verify it exists and is
  reachable from `cmd_ingest`. If not, add it.
- `should_skip_schema_probe(force, file)` helper with truth-table unit tests — already exists per
  Universal-pattern retro; verify still correct after v4.

**Patterns to follow.**

- v2→v3 migration block at `src/database.rs:777-805`.
- v2→v3 test fixtures at `src/database.rs:2336-2475` (full migration), 2508-2557 (column
  projection), 2700-2780 (without `patterns` table).
- Friendly-advisory wording at `src/database.rs:819-825`.
- `docs/solutions/best-practices/compatibility-check-advisory-must-verify-remedy-is-reachable-2026-04-21.md`
  for the three composition rules (shared DDL, `open_skipping_schema_check`,
  `should_skip_schema_probe`).

**Test scenarios.**

- Fresh DB initialises with v4 schema — `language_json` columns present on both tables.
- Hand-built v3 DB fixture (mirroring `database.rs:2336-2475`): the in-place migration runs, both
  columns appear, `PRAGMA user_version` returns 4.
- Hand-built v2 DB fixture: hard-bail path fires with the existing advisory wording (regression test
  that we didn't break the older-version path).
- Hand-built v1 DB fixture: hard-bail path fires.
- v4→v4 is a no-op (idempotent — `column_exists` returns true, ALTER TABLE skipped).
- Remedy-completion integration test using `Command::cargo_bin("lore")` against a raw-SQL v3
  fixture: assert probe fires with the expected advisory, run `lore ingest --force`, assert the
  probe accepts the new state. **Covers AE8.**
- `clear_all` after the bump leaves the `language_json` columns intact (verifies the DROP+CREATE
  path picks up the new DDL).

**Verification.**

- `cargo test database::tests` includes the new v3→v4 migration test and the remedy-completion test.
- Manually verify `PRAGMA user_version` returns 4 on a fresh init.

---

### U3. Frontmatter `language:` parsing and tier-2 validation

**Goal.** Parse the new optional `language:` frontmatter field (scalar or list form, mirroring the
existing `tags:` shape) and validate each token against the shared table. Unknown tokens trigger a
tier-2 warn-and-proceed: pattern still ingests, warning surfaces via the existing per-pattern
warning channel, aggregated per-token (not per-pattern) to keep ingest output legible at repo scale.

**Requirements.** R8, R12. Covers AE7.

**Dependencies.** U1 (needs the shared table to validate tokens against).

**Files.**

- `src/chunking.rs` — new `parse_frontmatter_language_list` function modelled on
  `parse_frontmatter_tag_list` (line 400); new `MalformedLanguageEntry` struct mirroring
  `MalformedPredicateEntry` (lines 45-55); update `pattern_row_from` (line 305) to call the parser
  and surface entries
- `src/chunking.rs` test module — unit tests for the new parser
- `src/engine/languages.rs` — expose a `is_known_token(token: &str) -> bool` helper for the
  validator

**Approach.**

`parse_frontmatter_language_list(content: &str) -> (Vec<String>, Vec<MalformedLanguageEntry>)`
returns the parsed token list plus any malformed/unknown entries for the warning channel. Both forms
supported per R8:

- Scalar: `language: rust` → `["rust"]`
- List: `language: [javascript, typescript]` → `["javascript", "typescript"]`
- Block list: `language:\n  - rust\n  - golang` → `["rust", "golang"]`
- Missing: returns empty `Vec`, no entries

Normalisation: lowercase each token at parse time. Whitespace trimmed; `strip_outer_quotes` applied
per the existing helper at `src/chunking.rs:770`.

Validation: each parsed token is checked against `LANGUAGES` via the new `is_known_token()` helper.
Unknown tokens are stored in the entry verbatim (so the warning can show the user's exact typo) and
added to the `MalformedLanguageEntry` list, not discarded outright. The token still lands in the
column — the structural retrieval path simply won't match it (no entry in the shared table → no
inferred language can equal it). Per R10, the pattern falls back to FTS coincidence for retrieval.

Warning aggregation happens at the ingest level (U4), not in the parser. The parser returns
per-pattern entries; U4 aggregates them across all patterns into per-token roll-ups before emitting.

**Patterns to follow.**

- `parse_frontmatter_tag_list` at `src/chunking.rs:400-427` for the scalar-or-list YAML shape and
  the simple parser style (hand-rolled, no `serde_yaml`).
- `MalformedPredicateEntry` at `src/chunking.rs:45-55` for the warning-channel struct.
- `parse_frontmatter_applies_when` at `src/chunking.rs:451-543` returning `(parsed, Vec<entries>)`
  for the warning-channel signature shape.
- `strip_outer_quotes` at `src/chunking.rs:770` for quote handling.

**Test scenarios.**

- `language: rust` → `["rust"]`, no entries.
- `language: [javascript, typescript]` → `["javascript", "typescript"]`, no entries.
- `language:\n  - rust\n  - golang` (block form) → `["rust", "golang"]`, no entries.
- `language: Rust` (capitalised) → `["rust"]` (normalised).
- `language: rrust` → `["rrust"]` plus one `MalformedLanguageEntry` describing the unknown token.
  **Covers AE7** (parser half).
- `language: [rust, kotlin]` (mixed known/unknown) → `["rust", "kotlin"]` plus one entry for
  `kotlin`.
- `language: []` → empty list, no entries (treated as no declaration).
- Missing `language:` field → empty list, no entries.
- `language: "quoted rust"` → `["quoted rust"]` plus entry (unknown token — pattern of
  not-stripping-internal-space).
- Malformed YAML (e.g., `language:` followed by colon) → empty list, no entries (graceful).

**Verification.**

- `cargo test chunking::tests::parse_frontmatter_language` covers the new cases.
- The new parser does not break any existing `tags:` or `applies_when:` test cases.

---

### U4. Ingest plumbing — column write and coverage tally

**Goal.** Wire the parsed `language:` token list into the schema column at ingest time; aggregate
`MalformedLanguageEntry` warnings per-token across the ingest run and emit them via the existing
`on_progress` warning channel; emit a one-line coverage tally at the end of `lore ingest` showing
how many patterns declare `language:` vs fall back to FTS.

**Requirements.** R12 (aggregation half), R13. Covers AE7 (warning-output half).

**Dependencies.** U2 (column must exist), U3 (parser produces entries to aggregate).

**Files.**

- `src/chunking.rs` — `pattern_row_from` (line 305) calls the new parser, stores the language list
  on `PatternRow` (new field) and `Chunk` (new field)
- `src/database.rs` — `upsert_pattern_in_tx` (line 835) and `insert_chunk_in_tx` (line 886) write
  the new column (serialised as JSON via existing serde or `serde_json::to_string`)
- `src/ingest.rs` — accumulate `MalformedLanguageEntry` entries during the ingest pass; aggregate
  per-token before emitting; emit the coverage tally
- The CLI command for `lore ingest` — print the tally line via the existing progress channel
- `tests/integration` — coverage-tally integration test

**Approach.**

Per-token aggregation: collect every `MalformedLanguageEntry` across the ingest pass into a
`HashMap<String, Vec<PatternId>>` keyed by the unknown token. At end-of-ingest, emit one warning per
key: `"Unknown language token`<token>`declared by <N> pattern(s)."` Single-pattern typos still
produce a clear message; bulk-typo'd repos (50+ patterns with the same misspelling) collapse to one
line.

Coverage tally: count patterns where `language_json` is non-NULL and non-empty (declared) vs the
rest (fallback). Emit one line:
`"Patterns: <total> ingested; <declared> declare
language:, <fallback> fall back to FTS coincidence."`

Wire location: tally fires after the final commit but before `lore ingest` returns success, in the
same place as the existing "ingested N patterns" summary.

**Composition-cascade audit** items that need verification (per docs/solutions/... composition
cascades):

- `delta_ingest` — when a pattern is unchanged, the existing language_json value must be preserved
  (delta path doesn't accidentally NULL-out the column).
- `full_ingest` — writes the column from the parser output.
- `clear_all` — re-populates from a fresh re-parse (DROP+CREATE preserves the schema).
- `lore list` — should not regress; reads only existing columns.
- `format_session_context` — should not regress; reads only existing columns.
- MCP `search_patterns` — uses the retrieval path; covered by U5.
- Hook-time queries (`PreToolUse`, `SessionStart`, etc.) — use the retrieval path; covered by U5.

**Patterns to follow.**

- `upsert_pattern_in_tx` at `src/database.rs:835-851` for the existing column-write shape.
- `insert_chunk_in_tx` at `src/database.rs:886-935` for chunk-side writes.
- `on_progress` / `empty_warning_message` channel pattern from
  `docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md` for per-pattern warnings.

**Test scenarios.**

- Pattern with `language: rust` in frontmatter ingests; DB row has `language_json` = `'["rust"]'`.
  **Covers AE3** (storage half, AE proper handled in U5).
- Pattern with no `language:` field ingests; DB row has `language_json` = NULL. **Covers AE5**
  (storage half).
- Re-ingest of same pattern updates `language_json` correctly if the frontmatter changed.
- delta-ingest path preserves `language_json` for unchanged patterns.
- Ingest of 12 patterns all declaring `language: rrust` (same typo) emits one warning line, not 12.
  **Covers AE7** (warning aggregation half).
- Ingest tally line appears in `lore ingest` output for a mixed repo (some declared, some not).
- `clear_all` followed by re-ingest produces the same language_json state.

**Verification.**

- `cargo test ingest::tests` includes the new column-write and aggregation cases.
- `cargo test --test integration_ingest` runs the coverage-tally integration test against a fixture
  pattern dir.
- Manual: `lore ingest` against a small fixture shows the new tally line.

---

### U5. Retrieval structural gate — three ranked lists into RRF

**Goal.** Add a structural language filter to the retrieval pipeline. Split the existing single FTS
path into two disjoint branches — FTS-fallback (today's behaviour for unlabelled patterns) and
FTS-structural (terms-only with column-membership gate for labelled patterns) — and apply an
oversample-and-filter step to the vector path so structurally-mismatched patterns can't sneak
through semantic similarity. The three independently-ranked candidate lists feed RRF as today.

**Requirements.** R9, R10, R11. Covers AE3, AE4, AE5, AE6, AE9.

**Dependencies.** U1 (detection produces the inferred-language set used by the filter), U2 (column
must exist for the filter to reference), U4 (column is populated so the structural path actually
matches things).

**Files.**

- `src/engine/query.rs` — `extract_query` return shape changes from `Option<String>` to
  `(Vec<String>, Vec<String>)` (inferred-language set + cleaned terms). Sizing: this is a breaking
  change to a publicly re-exported engine function touching ~30 caller sites in total — see the
  migration sites listed in Approach below
- `src/engine/mod.rs` — `pub use` for the new return type
- `src/database.rs` — split `search_fts` into `search_fts_fallback` and `search_fts_structural`
  (both at the same level of abstraction as today's `search_fts`); `search_vector` adjusted for
  oversample-and-filter; `search_hybrid` plumbs the inferred-language set and merges three ranked
  lists via existing `reciprocal_rank_fusion`
- `src/hook.rs` — call sites at line 191 (`extract_query` consumer), line 574 (`search_hybrid`
  invocation), line 896 (public shim) update to the new return shape
- `src/server.rs` — MCP `search_patterns` (line 643-658) extracts inferred-language set from the
  user's search query string by tokenising on whitespace, lowercasing each token, and checking each
  against the shared `LANGUAGES` table's canonical tokens; the resulting set is passed to
  `search_hybrid`. Small helper, 5-10 lines; reuses `is_known_token` from U3
- Test module — new retrieval tests covering each AE

**Approach.**

Retrieval composes as three independent ranked candidate lists fed to `reciprocal_rank_fusion`:

1. **FTS-fallback.** `MATCH "<lang> AND (terms)"` where `<lang>` is the OR-joined inferred language
   set (e.g., `(javascript OR typescript)`); `WHERE c.language_json IS NULL`. Returns patterns that
   did not declare a language and whose body contains both the language anchor (via FTS) and at
   least one enrichment term. Today's behaviour for unlabelled patterns. When the inferred set is
   empty, the MATCH collapses to `MATCH "(terms)"` (no language anchor) and the IS-NULL filter still
   applies — both labelled and unlabelled patterns admitted on terms alone per R11.

2. **FTS-structural.** `MATCH "(terms)"` only — no language anchor in the FTS predicate;
   `WHERE EXISTS (SELECT 1 FROM json_each(c.language_json) WHERE value IN (?lang_1,
   ?lang_2, ...))`
   — set intersection between the pattern's declared list and the inferred set, using SQL `IN` for
   the multi-language case. Returns patterns that declared a language matching the inferred set,
   regardless of whether the canonical token appears in the body. When the inferred set is empty,
   this branch is skipped entirely (nothing to gate against; structural eligibility is undefined).

3. **Vector (oversample-and-filter).**
   `vec0 MATCH ?embedding AND k = ?(top_k * N)
   ORDER BY v.distance` — request `N` times the
   desired result count from the KNN virtual table (initial multiplier `N = 3`, tunable).
   Post-fetch, filter the result set in code (or via a wrapping SQL with
   `JOIN chunks c ... WHERE c.language_json IS
   NULL OR EXISTS json_each ...`) by the same
   `IS NULL OR member` predicate the FTS branches use. Take the top `top_k` after filtering.
   Preserves AE4 — a wrong-language-labelled pattern can't sneak in via semantic similarity because
   the filter excludes it. When the inferred set is empty, the filter degenerates to no filter
   (terms-only/embedding-only retrieval per R11).

The two FTS branches are **disjoint by predicate** — `language_json IS NULL` and `language_json`
containing the inferred lang are mutually exclusive. No pattern double-counts across the two FTS
branches. RRF sees at most one FTS rank and one vector rank per pattern; the third-list addition
doesn't inflate FTS contribution arithmetic. Each FTS branch has its own internally-consistent BM25
weighting against its own MATCH terms — RRF uses positional rank from `enumerate`, not raw BM25
score, so cross-branch score commensurability is not a concern.

`reciprocal_rank_fusion` at `src/database.rs:951-991` already accepts any number of ranked lists as
input — extending from 2 to 3 lists is a one-line change at the caller in `search_hybrid`.

**MCP path detail.** `src/server.rs:643-658` (the `search_patterns` MCP tool handler) currently
passes the raw search string to `search_hybrid`. With the new contract, `search_hybrid` takes an
inferred-language set parameter. The MCP path extracts that set from the user's search string by
tokenising on whitespace, lowercasing each token, checking each against the LANGUAGES table's
canonical tokens, and collecting matches — e.g., a user typing `"rust async patterns"` yields the
set `{rust}`, which feeds the structural gate so labelled-Rust patterns surface even when their body
doesn't mention "rust". Five to ten lines of helper code; reuses the `is_known_token` function from
U3. Different from the hook path: the hook receives a `CallContext` (file paths and bash commands);
MCP receives a user-typed text string. Both surfaces benefit from the gate.

**`extract_query` return shape and migration scope.** The signature changes from
`pub fn extract_query(ctx: &CallContext) -> Option<String>` to a shape returning the
inferred-language set and the cleaned terms separately (e.g., `Option<(Vec<String>,
Vec<String>)>`
or returning an empty inner tuple on no signal — final shape decided at implementation time). This
is a breaking change to a publicly re-exported function. Migration sites:

- `src/hook.rs:191` (call site consuming the assembled query string today)
- `src/hook.rs:574` (`search_hybrid` invocation)
- `src/hook.rs:896` (the public shim `pub fn extract_query(input: &HookInput) ->
  Option<String>` —
  likely needs a parallel signature change or an in-shim re-assembly to preserve the public
  contract)
- ~10 tests in `src/engine/query.rs` calling `extract_query(&ctx).unwrap()` and string-asserting
  (assertions need to switch from substring checks on the assembled string to direct checks on the
  returned tuple)
- ~7 tests in `src/hook.rs` calling the shim with similar string assertions
- 13+ test call sites in `src/database.rs` for `search_fts` / `search_vector` / `search_hybrid` that
  pass the inferred-language parameter (today these tests pass pre-assembled FTS strings;
  post-change they pass the structured tuple)

Not a small change; size U5 accordingly and budget the test-migration work.

**Patterns to follow.**

- `search_fts` at `src/database.rs:240-274` for the current FTS query shape — splits into two
  functions, both at the same level of abstraction.
- `search_vector` at `src/database.rs:277-310` for the vector path — wrap with oversample-and-filter
  logic.
- `search_hybrid` at `src/database.rs:315-330` and `reciprocal_rank_fusion` at lines 951-991 for the
  RRF composition — extend the caller from 2 to 3 input lists.
- SQLite `json_each()` table-valued function — standard SQL, no extension required.
- Set-membership SQL with `IN` parametrisation — bind each inferred-language token as a separate
  parameter.

**Test scenarios.**

- Pattern with `language: rust` and body that does NOT contain "rust"; query with inferred set
  `{rust}` and any term match → pattern surfaces via FTS-structural branch. **Covers AE3.**
- Pattern with `language: rust` and any body; query with inferred set `{python}` → pattern does NOT
  surface (FTS-structural excludes; FTS-fallback requires `language_json
  IS NULL` which doesn't
  apply). **Covers AE4.**
- Pattern with no `language:` field; query with inferred set `{rust}` → pattern surfaces only if
  body matches `rust AND (terms)` via FTS-fallback. **Covers AE5.**
- Tool call with no inferred language (`mkdir build`); pattern with or without `language:` →
  terms-only retrieval (FTS-fallback collapses to terms; FTS-structural skipped; vector returns
  unfiltered top-k). **Covers AE6.**
- Multi-value pattern `language: [javascript, typescript]` surfaces on `npm test` inferred set
  `{javascript, typescript}` via FTS-structural (set intersection non-empty). **Covers AE9.**
- Multi-value pattern `language: [javascript, typescript]` surfaces on `{typescript}` query
  (singular inferred lang in pattern's list).
- Pattern with `language: rrust` (unknown token); query with inferred set `{rust}` → pattern
  surfaces only via the FTS-fallback branch if applicable (its `language_json` is `'["rrust"]'`, not
  NULL, but the EXISTS gate fails because `rrust ∉ {rust}` — falls through to no-match for
  structural; fallback excluded by IS NULL not matching either). Effectively unlabelled at retrieval
  despite the column being populated.
- Vector oversample-and-filter: configure `N = 3`, request top-k = 10, verify the intermediate fetch
  is 30 nearest, the filter applies the IS-NULL-OR-member predicate, and the final top-10 contains
  only structurally-eligible patterns.
- Wrong-label pattern surfacing test: a pattern with `language: rust`, body semantically about
  Python, and a vector embedding close to a Python query → does NOT surface (oversample-and-filter
  excludes it after the KNN fetch). **Covers AE4** (negative case, vector path).
- MCP search_patterns with query "rust async patterns": MCP layer extracts `{rust}` from the user
  string, passes to search_hybrid, structural gate fires for `language: rust` patterns even when
  their body lacks "rust".
- No regression: existing FTS-only test cases pass.
- No regression: existing vector-only test cases pass.
- No regression: existing hybrid-RRF test cases pass (RRF now takes 3 ranked lists instead of 2;
  existing tests verify the 2-list shape works when inferred set is empty).

**Verification.**

- `cargo test database::tests::search` includes the new tests.
- `cargo test --test integration_retrieval` runs the new acceptance examples end-to-end.

---

### U6. Pattern authoring guide section

**Goal.** Document the new `language:` field in `docs/pattern-authoring-guide.md`. Section structure
mirrors the existing `## Tool/command predicate (applies_when)` section.

**Requirements.** R15.

**Dependencies.** U1 (canonical token list known), U3 (parser shape known).

**Files.**

- `docs/pattern-authoring-guide.md` — new `## Pattern language declaration` section inserted after
  the existing `## Tool/command predicate (applies_when)` section (line ~465)
- Same file — small addition to the existing `## How Lore Finds Your Pattern` / Query Construction
  subsection (lines 113-158) signalling that the structural gate exists, cross-referencing the new
  section

**Approach.**

New section covers (in this order):

1. **Purpose** — declare what language(s) a pattern is about so retrieval doesn't depend on whether
   the body happens to contain the canonical token.
2. **Syntax** — `language: rust` (scalar) or `language: [javascript, typescript]` (list). Both forms
   accepted; internally normalised to a list.
3. **Canonical tokens** — table showing both the canonical FTS token (what authors type in
   `language:`) and the display name (how the language is referred to in prose), sourced from the
   shared `LANGUAGES` table's `token` and `display_name` fields. Six initial entries: Rust / `rust`,
   TypeScript / `typescript`, JavaScript / `javascript`, YAML / `yaml`, Python / `python`, Go /
   `golang`. The token-vs-display asymmetry is most visible for Go (token avoids the English
   stop-word) — call this out explicitly so authors don't try `language: go`.
4. **When to declare a single token** — the pattern is about one specific language.
5. **When to declare a multi-value list** — the pattern's content applies to a small specific set of
   languages, even if the example uses one ecosystem. List captures applicability, not provenance.
6. **When to omit the field** — content applies to too many languages or no clear-bounded set;
   retrieval falls back to FTS-coincidence on terms.
7. **Composition with `applies_when` / universal** — cross-language always-inject patterns use the
   existing mechanism; the `language:` field is orthogonal.
8. **Validation behaviour** — unknown tokens warn at ingest time (tier-2), pattern still ingests,
   structural retrieval can't match the unknown token, falls back to FTS coincidence.
9. **Worked examples** — three or four short examples covering the cases above.

The new section is a peer-level `##` heading, not nested under `applies_when`. The existing
`applies_when` doc already references "future track will extend evaluation to non-universal patterns
and introduce additional keys (`languages`, `environments`)" — this section fulfills the `languages`
half of that promise. Update or remove that forward-reference paragraph (line ~425) as appropriate.

**Test scenarios.** No code tests; this is documentation.

**Verification.**

- The new section is present, follows the structure above.
- Cross-reference from Query Construction added.
- No broken internal links.
- Visual inspection of rendered markdown.

---

### U7. Product documentation updates

**Goal.** Update the canonical product documentation surfaces affected by this feature beyond the
pattern authoring guide. Without these updates, the documented retrieval semantics and module
structure diverge from the implementation.

**Requirements.** Beyond origin's R15 (authoring guide, covered by U6), this unit serves
plan-internal completeness: the brainstorm decoupled `lore-patterns` migration but did not address
lore's own product documentation surfaces, which describe how retrieval works today. Once U1–U5
land, those documents are stale.

**Dependencies.** U1 (engine structure known), U2 (schema column known), U3 (parser shape known), U5
(retrieval composition settled). Lands last; depends on every other unit's shape being finalised.

**Files.**

- `docs/search-mechanics.md` — substantive update covering the new retrieval pipeline (three
  independently-ranked candidate lists), the structural gate's WHERE-clause semantics, the
  disjoint-FTS-branches arithmetic, and the oversample-and-filter vector path. This is the canonical
  document for retrieval; it must reflect post-change behaviour.
- `docs/hook-pipeline-reference.md` — small addition explaining that PreToolUse retrieval admits
  structurally-labelled patterns via the new gate, cross-referencing the updated
  `search-mechanics.md`.
- `docs/architecture.md` — small layout update mentioning `src/engine/languages.rs` and the split
  `search_fts_fallback` / `search_fts_structural` functions in the engine module section.
- `docs/configuration.md` — short subsection on the `language:` frontmatter field (when to declare,
  where to find the canonical token list — reference the authoring guide section from U6). If the
  vector oversample factor `N` becomes configurable per the Outstanding Question, document it here;
  if it stays hardcoded, no entry needed.
- `CHANGELOG.md` — one assertive-voice sentence per the project's CHANGELOG convention (user-facing
  changes only; one sentence ending in `(#N)`). Lands at PR-merge time as part of the commit, not as
  a separate unit step.
- `ROADMAP.md` — move the "Extend language detection dictionaries" bullet from "Up Next" to
  "Completed" at commit time. Bookkeeping, not a code change.
- `docs/solutions/conventions/schema-migration-strategy-2026-05-14.md` _(new)_ — written as part of
  this slice. Captures the "DB is a derived artefact" foundational principle, the two operational
  strategies (silent additive ALTER vs hard-bail with `--force`), the code-structure / UX /
  versioning / testing contracts, and the bump history so far. Future schema bumps consult this doc
  rather than re-deciding from scratch.

**Approach.**

Each doc gets the smallest change that keeps it consistent with the new behaviour. No new pages, no
new heading hierarchies — match the prose style of each existing doc.

- **`search-mechanics.md`** is the heaviest update. Find the section describing today's retrieval
  composition (single FTS + vector + RRF) and rewrite it to describe three ranked lists:
  FTS-fallback (today's `<lang> AND (terms)` for `language_json IS NULL` patterns), FTS-structural
  (terms-only with `EXISTS json_each` membership for declared patterns), and oversample-and-filter
  vector. Add a short paragraph on the disjoint-FTS-branches arithmetic so a reader understands the
  three-list count is a code-organisation choice, not an RRF-arithmetic inflation. Cross-reference
  the new authoring-guide section from U6.
- **`hook-pipeline-reference.md`**: one paragraph noting that PreToolUse retrieval benefits from the
  structural gate when patterns declare `language:`, with a forward reference to search-mechanics
  for the algorithmic detail.
- **`architecture.md`**: one or two lines under the engine-module section adding the new
  `languages.rs` and the FTS-function split.
- **`configuration.md`**: short subsection on the `language:` field. Brief — most of the authoring
  detail lives in U6's pattern-authoring-guide section.
- **`CHANGELOG.md`**: at PR-merge time. Example wording (final wording at PR time):
  `feat: First-class language: frontmatter field gates pattern retrieval structurally (#N)`.
- **`ROADMAP.md`**: at PR-merge time. Move the existing language-detection bullet to Completed with
  a one-line summary and the plan link.

**Patterns to follow.**

- Existing prose style in each affected doc — don't introduce new heading hierarchies or section
  conventions.
- `dprint` markdown formatting per project convention (line width 100, hard-wrapped prose).
- The brainstorm's other "Completed" ROADMAP entries for the bookkeeping format.
- Existing CHANGELOG entries for the assertive-voice / `(#N)` shape (per memory: "user-facing
  changes only, one assertive-voice sentence ending in `(#N)`").

**Test scenarios.** No code tests; this is documentation.

**Verification.**

- Each affected doc updated and reads coherently against the post-U5 retrieval pipeline.
- Cross-references between authoring guide, search mechanics, and hook pipeline reference resolve
  correctly.
- Markdown links validate (`just ci` / `dprint check` catches formatting).
- CHANGELOG entry follows the assertive-voice / `(#N)` convention.
- ROADMAP.md bullet moved Up Next → Completed at commit time.

---

## Acceptance Examples

The plan adopts AE1–AE8 from origin and adds AE9 below to cover the multi-language inference path
introduced by U1's shared-signal handling.

- **AE9.** **Covers R5 (shared signals) and R9 (structural retrieval).** Given a Bash tool call with
  command `npm test`, when language inference runs, the inferred set is `{javascript, typescript}`
  because both entries register `npm` as a command keyword. When retrieval runs against a pattern
  declaring `language: [typescript]`, the FTS-structural branch admits the pattern because the set
  intersection (`{javascript,
  typescript} ∩ {typescript}`) is non-empty. When retrieval runs
  against a pattern declaring `language: rust`, the FTS-structural branch excludes it because the
  set intersection is empty; the FTS-fallback branch also excludes it because the pattern's
  `language_json` is not NULL.

---

## Verification Strategy

End-to-end verification for the slice:

- **Unit tests** — every implementation unit has unit-test coverage per the test scenarios listed
  in-unit. Cargo test command: `cargo test --lib`.
- **Integration tests** — `cargo test --test integration_ingest` and
  `cargo test --test
  integration_retrieval` cover the cross-module flows (frontmatter → column →
  retrieval).
- **Migration test** — the remedy-completion test in U2 uses `Command::cargo_bin("lore")` against a
  hand-built v3 fixture to exercise the full upgrade flow.
- **Regression sweep** — full `cargo test` confirms no existing test breaks.
- **Manual smoke test** — `lore ingest` against a small fixture repo with a mix of labelled and
  unlabelled patterns; verify the coverage tally appears and the warning channel surfaces typos.
- **Documentation consistency check** — after U7 lands, re-read `docs/search-mechanics.md`,
  `docs/hook-pipeline-reference.md`, `docs/architecture.md`, and `docs/configuration.md` alongside
  the implementation to verify the documented behaviour matches code; verify the CHANGELOG entry and
  ROADMAP update are in place at PR-merge time.

### Composition-cascade audit

The audit splits into two sub-tables. The first lists subsystems that need active verification work;
the second lists subsystems pre-cleared as no-action because they only read pre-existing columns.

**Audit required:**

| Subsystem | Read site                                         | Audit item                                                               |
| --------- | ------------------------------------------------- | ------------------------------------------------------------------------ |
| Ingest    | `delta_ingest`                                    | Preserves existing `language_json` on unchanged patterns                 |
| Ingest    | `full_ingest`                                     | Writes column from parser output                                         |
| Ingest    | `clear_all`                                       | DROP+CREATE picks up the new column from DDL                             |
| Hook      | `PreToolUse`, `SessionStart`, `PostCompact`, etc. | Use the retrieval path; covered by U5                                    |
| MCP       | `search_patterns`                                 | Uses the retrieval path with MCP-side language extraction; covered by U5 |

**Confirmed safe (no action required — reads only pre-existing columns):**

| Subsystem | Read site                |
| --------- | ------------------------ |
| CLI       | `lore list`              |
| CLI       | `lore status`            |
| CLI       | `format_session_context` |
| MCP       | `lore_status`            |

---

## Key Technical Decisions

- **JSON column on `patterns` + `chunks`, not a separate `pattern_languages` join table.** Matches
  the established `applies_when_json` precedent; cardinality is bounded (~6 languages today, low
  double digits long-term); SQLite's `json_each()` provides the membership check in pure SQL without
  a join table.
- **Mirror v2→v3 (`applies_when_json`) migration shape exactly.** Additive `ALTER TABLE` with
  `column_exists` idempotency guard; in-place upgrade with no `--force` required for v3→v4. Pre-v3
  databases continue to fall through to the existing hard-bail path.
- **`const &[LanguageEntry]` static array, linear iteration.** No `HashMap`, `OnceLock`, `phf_map`,
  or `lazy_static`. Six entries today; cost of linear iteration is negligible and the static-slice
  convention already exists in `STOP_WORDS`.
- **Bash tokenisation via `command.split_whitespace()` + KEY=VAL filter.** Matches the existing
  `command_matches_with_wrappers` precedent for shell-token-aware matching. Avoids re-using the
  wrapper-stripping head-only logic — `language_from_bash` wants any-token-in-set matching.
- **Retrieval composes as three independently-ranked candidate lists fed to RRF**, not a UNION of
  FTS queries. The two FTS branches (fallback and structural) are disjoint by their `language_json`
  predicates — a pattern is either in one or the other, never both. RRF sees at most one FTS rank +
  one vector rank per pattern; the 3-list count is a code-organisation choice, not an arithmetic
  inflation. Each FTS branch keeps its own internally-consistent BM25 weighting against its own
  MATCH terms.
- **Vector path uses oversample-and-filter** to preserve AE4. Request top-k * N nearest neighbours
  from the `vec0` virtual table (initial N = 3, tunable), then filter by the IS-NULL-OR-member
  predicate, take top k. Cost: recall may drop below k for very-narrow-language corpora; the
  oversample factor is the lever.
- **`extract_query` returns the inferred-language set and cleaned terms separately** (Vec<String>
  for inferred langs, Vec<String> for terms). Supports the multi-language case (e.g., `npm test`
  yielding `{javascript, typescript}`) without coercing to a singular value. The downstream
  retrieval composition handles set-cardinality 0, 1, and N uniformly.
- **MCP `search_patterns` extracts inferred language from the user's search string.** Tokenise on
  whitespace, lowercase, check each token against the LANGUAGES table's canonical tokens, collect
  matches. Small helper (5-10 lines) reusing `is_known_token` from U3. Different from the hook path
  (which infers from `CallContext`'s file paths and bash commands), but produces the same
  `Vec<String>` shape for `search_hybrid` and delivers consistent gate behaviour across hook and MCP
  surfaces.
- **Per-token warning aggregation at ingest level.** Parser returns per-pattern entries; ingest
  aggregates into a `HashMap<token, Vec<pattern_id>>` and emits one line per token. Single typo
  across 50 patterns surfaces as one warning, not 50.
- **No new `CallContext` fields.** Existing `file_path: Option<String>` is sufficient for
  marker-filename and directory-hint extraction via `Path::file_name()` and `Path::components()`.
  Honours the engine's no-I/O contract.

---

## Risks & Dependencies

- **Schema migration risk** — mitigated by the established Universal-pattern precedent and the
  remedy-completion test pattern. The composition-cascade audit (per learnings) must verify every
  read path tolerates NULL `language_json` rows.
- **Migration UX is asymmetric.** v3→v4 is silent in-place for current users (no advisory, no
  `--force`); pre-v3 → v4 hard-bails with the established friendly advisory
  - `lore ingest --force` (legacy users only). AE8 applies to the legacy path specifically.
    Migration code does not need new advisory wording for the silent path — silence is the correct
    UX for an additive nullable column whose absence (NULL) has a defined fallback behaviour (R10).
    This asymmetry is consistent with the "DB is a derived artefact" project convention: the ALTER
    TABLE is purely a cache-shape change, and re-ingest from source markdown is the data path for
    both strategies.
- **Bash matcher behavioural risk** — the word-boundary fix could expose new false positives or
  false negatives that the substring matcher silently absorbed. Test coverage explicitly includes
  the regression case (`bundle install`) and env-prefix handling, but real-world commands may
  surface edge cases. Mitigation: keep the bash-matcher test set extensive in U1; add new cases as
  bugs surface in dogfooding.
- **Retrieval composition risk** — the three-ranked-lists approach is new and depends on
  `reciprocal_rank_fusion` tolerating an additional input list (one-line caller change). Tests must
  verify ranking is preserved when the inferred set is empty (RRF reduces to 2 lists, matching
  today). Vector oversample-and-filter may drop recall below `top_k` for very-narrow-language
  corpora; the N multiplier is the tunable. Mitigation: explicit recall-floor test in U5 with a
  single-language corpus and an aggressive oversample factor.
- **`extract_query` return-type change** — breaking change to a publicly re-exported function. ~30
  caller sites need updating (hook.rs, hook.rs shim, engine/query.rs tests, hook.rs tests,
  database.rs search tests). U5 is the most dependency-heavy unit in this slice; budget for the
  test-migration work explicitly.
- **FTS5 sanitiser compatibility** — for the six initial canonical tokens
  (`rust`/`typescript`/`javascript`/`yaml`/`python`/`golang`), all are FTS5-safe. No audit needed
  for this slice. The follow-on language-pack work that introduces hyphenated tokens (`objective-c`)
  or `+`/`#`-bearing tokens (`c++`, `f#`) will need to audit the sanitiser before adding those
  entries.
- **`universal` tag deprecation** — orthogonal to this slice. The brainstorm noted the `universal`
  tag is in deprecation orbit; nothing in this slice ties to its continued existence. The
  `applies_when` mechanism is the surviving primitive for cross-language always-inject patterns
  regardless.

---

## Scope Boundaries

Carried forward from origin (see
`docs/brainstorms/2026-05-13-language-detection-architecture-requirements.md`):

- New language additions beyond the existing six — maintainer-incremental follow-on.
- Migration of `lore-patterns` or any other knowledge repository — independent work.
- Runtime config-driven language table.
- Synonym groups in query output.
- Directory-as-fallback inference for labelling.
- Mandatory `language:` field with friendly advisory.
- Content-sniffing (shebang, file header).
- Walking up the path to find a manifest.
- `.gitignore` parsing for dynamic directory hints.
- Ranking boost for structural matches.
- Removing the FTS-coincidence fallback path.

### Deferred to Follow-Up Work

- Vendored-dependency edge case for R7's priority rule (`node_modules/foo/Cargo.toml` inferring
  `rust` despite the vendor context). AE2 accepts this as intentional; revisit if dogfooding reveals
  it as a real problem.
- Compounding-cost analysis for repeated schema bumps (FYI from doc-review).
- Bash word-boundary matcher could in principle ship as a separate small PR (FYI from doc-review).
  Bundled here for slice cohesion — the matcher fix is the smallest piece and slips cleanly into U1
  alongside the table refactor.

---

## Outstanding Questions

### Deferred to Implementation

- **Exact column nullability defaults** for `language_json`. Plan assumes `TEXT NULL` (matching
  `applies_when_json`). If the migration assigns `DEFAULT '[]'` instead, the retrieval gate's
  `IS NULL` branch becomes `language_json = '[]' OR language_json
  IS NULL` — verify during U2.
- **Whether `chunks.language_json` is strictly necessary** or `patterns.language_json` alone
  suffices. The existing `applies_when_json` lives on both, suggesting the precedent is "both"; the
  retrieval path joins through `chunks`, so the gate needs at least one of them on `chunks`. Plan
  currently mirrors the precedent (both tables). Verify in U2/U5 whether the patterns-table copy is
  actually consulted anywhere; if not, drop it.
- **Tally output exact format** — the wording "X declared, Y fallback" in U4 is a draft. Final
  wording chosen at implementation time; should match existing ingest-summary line style.
- **Vector oversample factor N** — plan suggests `N = 3` as initial; tune at implementation time
  against a fixture corpus. The recall floor may differ across query shapes (single-language vs
  multi-language inferred set, narrow vs broad term set).

---

## System-Wide Impact

See the **Composition-cascade audit** sub-tables in the Verification Strategy section for the
per-subsystem audit checklist. The full set of read sites touching `chunks` and `patterns` is
enumerated there; this section avoids duplication.

Affected parties: pattern authors (new optional frontmatter field, documented in U6); lore CLI users
(one-time `lore ingest --force` after upgrade per AE8); agents performing tool calls (new structural
retrieval gate transparent to them, behaviour unchanged for unlabelled patterns per R10); MCP
`search_patterns` callers (new language extraction from search query strings produces sharper
retrieval, transparent on the consumer side).

Downstream consumers (lore-patterns or any other knowledge repository) are decoupled from this slice
— they may use the new field but are not required to migrate.
