---
title: "Render `## Pinned conventions` from chunk bodies, not disk re-read"
priority: P2
category: architecture
status: ready
created: 2026-04-21
source: ce-review (feat/universal-patterns)
files:
  - src/hook.rs:513-607
  - src/ingest.rs:1210-1220
  - src/database.rs:384
related_pr: feat/universal-patterns
---

# Render `## Pinned conventions` from chunk bodies, not disk re-read

## Context

During ce-review of the universal-patterns PR, the architecture reviewer flagged that
`render_pinned_conventions` in `src/hook.rs:513-607` mixes two trust domains inside a single
SessionStart payload:

1. The `## Pinned conventions` section re-reads the source markdown file from disk (`hook.rs:590`),
   guarded by `validate_within_dir` against a tampered chunk row pointing at `../../../etc/passwd`.
2. The existing `Available patterns:` index renders from the DB (chunk titles via `list_patterns`).

Two consequences fall out of this split:

- **Source drift.** If an author edits a pattern file between `lore ingest` and a SessionStart, the
  pinned section shows the _fresh_ body, but the index shows stale chunk titles — or vice versa.
  Users think they have consistency; they don't.
- **The path-traversal guard exists only because of the split.** `validate_within_dir` in
  `src/ingest.rs:1210-1220` is load-bearing for hook.rs at `src/hook.rs:582` specifically because
  the DB is treated as trusted for `source_file` but untrusted for filesystem access. That is an
  awkward middle ground.

Rendering from the DB eliminates both: single source of truth, no filesystem I/O at SessionStart, no
containment guard required for the pinned section.

## Proposed fix

Replace the disk re-read in `render_pinned_conventions` with a `db.universal_chunks_by_source()`
call that returns `Vec<(source_file, Vec<ChunkBody>)>` grouped by file in deterministic order, where
each `ChunkBody` carries `heading_path` and `body`. Concatenate per-file bodies with blank lines,
render the group under a `### <title>` heading inside `## Pinned conventions`.

```rust
fn render_pinned_conventions(db: &KnowledgeDB) -> anyhow::Result<Option<String>> {
    let groups = db.universal_chunks_grouped_by_source()?;
    if groups.is_empty() {
        return Ok(None);
    }
    // Render each group under its source-file title, bodies separated by "\n\n".
    Ok(Some(format_pinned_section(&groups)))
}
```

Trade-off: the rendered body is the _chunked_ form (post-frontmatter, post-heading-split), not the
raw markdown. For most patterns that's identical; for patterns that rely on YAML frontmatter being
visible in the rendered body, the output changes. Pattern-authoring guidance should clarify that
frontmatter is metadata, not content.

## Why it's parked as a follow-up, not a blocker for the universal-patterns PR

- The current disk-read implementation is correct modulo the TOCTOU fix already in scope for this
  PR. It does not leak data.
- The path-traversal guard and `validate_within_dir` are load-bearing elsewhere (`add_pattern`,
  `update_pattern`, `append_to_pattern` write paths) and remain required regardless.
- Switching render sources is a meaningful behavioural shift deserving its own benchmark + test
  pass. Folding it into a 2.5K-line PR would blur the review signal.

## Test surface

When picked up:

1. `render_pinned_conventions_concatenates_multi_chunk_file_bodies_in_heading_order` — a universal
   pattern with three headings emits all three chunk bodies under the same file group.
2. `render_pinned_conventions_preserves_source_file_order` — two universal files, verify lexical or
   ingest order is stable across runs.
3. `render_pinned_conventions_handles_deleted_source_file_gracefully` — DB has universal chunks but
   the file on disk is gone; render still succeeds (no disk dependency).
4. `render_pinned_conventions_ignores_chunk_body_differing_from_source_file` — deliberately mutate a
   chunk body after ingest and confirm the render uses the DB body, not disk.

Existing tests for the path-traversal guard
(`hook_session_start_skips_pinned_pattern_with_path_traversal_source_file`) become obsolete for the
pinned section and should be removed; the guard remains tested through the write-path consumers.

## Trade-offs

- **Frontmatter visibility change.** Chunk bodies do not include the file's YAML frontmatter. If any
  existing universal pattern relies on the frontmatter being visible in the injected text, this
  changes its behaviour. Grep confirms the current universal tag just triggers behaviour; no pattern
  in `lore-patterns` relies on frontmatter being visible.
- **Heading-path granularity.** `chunk_by_heading` produces one chunk per heading section; the
  concatenated rendering needs to re-assemble the hierarchy. Simplest shape: `### <heading_path>`
  per chunk, body, blank line. Matches the SessionStart format that agents already parse.
- **Loses on-disk edits between ingest and session start.** Today, editing a universal pattern file
  and NOT re-ingesting still shows the edited body in SessionStart (because disk is read at session
  time). After this change, edits require `lore ingest` to take effect — which is the documented
  contract everywhere else in lore. Alignment, not regression.

## When to do this

Pick up when:

- A pattern author reports confusion about edit-vs-ingest timing of the pinned section.
- Any refactor touches `format_session_context` or `validate_within_dir`.
- A second universal-like mechanism (e.g., cycle-based dedup TTL) makes the disk-read path look even
  more special-cased.

## References

- ce-review synthesis (2026-04-21) — architecture reviewer + security reviewer both recommended
  unifying on the DB source.
- `src/hook.rs:513-607` — current `render_pinned_conventions` with disk read + path guard.
- `src/ingest.rs:1210-1220` — `validate_within_dir` (remains required for write-path consumers).
- `src/database.rs:384` — `universal_patterns()` returns `PatternSummary`; the grouped variant for
  pinned rendering would be a new sibling method.
- Plan: `docs/plans/2026-04-20-001-feat-universal-patterns-plan.md` (Future Considerations).
