---
date: 2026-03-30
topic: pull-progress
---

# Model Pull Progress Display

## Problem Frame

During `lore init`, when a model needs downloading, the pull output spams the terminal with repeated
`pulling sha256:...` lines because `pull_model` forwards every NDJSON status string verbatim. For
`nomic-embed-text` (~274 MB) this produces hundreds of identical-looking lines.

## Requirements

**TTY output (interactive terminal)**

- R1. Show a single progress line that updates in place using carriage return
- R2. Display human-readable progress: downloaded size, total size, and percentage (e.g.,
  `Pulling nomic-embed-text: 142 MB / 274 MB (51%)`)
- R3. When `total` is not available in the NDJSON, display the status text as-is (some phases like
  "verifying" have no byte progress)

**Non-TTY output (piped, redirected, CI)**

- R4. Print the first progress line after 1 second, then at most every 10 seconds thereafter
- R5. Each throttled line is a normal newline-terminated line (no carriage return tricks)

**General**

- R6. Preserve the existing `on_progress` callback architecture; the change should be internal to
  how `pull_model` reports and how `provision` renders

## Success Criteria

- `lore init` in a terminal shows a single updating progress line during pull
- `lore init | cat` produces a handful of periodic lines, not hundreds
- Non-pull status messages (checking Ollama, model found, etc.) are unchanged

## Scope Boundaries

- No third-party progress bar crate (indicatif, etc.) -- use plain `\r` + `eprint!` for the TTY case
- No changes to the `Embedder` trait or search/ingest paths
- No Windows terminal handling beyond what `\r` already provides

## Key Decisions

- Time-based throttling for non-TTY (1s initial, 10s interval) instead of percentage-based: avoids
  dependency on `total` being available and produces predictable output regardless of download speed

## Next Steps

-> /ce:plan for structured implementation planning
