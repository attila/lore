---
title: "Claude Code plugin assembly: hooks auto-loading, path resolution, and skill invocation"
date: 2026-04-02
category: integration-issues
module: integrations
problem_type: integration_issue
component: tooling
symptoms:
  - "Duplicate hooks error when loading plugin via /plugin command"
  - "MCP server not found when mcp.json placed inside .claude-plugin/"
  - "Permission denied errors and context inflation from auto-invocable skill"
root_cause: config_error
resolution_type: config_change
severity: high
tags:
  - claude-code
  - plugin
  - hooks
  - mcp
  - skill
  - auto-invocation
---

# Claude Code plugin assembly: hooks auto-loading, path resolution, and skill invocation

## Problem

Building a Claude Code plugin with hooks, MCP server config, and a skill surfaced three non-obvious
assembly rules that are poorly documented and cause silent or confusing failures.

## Symptoms

- Plugin loads with "duplicate hooks" error after adding hooks path to `plugin.json`
- MCP server fails to start with "server not found" when `mcp.json` is inside `.claude-plugin/`
- Skill triggers redundant MCP tool calls in don't-ask mode, causing permission denied errors and
  doubling token usage

## What Didn't Work

- Referencing `hooks/hooks.json` in `plugin.json` alongside having the file at the conventional
  location (Claude Code loads it twice)
- Placing `mcp.json` inside `.claude-plugin/` next to `plugin.json` (wrong resolution root)
- Leaving the skill as auto-invocable (`disable-model-invocation: false`) when hooks already inject
  the same patterns (redundant calls)

## Solution

Three rules for Claude Code plugin assembly:

**1. Do not reference hooks in plugin.json.**

Claude Code auto-loads `hooks/hooks.json` from the plugin root by convention. Explicitly referencing
it in `plugin.json` causes duplicate registration:

```json
// plugin.json — CORRECT
{
  "name": "lore",
  "version": "0.1.0",
  "description": "...",
  "skills": "./skills/",
  "mcpServers": "./mcp.json"
}
// No "hooks" field — auto-loaded from hooks/hooks.json
```

**2. Place mcp.json at plugin root, not inside .claude-plugin/.**

MCP server paths resolve relative to the plugin root directory (the parent of `.claude-plugin/`),
not relative to where `plugin.json` lives:

```
integrations/claude-code/
  .claude-plugin/
    plugin.json          # references "../mcp.json" or "./mcp.json"
  hooks/
    hooks.json           # auto-loaded
  mcp.json               # <-- HERE, at plugin root
  skills/
    search-lore/
      SKILL.md
```

**3. Disable auto-invocation when hooks handle injection.**

If PreToolUse hooks already inject patterns via `additionalContext`, an auto-invocable skill calling
the same MCP search tool doubles the work. Set `disable-model-invocation: true` in the skill's
YAML frontmatter to make it user-invocable only:

```yaml
---
name: search-lore
description: Search the lore knowledge base...
disable-model-invocation: true
user-invocable: true
---
```

## Why This Works

- **Hooks auto-loading** is a Claude Code convention to reduce plugin boilerplate. The plugin system
  scans for `hooks/hooks.json` at the plugin root automatically.
- **Path resolution** follows the plugin root (where the user points `--plugin-dir`), not the
  manifest subdirectory. This is consistent with how skills paths resolve.
- **Auto-invocation** is designed for skills that provide novel capabilities. When hooks already
  deterministically inject the same data, auto-invocation creates redundancy: the hook fires, then
  Claude also invokes the skill, producing duplicate context and permission prompts.

## Prevention

- Test plugin loading with `claude --plugin-dir <path>` and check the plugin page for errors before
  shipping
- Use `/reload-plugins` after config changes (restarting Claude is not necessary)
- When a plugin has both hooks and skills targeting the same data, default to
  `disable-model-invocation: true` on the skill
