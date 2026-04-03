---
title: Suppress stderr diagnostics when --json mode is active
date: "2026-04-03"
category: best-practices
module: cli
problem_type: best_practice
component: tooling
severity: low
applies_when:
  - CLI command supports both human-readable and structured (JSON) output modes
  - A --json or --format flag changes the output contract from human to machine
  - Code paths write diagnostic or courtesy messages to stderr
tags:
  - cli
  - json-output
  - stderr
  - structured-output
  - machine-consumer
---

# Suppress stderr diagnostics when --json mode is active

## Context

The `lore` CLI tool added a global `--json` flag for `search` and `list` commands. The initial
implementation unconditionally printed "No results found." to stderr even when `--json` was active.
Code review flagged this: when `--json` is active, the caller is signaling "I am a machine consumer"
— human courtesy messages on stderr pollute logs, confuse pipelines, and violate the principle of
least surprise.

The existing project convention
([cli-data-commands-should-output-to-stdout](cli-data-commands-should-output-to-stdout-2026-04-02.md))
established that data goes to stdout, diagnostics to stderr. But it did not address what happens
when `--json` mode changes the output contract — should diagnostics still go to stderr?

## Guidance

In JSON mode, suppress human-readable stderr messages. Let the structured output on stdout be the
complete contract. The `--json` flag is an explicit declaration by the caller: "I will parse stdout,
nothing else matters."

Gate all `eprintln!` / stderr writes behind a check that JSON mode is not active. Restructure
conditionals so the JSON branch is checked first and exits early, with human-readable diagnostics
(including empty-result messages) only in the else branch.

```rust
// BEFORE (incorrect) — stderr message leaks into JSON mode
if results.is_empty() {
    eprintln!("No results found.");
}

if json {
    println!("{}", serde_json::to_string(&results)?);
} else {
    // human format...
}

// AFTER (correct) — JSON branch is self-contained
if json {
    println!("{}", serde_json::to_string(&results)?);
} else if results.is_empty() {
    eprintln!("No results found.");
} else {
    // human format...
}
```

## Why This Matters

Machine consumers (scripts, agents, pipelines) parse stdout as the single source of truth.
Unexpected stderr text can: break log parsing, confuse monitoring tools, and create implicit
dual-channel contracts that are fragile and undocumented. The `--json` flag is a contract boundary —
everything the caller needs must be in the JSON on stdout.

## When to Apply

- Any CLI command that supports both human and structured (JSON) output modes
- During code review of any command that accepts `--json`, `--format`, or `--output`
- When adding a new `--json` flag to an existing command, sweep all stderr writes in that code path
- Audit every `eprintln!` and stderr write in commands that have a structured output mode

## Examples

The fix is structural: by checking the `json` flag first and returning from that branch, all
human-only diagnostics are naturally excluded from the machine-consumer path. An empty JSON array
`[]` is the correct empty-result signal — no additional stderr message needed.

## Related

- [CLI data commands should output to stdout](cli-data-commands-should-output-to-stdout-2026-04-02.md)
  — foundational convention this learning refines for structured output modes
