---
title: "lore_status tool name uses project prefix instead of domain prefix"
priority: P3
category: maintainability
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/server.rs:284-296
  - src/server.rs:handle_lore_status
  - src/snapshots/lore__server__tests__tools_list_returns_all_five_tools.snap
  - README.md (MCP Tools table)
related_pr: feat/git-optional-knowledge-base
---

# lore_status tool name uses project prefix instead of domain prefix

## Context

The new MCP tool is named `lore_status`. The other tools follow a domain-action pattern:

- `search_patterns`
- `add_pattern`
- `update_pattern`
- `append_to_pattern`

`lore_status` breaks this pattern by leading with the project name. A more consistent name would
centre the domain (the knowledge base) rather than the implementing tool:

- `knowledge_base_status`
- `pattern_base_status`
- `kb_status` (abbreviated)

The lore project is in active development with no external MCP consumers, so renaming is cheap.

## Proposed fix

Pick a new name. Recommended: `knowledge_base_status`. Rationale:

- Centred on the domain ("knowledge base") rather than the tool
- Plural-noun-status pattern matches `search_patterns` (verb-noun) by being noun-action
- "kb_status" is too cryptic for an agent prompt; full word is clearer

If renamed, update:

1. `src/server.rs:285` — tool definition `name` field
2. `src/server.rs:dispatch` — match arm in `handle_tool_call`
3. `src/server.rs:tests` — three test functions reference the name in JSON-RPC bodies
4. `src/snapshots/lore__server__tests__tools_list_returns_all_five_tools.snap`
5. `src/main.rs:cmd_init` advisory text (mentions `lore_status` by name)
6. `src/hook.rs:format_session_context` advisory text
7. `README.md` MCP Tools table
8. `docs/todos/*.md` files in this directory that reference the name

Counter-argument for keeping `lore_status`:

- It mirrors the existing CLI command `lore status`, which strengthens the user-agent parity story
  (see `cmd-status-cli-mcp-parity.md`)
- Agents discovering tools by introspection will see "lore_status" and may intuit that it relates to
  the lore tool itself

If you decide to keep `lore_status`, document the rationale in a code comment near the tool
definition so future contributors don't mistakenly rename it.

## References

- Maintainability finding (confidence 0.62)
- Agent-native finding: naming convention drift
- Related: docs/todos/cmd-status-cli-mcp-parity.md (the CLI parity case for keeping the
  `lore_status` name)
