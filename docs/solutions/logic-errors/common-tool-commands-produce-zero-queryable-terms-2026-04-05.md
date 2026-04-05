---
title: "Common tool commands produce zero queryable terms after cleaning"
date: 2026-04-05
category: logic-errors
module: hook
problem_type: logic_error
component: tooling
severity: medium
symptoms:
  - "Pattern about a specific tool command never surfaces during that command's execution"
  - "LORE_DEBUG shows an empty query or no search performed for a Bash tool call"
root_cause: logic_error
resolution_type: documentation_update
tags: [hook, stop-words, term-cleaning, query-extraction, bash, pipeline-limitation]
---

# Common tool commands produce zero queryable terms after cleaning

## Problem

Certain common tool commands produce zero queryable terms after the hook's term cleaning pipeline
runs. When this happens, no search is performed for that tool call, and no patterns are injected —
regardless of how well-authored the patterns are.

## Symptoms

- A pattern about `just ci` never surfaces when an agent runs `just ci`
- `LORE_DEBUG=1` shows no query assembled for a Bash tool call
- The pattern surfaces through other signals (file edits, transcript context) but never through the
  specific command it documents

## What Didn't Work

- **Improving pattern vocabulary:** Adding synonyms to the pattern body does not help because the
  problem is on the query side, not the pattern side. The hook extracts zero terms from the tool
  call, so no search occurs at all.
- **Lowering `min_relevance`:** Irrelevant — no search is performed, so there are no results to
  threshold.

## Solution

This is a pipeline limitation, not a bug to fix. The term cleaning pipeline applies three filters
that can eliminate all terms from short commands:

1. **Stop word removal:** 60 common English words are filtered, including `just`, `use`, `run`,
   `get`, `set`, `add`, and `new`
2. **Short term removal:** Terms shorter than three characters are discarded (`ci`, `pr`, `go`)
3. **Non-alphabetic stripping:** Digits and symbols are removed before length checking

For `just ci`, "just" is a stop word and "ci" is two characters. Both are removed. Zero terms
survive.

**Known commands affected:**

| Command   | Why it fails                                                              |
| --------- | ------------------------------------------------------------------------- |
| `just ci` | "just" is a stop word, "ci" is two characters                             |
| `go run`  | "go" is two characters, "run" is a stop word                              |
| `go test` | "go" is two characters, "test" survives but alone may not match precisely |
| `npm run` | "npm" survives (three characters), but "run" is a stop word               |

**Mitigations for pattern authors:**

The pattern will still surface through adjacent signals in the surrounding workflow:

- File edits that precede or follow the command (language anchor + filename terms)
- Transcript tail context from the user's last message
- Related Bash commands with longer terms (e.g., `cargo clippy` produces "rust" + "clippy")

Include vocabulary for these adjacent contexts: "quality gate," "continuous integration,"
"pre-commit check" rather than relying solely on the tool command name.

## Why This Works

The term cleaning pipeline is intentionally aggressive to prevent noise from polluting queries. Stop
words and short terms are filtered because they match too broadly and would return irrelevant
results. The tradeoff is that some legitimate short commands become invisible to search.

The adjacent-signal mitigation works because agent sessions rarely consist of a single isolated
command. The surrounding tool calls provide sufficient context for pattern injection.

## Prevention

- When writing patterns about short tool commands, include vocabulary for the surrounding workflow,
  not just the command itself
- Document this limitation in the [Pattern Authoring Guide](../../pattern-authoring-guide.md)
  anti-patterns section (done: "The Stop-Word Trap")
- ROADMAP: "Evaluate transcript tail truncation limit" may improve the transcript signal for
  commands that lack their own queryable terms

## Related Issues

- [Pattern Authoring Guide](../../pattern-authoring-guide.md) — "The Stop-Word Trap" anti-pattern
- [Search Mechanics Reference](../../search-mechanics.md) — term cleaning section with full stop
  word list
- `docs/solutions/database-issues/fts5-query-construction-for-hook-based-search-2026-04-02.md` —
  query assembly details
