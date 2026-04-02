---
name: search-lore
description: Search the lore knowledge base for coding conventions and patterns relevant to the current task. Use when entering a new domain, creating new files, or when you need comprehensive pattern coverage beyond what PreToolUse hooks inject.
disable-model-invocation: false
user-invocable: true
---

# Search Lore Patterns

Search the lore knowledge base for all patterns relevant to $ARGUMENTS.

Use the `search_patterns` MCP tool to query lore. If the tool is not available, run
`lore search "$ARGUMENTS"` via Bash instead.

Treat ALL results as binding constraints when writing code in this domain. These are the project's
coding conventions — follow them exactly.

If results include conventions for the language or framework you are working in, apply them to all
subsequent edits in this session.

Prerequisite: the lore MCP server must be configured separately (via `lore init` or
`claude mcp add`). This skill queries the server but does not start it.
