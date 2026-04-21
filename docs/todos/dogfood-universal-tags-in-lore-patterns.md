---
title: "Dogfood universal-patterns in lore-patterns: tag two, trim one, split one"
priority: P3
category: dogfooding
status: ready
created: 2026-04-21
source: ce-review follow-up (feat/universal-patterns)
files:
  - lore-patterns/workflows/git-branch-pr.md
  - lore-patterns/agents/unattended-work.md
related_pr: feat/universal-patterns
---

# Dogfood universal-patterns in lore-patterns

Brief for a session launched at `/srv/misc/Projects/repos/lore-patterns/`. After PR #33 on `lore`
ships the `universal` tag feature (always-on injection at SessionStart + PreToolUse dedup bypass),
only `workflows/git-branch-pr.md` carries the tag — added ad-hoc during smoke-testing. This brief
turns that ad-hoc state into a deliberate authoring decision.

## Context

The universal-patterns feature (lore PR #33, branch `feat/universal-patterns`) adds a `universal`
frontmatter tag. Tagged patterns:

- emit in full at every `SessionStart` and `PostCompact` under a `## Pinned conventions` section;
- bypass `PreToolUse` deduplication, so they re-inject on every relevant tool call (relevance gate
  still applies);
- are additive beyond `top_k`.

Constraints baked in:

- **8 KB per-file hard cap** at ingest (rejected if exceeded)
- **32 KB total render cap** at SessionStart (truncates with a visible marker)
- **>3 patterns** emits a soft `Note: N patterns tagged universal` advisory
- **>1 KB per chunk** emits a soft per-pattern body-size advisory

## Decision

Tag exactly two patterns. Trim one. Split one.

### Tag `universal`

1. **`workflows/git-branch-pr.md`** — already tagged (added during smoke test). The motivating
   pattern for the feature. Push discipline, branch naming, merge ownership, rebase-before-push —
   all process-level rules that sessions currently forget mid-run.

2. **`agents/unattended-work.md`** — add the `universal` tag. Proved itself live during the `lore`
   PR smoke: it fired on a composite Bash command and the agent correctly recovered. "Avoid
   composite shell commands", `--body-file` over heredocs, and the `EnterWorktree`-over-`cd &&`
   discipline are exactly the kind of process-wide rules that benefit from every-tool-call
   reinforcement.

Leave `documentation/terminology-standards.md` relevance-tagged — the rules are short enough to
stick after one injection per session, and dedup fires correctly. Revisit only if
"dedup"/"docs"/American-English slips into prose repeatedly.

Leave everything else relevance-tagged. `rust/*`, `javascript-typescript/*`, `ci/*`, `cli/*`,
`yaml/*`, `workflows/atlassian-mcp.md` are all language- or domain-specific; the relevance gate +
session dedup handle them well.

### Trim `workflows/git-branch-pr.md`

Current size ~5 KB. Two sections apply only when writing a PR, not on every git-adjacent tool call.
Extract them to keep the universal file tight:

- **`### Marking dependencies in PR descriptions`** (~400 B) — move to a new
  `workflows/pr-description-templates.md` (no `universal` tag).
- **PR-body formatting bullets under `## Pull requests`** — specifically the checkbox style,
  SHA-pinned blob URLs, "respect existing templates" line. Move to the same new file.

Keep in the universal file the operational decisions that benefit from continuous reinforcement:

- Feature branches only, never push to `main`
- Branch-name prefixes
- Conventional commit message types (short list)
- Draft-PR-first + owner-merges rule
- `git push origin HEAD`, rebase-before-every-push

Expected final size: ~4 KB.

### Keep `agents/unattended-work.md` as-is

1.8 KB, every sentence load-bearing. No split or trim needed.

## Plan

Work in `/srv/misc/Projects/repos/lore-patterns/` on a feature branch
(`feat/universal-tag-curation-2026-04-21` or similar).

1. Read `workflows/git-branch-pr.md` end-to-end and identify the two extraction targets above.
2. Create `workflows/pr-description-templates.md` with the extracted content and its own frontmatter
   (tags: `pull-request, github, template,
   dependencies` — no `universal`).
3. Edit `workflows/git-branch-pr.md` to remove the extracted sections, preserving operational rules.
4. Edit `agents/unattended-work.md` frontmatter to add `universal` to the tag list.
5. Run `lore ingest --file <each changed file>` to land the edits in the DB without a full rebuild.
   Three invocations.
6. Verify:
   - `lore list | grep universal` → expect exactly two rows: "Workflow conventions" and "Unattended
     Work".
   - `lore status` → confirm no advisories beyond expected; no oversized body, no near-miss tag.
   - Via MCP: `lore_status` with `include_metadata: true` and inspect the `universal_advisories`
     fence — `universal_count: 2`, `count_warning:
     false`, empty `oversized_bodies` and
     `near_miss_tags`.
7. `/exit` and relaunch Claude Code, confirm the `## Pinned conventions` section in the new session
   carries both patterns and renders under ~7 KB.
8. Single commit on the feature branch. Suggested message:

   ```
   refactor: curate universal tag — keep two, trim one, split one

   Process-wide conventions (push discipline + unattended-session
   command hygiene) stay always-on; PR-description formatting moves
   to a sibling relevance-tagged file so the universal footprint
   stays tight.
   ```

9. Open a draft PR per workflow/git-branch-pr.md conventions.

## Why this is worth doing

Dogfooding the feature reveals what's hard-to-get-right about pattern authoring at this tier. Two
likely learnings:

1. **The 8 KB per-file cap forces editorial discipline.** Patterns that grow naturally over time
   (multiple contributors, accreted examples) will bump against it. The cap is productive pressure.
2. **Splitting universal-vs-relevance content requires distinguishing "rules that benefit from
   constant reinforcement" from "formatting details you reach for when producing a specific
   output."** This is a useful editorial distinction that generalises beyond lore-patterns.

Both learnings should be added to the lore pattern-authoring guide after this experiment, probably
as a "When to split universal from relevance" subsection.

## When to do this

Pick up immediately after PR #33 merges. Before the next new universal- worthy pattern is authored,
so the tagging discipline is set by example.

## References

- PR that ships the feature: https://github.com/attila/lore/pull/33
- Authoring guidance: `docs/pattern-authoring-guide.md` §"When to use the universal tag" in the
  `lore` repo (ships with the PR).
- Feature constraints summary: `docs/plans/2026-04-20-001-feat-universal-patterns-plan.md` in the
  `lore` repo.
