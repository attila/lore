---
title: "lore_status is only mentioned in the SessionStart prompt for non-git repos"
priority: P2
category: agent-native
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/hook.rs:format_session_context
  - integrations/claude-code/skills/search-lore/SKILL.md
related_pr: feat/git-optional-knowledge-base
---

# lore_status is only mentioned in the SessionStart prompt for non-git repos

## Context

Commit `dcda0a3` (Advertise git status in SessionStart and PostCompact context) added a conditional
advisory paragraph to `format_session_context` that fires only when the knowledge base is not a git
repository. The paragraph mentions the new `lore_status` MCP tool by name:

> Use the lore_status tool to inspect this state at any time.

For agents starting in a git-enabled knowledge base — the common case — this paragraph never
appears. The agent can still discover `lore_status` by reading the `tools/list` response, but
explicit prompt mention is a stronger signal than introspection.

The result is asymmetric agent context:

- Non-git knowledge base: agent learns `lore_status` exists, learns why it matters, has a clear use
  case
- Git knowledge base: agent must introspect tools/list, read the description, and decide on its own
  when to call it

## Proposed fix

Pick one of:

1. **Mention `lore_status` unconditionally in the SessionStart context** as a "see also" line,
   regardless of git state:

   ```
   Available patterns:
   - ...

   Tip: call the lore_status tool to inspect knowledge base health
   (git status, indexed counts, last ingested commit) before write operations.
   ```

2. **Add `lore_status` to the search-lore skill description** at
   `integrations/claude-code/skills/search-lore/SKILL.md` so agents using the skill see it. This
   couples the skill to the new tool but avoids polluting every SessionStart context with extra
   tokens.

3. **Both.** Modest token cost, maximum discoverability.

The recommended approach is option 1 — the line is short (~20 tokens) and the SessionStart context
is the canonical agent onboarding surface.

## Test surface

If option 1 is chosen, update `tests/hook.rs::hook_session_start_returns_meta_instruction` to assert
the new tip is present, and add a new test for the git-initialised case to confirm the tip appears
regardless of git state.

## References

- Agent-native finding (confidence 0.80): asymmetric discovery
