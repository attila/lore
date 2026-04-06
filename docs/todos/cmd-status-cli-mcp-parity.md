---
title: "lore status CLI lacks git_repository / delta_ingest_available fields"
priority: P2
category: cli-readiness
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/main.rs:465-519
related_pr: feat/git-optional-knowledge-base
---

# lore status CLI lacks git_repository / delta_ingest_available fields

## Context

The branch added a `lore_status` MCP tool that exposes:

- `git_repository: bool`
- `delta_ingest_available: bool`
- `inbox_workflow_configured: bool`
- `last_ingested_commit: string | null`
- `chunks_indexed: number | null`
- `sources_indexed: number | null`

The existing CLI `lore status` command (`cmd_status` in `src/main.rs:465-519`) predates this work
and only reports config path, knowledge_dir, database path, bind, Ollama health, model availability,
sqlite-vec status, chunks, sources, and last commit (when present).

It does NOT report whether the knowledge base is a git repository or whether delta ingest is
available. A terminal user who runs `lore status` after seeing the cmd_init advisory has no CLI
command to verify they fixed the problem. They would have to either:

- Read the source markdown files and check for `.git/`
- Run `lore serve` and call `lore_status` over MCP from another tool
- Trust the absence of the cmd_init advisory on the next `lore init`

None of these are great. The CLI should provide parity with the MCP tool.

## Proposed fix

Update `cmd_status` to include git status and delta ingest availability:

```rust
fn cmd_status(config_path: &Path) -> anyhow::Result<()> {
    // ... existing prelude

    eprintln!("=== lore status ===\n");
    eprintln!("  Config:       {}", config_path.display());
    eprintln!("  Knowledge:    {}", config.knowledge_dir.display());
    eprintln!("  Database:     {}", config.database.display());
    eprintln!("  Bind:         {}", config.bind);
    eprintln!();

    let is_git = git::is_git_repo(&config.knowledge_dir);
    eprintln!("  Git repo:     {}", if is_git { "✓ yes" } else { "✗ no" });

    // ... existing Ollama / model / sqlite-vec block ...

    if let Ok(db) = KnowledgeDB::open(&config.database, ollama.dimensions())
        && db.init().is_ok()
        && let Ok(stats) = db.stats()
    {
        // ... existing chunks / sources / last commit block ...

        let last_commit = db
            .get_metadata(crate::ingest::META_LAST_COMMIT)
            .ok()
            .flatten();
        let delta_available = is_git && last_commit.is_some();
        eprintln!(
            "  Delta ingest: {}",
            if delta_available {
                "✓ available"
            } else {
                "✗ unavailable"
            }
        );
    }
    Ok(())
}
```

Also consider: should `cmd_status` honour the global `--json` flag and emit the same structured
shape that the `lore_status` MCP tool returns? That would give CLI and MCP exact parity. See also
`docs/todos/lore-init-json-flag-support.md` for the parallel `lore init` work — both tasks might
land together.

## References

- CLI-readiness finding CLI-003 (confidence 0.80)
- Related: docs/todos/lore-init-json-flag-support.md (matching --json work for the init command)
