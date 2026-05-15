---
date: 2026-05-15
status: ready-for-planning
scope: lightweight
follows: docs/brainstorms/2026-05-13-language-detection-architecture-requirements.md
---

# Surface language coverage in `lore status`

## Problem

PR #50 shipped the `language:` frontmatter feature and the structural retrieval gate it drives.
Operators have no way to see, from the CLI, how much of their knowledge base actually declares a
language — and therefore how much of it can benefit from the structural gate versus falling through
to the undeclared fallback path. `lore status` already reports `Chunks` and `Sources` but is silent
on this dimension.

## Goal

Add a per-language breakdown to `lore status` that reports, across ingested sources, how many
declare each language in frontmatter plus how many declare nothing.

## Behaviour

After the existing `Sources:` line in `cmd_status` (`src/main.rs:779`), emit a `Languages:` block
listing each declared language with its source count and an `undeclared` bucket at the end:

```
Languages:    Rust 12, TypeScript 5, YAML 3, undeclared 5
```

- Counts are over distinct **source files**, not chunks. A source contributes to each language it
  declares — `language_json` is a JSON array, so a pattern tagged `[rust, typescript]` counts toward
  both. A source with null `language_json` contributes only to `undeclared`.
- Each language is rendered using its `display_name` from `LANGUAGES` (`src/engine/languages.rs`) —
  `Rust`, `TypeScript`, `YAML`, `Python`, `Go`, `JavaScript` — not the canonical FTS5 `token` stored
  in `language_json`. Resolution is a lookup keyed by `token`; an unknown token (would only appear
  if frontmatter declared a language the table doesn't cover yet) falls back to the raw token so the
  operator still sees something meaningful.
- Ordering: declared languages by count descending, ties broken alphabetically on the display name;
  `undeclared` always last.
- Wrapping: if the joined list exceeds the available width, continuation lines align under the value
  column (mirrors the existing `Scan set:` / `Last commit:` indentation discipline). Exact wrap
  width matches whatever the surrounding lines use; no new wrapping primitive needed.
- When the database is empty or has no sources, the whole block is suppressed — consistent with the
  existing `db.stats()` guard around the surrounding block.
- When every source is undeclared, the line still emits as `undeclared N` so the operator can see
  the gate has nothing to act on.
- Writes to stderr alongside the other status lines (`cli-output-conventions`: status is diagnostic,
  not data output).

## Out of scope

- Listing the supported-language table from `src/engine/languages.rs`. Static data; belongs in
  `--help` or documentation, not in runtime status.
- JSON output for `lore status`. Not currently structured; out of scope for a one-line followup.
- Any change to the language gate, the schema, or ingestion. This is a read-only surfacing change.

## Success criteria

- `lore status` against a populated database prints the new `Languages:` block in the expected
  position with one count per declared language plus an `undeclared` bucket.
- Counts match a manual query against `language_json` — including the multi-language case (a source
  declaring `[rust, typescript]` is counted in both buckets, and the bucket sum can exceed
  `Sources:`).
- Ordering is stable: count descending, alphabetical tiebreak, `undeclared` last.
- A unit test on the count helper locks the multi-language and all-undeclared cases.

## Open questions

None. The query shape is determined by the existing `language_json` column; the copy and ordering
are fixed.
