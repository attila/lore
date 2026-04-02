---
name: search-lore
description: Search the lore knowledge base for coding conventions and patterns relevant to the current task. Use when entering a new domain, creating new files, or when you need comprehensive pattern coverage beyond what PreToolUse hooks inject.
disable-model-invocation: true
user-invocable: true
---

# Search Lore Patterns

Search the lore knowledge base for all patterns relevant to $ARGUMENTS.

Use the `search_patterns` MCP tool to query lore. If the tool is not available, run
`lore search "$ARGUMENTS"` via Bash instead.

Apply ALL results as project conventions when writing code in this domain. These are the author's
strong coding preferences — follow them unless they conflict with explicit project-level
instructions.

If results include conventions for the language or framework you are working in, apply them to all
subsequent edits in this session.

The lore MCP server is bundled with this plugin and starts automatically.
