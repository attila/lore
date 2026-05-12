# Lore — local conventions

## CHANGELOG entries

Every `[Unreleased]` bullet is **one assertive-voice sentence** ending in `(#N)`. Lead with the
subject of the change (the CLI, the function, the config knob, the data model) and the verb users
experience.

**Example:**

> `lore ingest` skips non-UTF-8 filenames during the directory walk and surfaces them on
> `IngestResult::errors` instead of indexing them under U+FFFD-substituted keys. (#46)

Do **not** put in entries: implementer rationale (`Cow::Owned`, signature decisions), CLI behaviour
ladder tier callouts, brainstorm or plan provenance, side-effect chains, or accounting fixes that
ride along. Those live in the PR body, plan, or origin brainstorm — not duplicated here.

Architectural events can earn a second sentence: schema bumps requiring migration, breaking API
surface changes, dependency changes with security implications. The `### Notes` block is the right
home for release-wide caveats (one-way schema bumps, etc.).

**Why:** drafting entries while implementation is fresh conflates user-facing change with
implementer rationale, producing 10+ line entries that need rewriting at release-prep. Cheaper to
keep tight on every entry; the next `just release-prep` rotates a tight section into versioned
history without context recall.

This rule complements (does not replace) the user-facing-only rule already captured in project
memory at
`~/.claude/projects/-srv-misc-Projects-lore/memory/feedback_changelog_user_facing_only.md`.
