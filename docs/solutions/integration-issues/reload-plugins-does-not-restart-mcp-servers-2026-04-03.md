---
title: "/reload-plugins does not restart plugin MCP server processes"
date: 2026-04-03
category: integration-issues
module: integrations
problem_type: workflow_issue
component: tooling
severity: medium
applies_when:
  - "Installing an updated binary that provides an MCP server via a Claude Code plugin"
  - "Testing MCP tool behavior after code changes during iterative development"
  - "Expecting /reload-plugins to refresh all plugin components including MCP servers"
tags:
  - claude-code
  - mcp-server
  - plugin
  - reload
  - process-lifecycle
  - development-workflow
---

# /reload-plugins does not restart plugin MCP server processes

## Context

During iterative development of the lore plugin, we fixed an FTS5 crash in `sanitize_fts_query()`,
installed the new binary via `just install`, and ran `/reload-plugins` to pick up the fix. The
command reported "1 plugin MCP server" reloaded, suggesting the MCP server was refreshed. Testing
revealed an asymmetry: hooks worked with the new binary but the MCP server still ran the old one.

The lore plugin registers two integration paths:

- **Hooks** (`hooks/hooks.json`) — `lore hook` invoked as a fresh process per event
- **MCP server** (`mcp.json` via `plugin.json`) — `lore serve` as a long-running stdio process

## Guidance

**After updating a binary that provides an MCP server, restart the Claude Code session to pick up
the new binary for MCP tools.** `/reload-plugins` refreshes plugin configuration and hook bindings
but does not terminate and respawn MCP server processes.

For iterative development, use this workflow:

1. `just install` — build and install the updated binary
2. Test via **hooks** immediately — each hook invocation forks a fresh process from PATH
3. Test via **CLI** immediately — `lore search "query"` also uses the new binary
4. To test **MCP tools** — `/exit` then relaunch Claude Code

## Why This Matters

The asymmetry between hooks and MCP tools creates confusing failures. A fix that works via hooks
(PreToolUse injection) and CLI (`lore search`) can still fail via MCP tools (`search_patterns`)
because the MCP server process was started with the old binary. The `/reload-plugins` output
reporting "1 plugin MCP server" reloaded is misleading — it appears to confirm the server was
refreshed when it was not.

## When to Apply

- After running `just install` or any binary update when the plugin provides an MCP server
- When MCP tools return errors that the CLI does not reproduce
- When hooks work correctly but MCP tools fail after a code change
- When debugging "stale behavior" in MCP tools after deploying a fix

## Examples

**Confirmed test (2026-04-03):**

```
# Install fixed binary
$ just install

# Hook test — WORKS (new binary, fresh process)
$ echo '{"hook_event_name":"PreToolUse",...}' | lore hook
→ Returns results (no crash)

# CLI test — WORKS (new binary on PATH)
$ lore search "pre-commit hook" --top-k 3
→ Returns results (no crash)

# /reload-plugins — MISLEADING
> /reload-plugins
→ "Reloaded: ... 1 plugin MCP server"

# MCP tool test — FAILS (old binary still running)
> search_patterns("pre-commit hook dprint formatting")
→ MCP error -32000: Search failed: no such column: commit

# Full session restart — WORKS
> /exit
$ claude
> search_patterns("pre-commit hook dprint formatting")
→ Returns results (no crash)
```

## Related

- [Claude Code plugin assembly pitfalls](claude-code-plugin-assembly-pitfalls-2026-04-02.md) —
  covers plugin config structure; this doc covers runtime lifecycle
- [additionalContext timing in PreToolUse hooks](additional-context-timing-in-pretooluse-hooks-2026-04-02.md)
  — covers hook execution model
