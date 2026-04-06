---
title: "Delta ingest requires committed changes for pattern testing"
date: 2026-04-05
category: best-practices
module: ingest
problem_type: best_practice
component: tooling
severity: low
status: superseded
superseded_by: "lore ingest --file <path> (2026-04-06)"
applies_when:
  - "Testing whether a new or modified pattern surfaces in search results"
  - "Running the vocabulary coverage technique from the pattern authoring guide"
  - "Debugging why a recently edited pattern does not appear in lore search"
tags: [ingest, delta, git, commit, pattern-testing, workflow]
---

> **Superseded (2026-04-06):** `lore ingest --file <path>` now indexes an uncommitted file directly
> without touching delta-ingest state. The original friction described below no longer applies —
> prefer `lore ingest --file` for the pattern authoring feedback loop. This document is kept for
> historical context about why the flag exists.

# Delta ingest requires committed changes for pattern testing

## Context

When authoring patterns, the natural workflow is to edit a file, run `lore ingest`, and test with
`lore search` to verify discoverability. However, delta ingest uses `git diff --name-status` between
the last-ingested commit and HEAD to detect changes. Files that have been modified but not committed
are invisible to this mechanism.

This creates friction during the pattern authoring feedback loop: edits do not appear in search
results until they are committed.

## Guidance

Commit pattern changes before running `lore ingest` to test discoverability. A temporary commit is
acceptable — amend or rewrite it after testing.

```sh
git add patterns/my-new-pattern.md
git commit -m "wip: draft pattern for review"
lore ingest
lore search "expected query terms" --top-k 3
```

If the pattern needs changes, edit it, amend the commit, and run `lore ingest` again:

```sh
# Edit the pattern file
git add patterns/my-new-pattern.md
git commit --amend --no-edit
lore ingest
lore search "expected query terms" --top-k 3
```

The alternative is `lore ingest --force`, which reads files from disk regardless of git state and
picks up uncommitted changes. However, it drops and recreates the entire FTS5 table and re-embeds
all files, which is slow on larger knowledge bases — particularly on x86 hardware.

## Why This Matters

Authors who do not know about this behaviour will assume their edits are immediately searchable
after running `lore ingest`. When the pattern does not appear in search results, they may
incorrectly diagnose a vocabulary coverage problem or a search engine issue when the real cause is
that delta ingest never saw the change.

## When to Apply

- Every time you test a pattern's discoverability during authoring
- When running the vocabulary coverage technique from the
  [Pattern Authoring Guide](../../pattern-authoring-guide.md)
- When debugging search results that seem to ignore recent pattern edits

## Examples

**Before (fails silently):**

```sh
# Edit patterns/rust/error-handling.md
lore ingest          # Delta mode: no committed changes detected, nothing indexed
lore search "anyhow" # Pattern not found — still indexed with the old content
```

**After (works correctly):**

```sh
# Edit patterns/rust/error-handling.md
git add patterns/rust/error-handling.md
git commit -m "wip: test vocabulary coverage"
lore ingest          # Delta mode: detects committed change, re-indexes the file
lore search "anyhow" # Pattern found with updated content
```

## Related

- [Pattern Authoring Guide](../../pattern-authoring-guide.md) — vocabulary coverage technique
  references this workflow
- ROADMAP: "Single-file ingest" (`lore ingest --file <path>`) is planned to eliminate the
  commit-before-test workaround
