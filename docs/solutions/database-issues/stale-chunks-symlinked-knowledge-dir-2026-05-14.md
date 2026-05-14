---
title: Stale chunks after update through symlinked knowledge directories
date: 2026-05-14
category: database-issues
module: ingest
problem_type: database_issue
component: database
symptoms:
  - "`update_pattern` and `append_to_pattern` leave stale chunk rows in the SQLite index when the knowledge directory is reached through a symlink"
  - Searches return both the old and new pattern bodies for the same source after an update or append
  - Chunk rows for the same file end up keyed under both a relative path and an absolute path, splitting the index
  - Reproduces reliably on macOS where `tempfile::tempdir()` and `$TMPDIR` resolve via `/var` to `/private/var` symlinks
  - Linux-only CI never exercised the symlinked-knowledge-dir path, so the regression shipped undetected in v0.3.0
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - tooling
  - testing_framework
tags:
  - sqlite
  - symlinks
  - macos
  - canonicalisation
  - chunk-index
  - rel-path
  - ingest
  - strip-prefix
---

# Stale chunks after update through symlinked knowledge directories

## Problem

`update_pattern` and `append_to_pattern` corrupted the pattern index whenever the knowledge
directory was reached through a symlink. Fresh chunks were inserted under one `source_file` key
while stale chunks for the same file lingered under another. Subsequent searches could return the
old body, the new body, or both — depending on which key happened to resolve first. The bug shipped
in v0.3.0 because the project's continuous integration was Linux-only; macOS development hit it
immediately after release.

## Symptoms

- After updating or appending to a pattern, searching for that pattern returned two bodies for the
  same `source_file` — the old and the new — instead of one.
- Test failures on macOS that did not reproduce on Linux:
  - `mcp_update_pattern_clears_applies_when_when_predicate_stripped` found stale chunks alongside
    the freshly written ones.
  - `mcp_append_to_pattern_preserves_applies_when_json_on_every_chunk` reported one chunk where it
    expected at least two, because the append landed under a different `source_file` than the
    original ingest.
  - `update_pattern_with_none_tags_preserves_existing_frontmatter_tags` reported the universal flag
    missing because the post-update row was filed under an absolute path the query never used.
- A separate cluster of four `plant_non_utf8_md` test panics surfaced at the same time on macOS,
  caused by APFS rejecting `0xFF`-prefixed filenames with `EILSEQ`. That cluster is unrelated to
  this defect but worth knowing about — see Prevention for the test-side mitigation that shipped in
  the same fix.

## What Didn't Work

The first fix attempt canonicalised only the knowledge directory, leaving the incoming file path
untouched:

```rust
// Broken: only one side normalised.
let canonical_dir = knowledge_dir.canonicalize()?;
let rel_path = file_path
    .strip_prefix(&canonical_dir)
    .unwrap_or(file_path)
    .to_string_lossy()
    .to_string();
```

This made `update_pattern` and `append_to_pattern` tests pass, but broke more than twenty-five other
tests in the `ingest::tests::` module. The reason: `full_ingest` and `delta_ingest` happened to pass
non-canonical paths on both sides, so the original bug had been compensating for itself (`/var/...`
strips cleanly off `/var/...`). Canonicalising only the directory flipped the asymmetry for that
caller group, and `rel_path` collapsed to an absolute path in the opposite direction. Fixing at one
side of the comparison simply moved the bug.

## Solution

In `src/ingest.rs`, function `index_single_file` (lines 1637–1648), canonicalise both sides before
stripping:

```rust
let content = std::fs::read_to_string(file_path)?;
// Canonicalise both sides so strip_prefix succeeds regardless of which
// caller mix passes canonical or non-canonical paths. Without this,
// macOS /var → /private/var symlinks cause rel_path to fall through to
// an absolute path, keying DB rows inconsistently across ingest paths.
let canonical_dir = knowledge_dir.canonicalize()?;
let canonical_file = file_path.canonicalize()?;
let rel_path = canonical_file
    .strip_prefix(&canonical_dir)
    .unwrap_or(&canonical_file)
    .to_string_lossy()
    .to_string();
```

After this change, `just ci` and `just test-integration` are both green on macOS: 583 passed, 0
failed.

## Why This Works

`index_single_file` uses `rel_path` as the SQLite `source_file` key, and its transaction deletes
existing rows for that key before reinserting. The original derivation succeeded only when both
sides of `strip_prefix` shared the same canonicalisation state.

- `ingest_single_file` canonicalised the knowledge directory itself before calling, so both sides
  matched and `rel_path` became `"foo.md"`.
- `update_pattern` and `append_to_pattern` fed in a canonical file path from `validate_within_dir`
  (which canonicalises in service of path-traversal protection) but a non-canonical knowledge
  directory.

On macOS, `tempfile::tempdir()` returns paths under `/var/folders/...`, which is a symlink to
`/private/var/folders/...`. `Path::canonicalize` resolves the symlink; the bare path does not. So
`strip_prefix(/var/folders/...)` over `/private/var/folders/.../foo.md` failed, the
`unwrap_or(file_path)` fall-through kept the absolute path, and the delete-then-insert pair fired
against a key the original ingest had never used. Old rows were never deleted; new rows were
inserted under the absolute path. Two row sets for the same source file under two different keys.

Canonicalising both sides inside the function preserves the symmetry regardless of which caller
convention is in play. The delete now always targets the same rows the next insert will replace.

## Prevention

- Add a macOS runner to continuous integration so this class of `/var` → `/private/var` symlink
  behaviour is caught before release.
- When a function combines two paths and compares them, canonicalise consistently on both sides or
  neither. Never trust callers to pass canonical paths uniformly.
- For any path-keyed database column, write at least one round-trip test (ingest, then update, then
  re-query by the original key) that runs on macOS.
- Gate filesystem-capability-sensitive tests on a cached probe rather than `cfg(target_os)`. The
  same commit replaced an unconditional `plant_non_utf8_md` helper with `try_plant_non_utf8_md`
  backed by a `OnceLock<bool>` that probes once whether the tempdir filesystem accepts non-UTF-8
  filenames. APFS rejects them with `EILSEQ`; the probe lets the tests skip cleanly on macOS while
  still exercising the path on Linux.
- Treat silent `unwrap_or(value)` fall-throughs on `strip_prefix` results as a code smell when the
  result is used as a stable identifier. The fall-through path silently produces a different key
  shape than the success path, and the difference only matters under conditions a developer is
  unlikely to test locally.

## Related Issues

- [`docs/solutions/design-patterns/round-trip-discriminator-canonicalise-both-sides-2026-05-10.md`](../design-patterns/round-trip-discriminator-canonicalise-both-sides-2026-05-10.md)
  — the same "canonicalise on both sides" lesson, applied to Unicode round-tripping in `add_pattern`
  rather than filesystem paths.
- Commit `e3ecff6` on branch `fix/canonicalise-source-file-keys`, draft PR #55, against `main`.
