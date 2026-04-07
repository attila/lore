# lore — Claude Code plugin

Bundles the lore MCP server, lifecycle hooks, and user-invocable skills into a single Claude Code
plugin. See `../README.md` and `../../README.md` for the broader project overview.

## Skill naming convention

Skills inside this plugin are named by their function alone, with no `lore-` prefix. Claude Code's
plugin namespace handles disambiguation as `/lore:<skill-name>` (or just `/<skill-name>` when there
is no collision with another installed plugin).

For example, the search skill lives at `skills/search/SKILL.md` and is addressable as `/lore:search`
or, when unambiguous, `/search`. New skills follow the same shape — pick a function-shaped bare name
(`coverage-check`, not `lore-coverage-check`) and let the plugin namespace carry the provenance.

This convention is documented here so future skill authors do not reach for the redundant `lore-`
prefix.
