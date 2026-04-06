---
title: "cmd_init advisory has no machine-parseable marker"
priority: P3
category: cli-readiness
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/main.rs:152-158
related_pr: feat/git-optional-knowledge-base
---

# cmd_init advisory has no machine-parseable marker

## Context

The advisory printed by `cmd_init` is plain prose:

```
Note: /path/to/dir is not a git repository.
  Lore will work, but delta ingest, the inbox branch workflow, and version
  history will be unavailable. Run `git init` in this directory to enable
  them. See docs/configuration.md#git-integration for details.
```

Agents that detect this advisory must keyword-match phrases like "is not a git repository" or "delta
ingest". The smoke test (`init_against_plain_directory_emits_git_advisory`) pins three keywords, but
the assertion is fragile against future wording changes.

A structured prefix would let agents reliably detect the advisory without prose parsing.

## Proposed fix

Pick one of:

1. **Add a structured prefix to the first line:**

   ```
   [lore:advisory:not-a-git-repository] /path/to/dir is not a git repository.
     Lore will work, but delta ingest...
   ```

   Agents can grep for `[lore:advisory:` reliably. The bracket prefix is also distinct from normal
   log output.

2. **Implement `lore init --json` (see lore-init-json-flag-support.md).** When `--json` is set, the
   advisory becomes a structured field (`"advisory": "not_a_git_repository"`) and the prose-marker
   problem disappears for json consumers. Agents using prose mode still need a marker or the JSON
   path.

3. **Both.** The structured prefix helps prose consumers; the JSON output helps agents that opt into
   machine mode. Modest cost.

The recommended approach is option 2 alone — the JSON flag is a more general solution and supersedes
the marker need. Option 1 alone is acceptable as a short-term improvement if the JSON work is
deferred.

## Considerations

- The current advisory text is stable and well-tested. This change is not blocking.
- Adding a marker changes the user-visible output for terminal users. Brackets in stderr are not
  unusual but should be tested for visual clarity.
- Once a marker is committed, removing it later is a contract break for agents that depend on it.

## References

- CLI-readiness finding CLI-004 (confidence 0.75)
- Related: docs/todos/lore-init-json-flag-support.md (the alternative solution that supersedes this
  once landed)
