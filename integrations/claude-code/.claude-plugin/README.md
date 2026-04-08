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

## Author prompt format (house style)

Skills that prompt the author for a decision MUST use a lettered option block so replies are
single-letter and unambiguous. The canonical format:

```
Options:

A: <imperative verb phrase>
B: <imperative verb phrase>
C: <imperative verb phrase>
```

Rules:

- Start the block with a single `Options:` label on its own line.
- Use consecutive uppercase letters from `A`. No numbered lists, no `y/N`, no free-form prose
  choices.
- Each option is a single imperative verb phrase.
- If an option needs a payload (list of numbers, a corrected list), accept it on the same reply when
  the author prefixes the payload with the letter, e.g. `B: 1, 3, 4`. If the letter arrives without
  the payload, prompt once more; never guess a default.
- No footer explaining how to reply — the format is self-evident.
- Any numbered list that feeds into a letter choice (menus, edit suggestions, tool-call lists) is
  rendered **above** the `Options:` block, not mixed into it. The numbered list is the menu; the
  lettered block is the decision.

See `skills/coverage-check/SKILL.md` for worked examples at steps 4, 5, and 14.
