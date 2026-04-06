---
date: 2026-04-05
topic: loreignore
---

# .loreignore

## Problem Frame

Every `.md` file in a pattern repository is indexed during ingest, including repository
documentation (README, CONTRIBUTING, LICENSE) and tooling files (.github/). These non-pattern files
pollute the knowledge database with chunks that will never be useful search results for an agent,
diluting search relevance and eroding trust in results.

Users currently have no way to exclude files from indexing without removing them from the
repository.

## Requirements

**File discovery and filtering**

- R1. A `.loreignore` file at the root of a pattern repository specifies files and directories to
  exclude from indexing
- R2. The file uses gitignore-style glob syntax: bare filenames (`README.md`), directory patterns
  with trailing slash (`docs/`, `.github/`), wildcards (`*.txt`), and recursive globs
  (`**/*.draft.md`). Patterns without a slash match in any subdirectory; patterns with a slash are
  anchored to the repository root. Negation patterns (`!`) un-ignore a previously excluded file.
  These semantics must be preserved regardless of the implementation crate chosen
- R3. Filtering is purely opt-in — no built-in defaults. Without a `.loreignore` file, all markdown
  files are indexed as today
- R4. The `.loreignore` file itself is never indexed (note: already excluded by the existing
  markdown-only extension filter; no additional handling required)
- R5. Lines beginning with `#` are comments. Empty lines are ignored. Malformed patterns (invalid
  glob syntax) are skipped with a warning to stderr and do not abort the ingest

**Full ingest**

- R6. Full ingest reads `.loreignore` before the directory walk and skips matched files. Full ingest
  already calls `db.clear_all()` before re-indexing, so previously indexed files that are now
  ignored are implicitly excluded — no additional removal logic is needed on this path

**Delta ingest**

- R7. Delta ingest reads `.loreignore` and skips matched files when processing
  `git diff --name-status` output
- R8. Delta ingest detects changes to `.loreignore` itself in the diff (added, modified, or
  deleted). When `.loreignore` has changed, delta ingest runs a reconciliation pass: query all
  `source_file` entries in the database, check each against the current ignore list, and delete
  chunks for any file that is now ignored. This handles the case where `.loreignore` is the only
  file that changed in a commit
- R9. If `.loreignore` is deleted, the reconciliation pass finds no files to remove (no ignore list
  means nothing is excluded). Previously ignored files will be re-indexed on the next full ingest or
  when they next appear in a delta diff

**Observability**

- R10. At each skipped file, emit a `lore_debug!` line including the file path and the matched
  pattern. No new debug infrastructure is required
- R11. When a reconciliation pass removes chunks for previously indexed files, emit a `lore_debug!`
  line for each removal

## Success Criteria

- Non-pattern markdown files (README, CONTRIBUTING, LICENSE) no longer appear in search results
  after adding them to `.loreignore` and re-ingesting
- Existing repositories without a `.loreignore` file behave identically to today
- Delta ingest correctly removes chunks for newly ignored files when `.loreignore` changes
- Malformed patterns produce a warning but do not prevent other valid patterns from being applied

## Scope Boundaries

- Negation patterns (`!` syntax) are supported — the `ignore` crate handles them natively. Negation
  only widens what gets indexed (un-ignores a file), so there is no security concern
- No support for nested `.loreignore` files in subdirectories. This is a known limitation: users
  with monorepo-style pattern repositories must use root-level patterns with path prefixes (e.g.,
  `team-a/drafts/`) to target subdirectories
- No changes to the MCP tool interface — this is purely an ingest pipeline concern
- The `.loreignore` file does not affect `lore add`, `lore append`, or `lore update` MCP operations
  (these write to explicit paths chosen by the caller)
- The `.loreignore` path is not configurable — it is always at the repository root. Configurability
  via `lore.toml` is not planned

## Key Decisions

- **Purely opt-in, no built-in defaults:** Predictable behaviour, no surprises. Users add exclusions
  explicitly
- **Gitignore-style syntax:** Familiar mental model, well-understood by the target audience. The
  semantics (trailing-slash directory matching, recursive globs, anchoring rules) are a hard
  requirement — the implementation crate must support them faithfully
- **Reconciliation on `.loreignore` change:** Delta ingest detects `.loreignore` modifications and
  scans the database for stale entries, rather than requiring a manual full re-ingest. This keeps
  the delta path self-healing without adding excessive complexity
- **Graceful handling of malformed patterns:** Skip invalid lines with a warning rather than
  aborting. One bad pattern should not prevent all other exclusions from working

## Outstanding Questions

### Deferred to Planning

- [Affects R2][Needs research] Should we use the `ignore` crate (ripgrep's engine) or `globset` for
  pattern matching? The `ignore` crate handles gitignore semantics natively (trailing-slash,
  anchoring, recursive globs) but adds transitive dependencies (crossbeam, thread_local). `globset`
  is lighter but requires manual implementation of gitignore directory semantics. Evaluate the
  dependency cost against the project's minimal binary size stance (opt-level z, LTO, strip)
- [Affects R6][Technical] The `ignore` crate can replace `WalkDir` entirely with its own walker that
  reads ignore files natively. Alternatively, filtering can be applied after the existing `WalkDir`
  collection using `globset`. The crate choice determines the integration approach

## Next Steps

→ `/ce:plan` for structured implementation planning
