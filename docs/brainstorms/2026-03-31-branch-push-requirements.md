---
date: 2026-03-31
topic: branch-push
---

# Branch Push for Agent Submissions

## Problem Frame

Lore's pattern repo uses trunk-based development where a human pushes edits directly. When an agent
submits patterns via MCP tools (`add_pattern`, `update_pattern`, `append_to_pattern`), those commits
land on whatever branch is checked out — typically trunk. This mixes untrusted agent content with
curated human content without any review gate.

The human needs an inbox: agent submissions go to per-submission branches, get pushed to the remote,
and wait for human review before being curated into trunk.

```
Agent submits pattern
        │
        ▼
fork from HEAD ──► inbox/<slug>  ──► push to origin
                   (one commit)

Agent submits another pattern
        │
        ▼
fork from HEAD ──► inbox/<slug>  ──► push to origin
                   (one commit)

        Human reviews each branch independently,
        cherry-picks worthy ones into trunk.
```

## Requirements

**Configuration**

- R1. A new optional config field `inbox_branch_prefix` specifies the branch name prefix for agent
  submissions (e.g., `"inbox/"`). When absent, current behavior is preserved (commit locally on the
  checked-out branch, index locally, no push).

**Git workflow**

- R2. When configured, all three write operations (`add_pattern`, `update_pattern`,
  `append_to_pattern`) use the branch push flow.
- R3. Each submission creates a uniquely-named branch forked from HEAD using git plumbing (object
  creation and ref update) without switching the working tree or HEAD. The human's checkout is never
  disturbed.
- R4. For `update_pattern` and `append_to_pattern`: read the file content from the currently
  checked-out branch (HEAD). Modifications to files that only exist in previous inbox branches (not
  yet curated to trunk) are not supported.
- R5. After committing, push the per-submission branch to the remote. Push failure is a hard error —
  the MCP response must report the failure. The local branch ref is left in place for potential
  retry.

**Branch naming**

- R6. Branch names follow the pattern `<prefix><slug>`, where prefix comes from config and slug is
  derived from the operation (e.g., the pattern title for `add_pattern`, the source file stem for
  `update_pattern` / `append_to_pattern`).
- R7. If a branch with that name already exists locally, append a short disambiguator (e.g.,
  timestamp or counter) to avoid collisions.

**Indexing**

- R8. When branch push is active, the submitted content is NOT indexed into the local SQLite
  database. The content exists only on the per-submission branch ref, not in the working tree, so
  indexing it would create incoherent search results.
- R9. When branch push is not configured, indexing works as today.
- R10. Content becomes searchable after the human curates it into trunk and triggers a re-index
  (existing `lore init` / ingest flow).

**MCP response**

- R11. The MCP tool response must clearly indicate that the pattern was pushed to a named branch and
  is pending review, distinguishing this from the normal "committed locally" response.

## Success Criteria

- Agent can add, update, and append patterns that land on uniquely-named remote branches without
  affecting the local working tree or HEAD.
- When the config field is absent, behavior is identical to today (commit locally, index locally, no
  push).
- The human's checkout remains undisturbed throughout the operation.
- Concurrent sessions (worktrees, multiple clones, different machines) can all push submissions
  without races or data loss.

## Scope Boundaries

- No PR creation — branches are a simple inbox, not a GitHub workflow.
- No building on previous inbox submissions — update/append only works on files that exist on the
  checked-out branch (trunk).
- No local indexing of inbox content (deferred to a future "use immediately" mode if needed).
- No automatic cleanup of merged inbox branches.
- Cherry-pick conflicts are the human's responsibility.

## Key Decisions

- **Per-submission branches, not a single long-lived branch:** Each write operation creates its own
  branch forked from HEAD. This eliminates all concurrency issues (no shared ref to race on) at the
  cost of branch proliferation. The human reviews and cleans up branches, similar to a PR workflow.
- **Git plumbing, not checkout:** Commits are created directly on the per-submission branch ref
  using git object/ref commands. The working tree and HEAD are never changed.
- **Agent is submitter, not consumer:** Agent does not need to search its own inbox submissions. The
  agent must remember `source_file` paths from add responses if it wants to update/append in the
  same session — search will not find inbox content.
- **Always fork from HEAD:** Each submission starts from the current trunk state. No need to resolve
  or build on previous inbox branches.
- **Push failure is a hard error:** The agent is told the push failed. The commit remains on the
  local branch ref. No silent swallowing.
- **Presence of config field is the toggle:** `inbox_branch_prefix` being present enables inbox
  mode; absent means current behavior. No separate boolean flag needed.

## Outstanding Questions

### Deferred to Planning

- [Affects R3][Needs research] Exact git plumbing sequence for creating a commit on a branch ref
  without checkout.
- [Affects R1][Technical] Exact config field placement in lore.toml (e.g.,
  `[git] inbox_branch_prefix = "inbox/"`).
- [Affects R5][Technical] Which remote to push to. Presumably `origin`, but should be verified or
  configurable.
- [Affects R5][Technical] Push requires non-interactive authentication (SSH key or credential
  helper). Document as a prerequisite.
- [Affects R7][Technical] Disambiguator strategy when branch name collides (timestamp suffix,
  counter, or short hash).
- [Affects R6][Technical] Slug derivation for update/append operations — use source file stem
  without directory prefix.

## Next Steps

→ `/ce:plan` for structured implementation planning
