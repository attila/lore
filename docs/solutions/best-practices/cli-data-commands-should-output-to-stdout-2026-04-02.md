---
title: "CLI data commands should output to stdout, not stderr"
date: 2026-04-02
category: best-practices
module: cli
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Adding new CLI subcommands that output data (search results, lists, reports)"
  - "Building CLI tools consumed by AI agents or scripts"
  - "Refactoring existing commands to support programmatic consumption"
tags:
  - cli
  - stdout
  - stderr
  - agent-readiness
  - machine-readable
  - unix-conventions
---

# CLI data commands should output to stdout, not stderr

## Context

During the lore CLI development, `lore search` and `lore list` were implemented using `eprintln!()`
for all output — both status messages and data. This followed the pattern of other commands in the
codebase (like `lore init` and `lore ingest`) that use stderr for progress output. However, search
results and pattern listings are *data*, not diagnostics. Using stderr made the output invisible to
agent pipelines and standard Unix tools like `grep`, `jq`, and pipe chains.

## Guidance

**Data goes to stdout. Diagnostics go to stderr.**

```rust
// Status messages → stderr (for humans watching the terminal)
eprintln!("No results found.");
eprintln!("Searching...");

// Data output → stdout (for agents, scripts, and pipes)
println!("[1] {}", result.title);
println!("    source: {}", result.source_file);
```

The dividing line: if the output is the *purpose* of running the command (search results, pattern
listings, status reports), it belongs on stdout. If it's progress feedback, warnings, or errors that
help a human understand what happened, it belongs on stderr.

## Why This Matters

- **Agent consumption**: AI agents using Bash tool calls capture stdout. Output on stderr is
  invisible to the agent's pipeline and wastes the information.
- **Unix composability**: `lore search "rust" | grep "error"` only works when results are on stdout.
  With stderr, the pipe receives nothing.
- **Script integration**: `results=$(lore search "typescript")` captures stdout only. Stderr output
  requires explicit redirection (`2>&1`) which breaks the separation of data from diagnostics.

## When to Apply

- Any CLI command whose primary purpose is to *return data* to the caller
- Commands that agents or scripts might invoke programmatically
- Subcommands like `search`, `list`, `show`, `get`, `export`

Not applicable to:
- Commands whose purpose is to *perform an action* (like `init`, `ingest`) — their progress output
  is rightly on stderr
- Error messages and warnings (always stderr)

## Examples

**Before** (everything to stderr):
```rust
fn cmd_list(config_path: &Path) -> anyhow::Result<()> {
    let patterns = db.list_patterns()?;
    for p in &patterns {
        eprintln!("{}", p.title);  // invisible to pipes and agents
    }
    Ok(())
}
```

**After** (data to stdout, status to stderr):
```rust
fn cmd_list(config_path: &Path) -> anyhow::Result<()> {
    let patterns = db.list_patterns()?;
    for p in &patterns {
        println!("{}", p.title);  // visible to pipes and agents
    }
    Ok(())
}
```

## Related

- [Unix Philosophy: stdout vs stderr](https://en.wikipedia.org/wiki/Standard_streams)
- The `lore hook` subcommand correctly uses stdout for JSON output (designed for machine consumption
  from the start)
