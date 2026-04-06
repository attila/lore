---
title: "Filter changes in delta pipelines need bidirectional reconciliation"
date: 2026-04-06
category: best-practices
module: ingest
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Designing a delta-driven update pipeline that consumes a filter file (.gitignore, .loreignore, .npmignore, allowlist, etc.)"
  - "Adding an exclusion mechanism to an existing system that has incremental updates"
  - "Reviewing a documented v1 limitation that says removing X requires a full rebuild"
  - "Building a search index, cache, or derived store that depends on a user-controlled allow/deny rule"
tags:
  - ingest
  - delta
  - reconciliation
  - filter
  - bidirectional
  - design-pattern
---

# Filter changes in delta pipelines need bidirectional reconciliation

## Context

When you bolt a filter rule (`.loreignore`, `.gitignore`, an allowlist, a tag-based exclusion) onto
a delta-driven update pipeline, the obvious reconciliation is one-directional: when the filter
changes, walk the database and remove anything that the new filter now matches. The lore project
shipped exactly this design for `.loreignore` v1 and documented the half it left out as a "v1
limitation":

> Removing a pattern from `.loreignore` does not automatically re-index the previously excluded
> files. Run `lore ingest --force` to bring them back into the index.

The justification at the time was that the missing direction (re-index files newly allowed by the
filter) required walking the filesystem during a delta ingest, which broke the mental model of
"delta ingest is driven by `git diff` only". The cost of doing the extra walk was framed as
non-trivial, and the workaround (`lore ingest --force`) was framed as acceptable.

The first user to actually exercise the feature found the limitation confusing within minutes. Their
question after a successful add-then-remove cycle was, verbatim:

> Question: explain why we need to force ingest when we drop a previously ignored doc from the
> ignore file? Why can't that update be cumulative?

There is no good answer. The limitation is the wrong default.

## Guidance

When a delta-driven update pipeline gains a filter rule, design reconciliation **bidirectionally
from the start**. Both directions must run whenever the filter changes:

1. **Removal pass.** For every entry currently in the derived store, check whether the new filter
   rejects it. If yes, delete it. (This is the obvious half.)
2. **Addition pass.** For every source on disk, check whether it is no longer rejected by the filter
   and is missing from the derived store. If both, re-index it. (This is the half people forget.)

Both passes consult the same filter snapshot. Run them in a fixed order (removal first, then
addition) so that files just removed in pass 1 are correctly excluded from pass 2 by an existing
in-memory snapshot of the pre-pass-1 store.

Detection of "filter changed" should compare a content hash of the filter file against a stored
hash, so the reconciliation pass only runs when the filter has actually changed — not on every delta
update.

```rust
// Pseudocode for the bidirectional reconciliation pass.
fn reconcile(db: &Db, walk: &dyn Walker, filter: &Filter) -> Result<Stats> {
    let mut stats = Stats::default();
    let db_snapshot = db.all_sources()?; // taken BEFORE pass 1 mutates anything

    // Pass 1: removals
    for source in &db_snapshot {
        if filter.rejects(source) {
            db.delete(source)?;
            stats.removed += 1;
        }
    }

    // Pass 2: additions (uses db_snapshot, not the post-pass-1 state, so we
    // do not try to re-add things we just removed)
    for source in walk.iter().filter(|s| !filter.rejects(s)) {
        if !db_snapshot.contains(source) {
            db.index(source)?;
            stats.added += 1;
        }
    }

    Ok(stats)
}
```

The CLI summary should report both counts so users see what happened in both directions:

```
Done (delta): 1 reconciled (removed), 2 reconciled (re-indexed)
```

## Why This Matters

A one-directional reconciliation makes a filter rule feel like a one-way ratchet: every time you add
a pattern, the index shrinks; you can never grow it back without a full rebuild. Users discover this
the second time they edit the filter, not the first, because the first edit usually only adds
exclusions. By the second edit, they have built a mental model of "this thing is incremental" and
the asymmetry surprises them.

Three concrete ways the asymmetry hurts:

1. **Compound edits silently lose work.** A single filter edit that both adds and removes patterns
   (e.g. swap one exclusion for another) only executes the destructive half. The user does not
   notice until they go looking for one of the un-ignored files in search results.
2. **The v1 limitation document is the wrong shape.** Documenting a workaround does not make the
   workaround obvious. The user reading the docs after they hit the surprise has to scroll back to
   find the caveat, recognise it as their problem, and run the workaround. Most won't.
3. **The "extra cost" framing was wrong.** The actual cost of pass 2 is one `WalkDir` of the
   knowledge directory — which the full ingest path already does on every run. For the delta path it
   only runs when the filter has changed, which is rare. The cost is bounded and the benefit is "the
   feature behaves intuitively."

## When to Apply

- **Always**, when designing a new filter rule for any incremental update pipeline. There is no v0
  of this pattern that is correct as one-directional.
- When you find yourself writing a "v1 limitation" document for a feature where one direction is
  cheap and the other is "we'll get to it." That is a sign the feature is not done.
- When user feedback on a feature includes the phrase "why can't that just be cumulative?"

## Examples

### Lore: `.loreignore` reconciliation

**v1 (one-directional, wrong):** the original `reconcile_ignored` walked `db.source_files()` and
removed anything that matched the current matcher. Removing a pattern from `.loreignore` was a
no-op. Documented as a v1 limitation.

**v2 (bidirectional, correct):** the same function additionally walks `knowledge_dir` and re-indexes
any markdown file that is missing from the database snapshot taken before the removal pass.
Implementation in [`src/ingest.rs::reconcile_ignored`](../../../src/ingest.rs).

The diff that introduced bidirectional reconciliation also removed three pieces of documentation:

- The "v1 limitation" caveat in `docs/pattern-authoring-guide.md`
- The matching note in `docs/configuration.md`
- The misleading `Note: if you removed exclusions from .loreignore...` stderr output that fired on
  every filter change

When the limitation is gone, every breadcrumb that explained it should also disappear.

### Generalisation

| System                               | One-directional half                        | Missing half                                       | Failure mode                       |
| ------------------------------------ | ------------------------------------------- | -------------------------------------------------- | ---------------------------------- |
| `.loreignore` (lore)                 | Remove indexed files matched by new pattern | Re-index unmatched files no longer in DB           | Removed exclusions never come back |
| `.gitignore` in a custom build cache | Drop cached artefacts now ignored           | Re-cache files no longer ignored                   | Cache permanently shrinks          |
| Tag-based search index exclusion     | Delete documents tagged for exclusion       | Re-index documents whose exclusion tag was removed | Untagging looks broken             |
| Allowlist for sync                   | Stop syncing files no longer in allowlist   | Sync newly-allowed files                           | Allowlist additions silently no-op |

## Related

- [`delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md`](./delta-ingest-requires-committed-changes-for-pattern-testing-2026-04-05.md)
  — sibling lesson about the lore delta ingest pipeline
- ROADMAP entry on "Universal patterns via tag-based SessionStart injection" — captures a related
  but distinct failure mode where session deduplication, not reconciliation asymmetry, suppresses
  meta-rules
- The lore PR that fixed this:
  [`docs/plans/2026-04-06-001-feat-loreignore-plan.md`](../../plans/2026-04-06-001-feat-loreignore-plan.md)
