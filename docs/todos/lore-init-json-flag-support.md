---
title: "lore init does not respect the global --json flag"
priority: P2
category: cli-readiness
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/main.rs:134-243
related_pr: feat/git-optional-knowledge-base
---

# lore init does not respect the global --json flag

## Context

The lore CLI declares `--json` as a global flag in the `Cli` struct. The `search` and `list`
subcommands honour it and emit structured JSON. The `init` subcommand silently ignores it:
`cmd_init` writes the same human-readable output to stderr regardless of whether `--json` is set.

An agent that runs `lore init --json --repo /tmp/patterns` expects machine output and gets prose.
The git advisory introduced in commit e6d59df is part of that prose, so an agent using `--json`
would also miss the "not a git repository" warning entirely.

## Proposed fix

1. Thread the `json` flag into `cmd_init`'s signature.
2. When `json: true`, emit a single JSON object on stdout at the end of the command instead of
   streaming prose to stderr. Suggested shape:

```json
{
  "status": "success" | "degraded" | "error",
  "config_path": "/Users/.../lore.toml",
  "knowledge_dir": "/path/to/patterns",
  "database_path": "/path/to/knowledge.db",
  "git_repository": false,
  "ingestion": {
    "files_processed": 12,
    "chunks_created": 47,
    "errors": []
  },
  "advisory": "not_a_git_repository" | null
}
```

3. When `--json` is set, suppress all stderr prose (matching the convention used by
   `lore search --json`).
4. The `advisory` field becomes the structured equivalent of the human notice; agents can branch on
   `advisory == "not_a_git_repository"` directly.

## Test surface

Add `init_json_against_plain_directory_emits_advisory` and
`init_json_against_git_repo_omits_advisory` in `tests/smoke.rs` mirroring the existing non-json
tests but parsing the stdout JSON.

## References

- CLI-readiness finding CLI-002 (confidence 0.90)
- Related: docs/todos/cmd-init-advisory-machine-marker.md (alternative approach for keyword
  matching, supersedes once --json lands)
