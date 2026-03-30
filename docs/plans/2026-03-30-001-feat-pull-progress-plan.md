---
title: "feat: Show progress during model pull"
type: feat
status: completed
date: 2026-03-30
origin: docs/brainstorms/2026-03-30-pull-progress-requirements.md
---

# feat: Show progress during model pull

## Overview

Replace the spammy per-chunk status output during `lore init` model pulls with a single updating
progress line (TTY) or time-throttled periodic lines (non-TTY).

## Problem Frame

When a model needs pulling, `pull_model` forwards every NDJSON `status` string verbatim through the
`on_progress` callback, producing hundreds of near-identical `pulling sha256:...` lines. For
`nomic-embed-text` (~274 MB) this makes the init output unreadable. (see origin:
docs/brainstorms/2026-03-30-pull-progress-requirements.md)

## Requirements Trace

- R1. Single carriage-return progress line on TTY
- R2. Human-readable progress: downloaded size, total size, percentage
- R3. Fall back to raw status text when `total` is unavailable
- R4. Non-TTY: first progress line after 1s, then every 10s
- R5. Non-TTY lines are normal newline-terminated
- R6. Preserve the `on_progress` callback for non-pull messages

## Scope Boundaries

- No third-party progress bar crate (indicatif, etc.)
- No changes to `Embedder` trait, search, or ingest paths
- No special Windows terminal handling beyond what `\r` provides

## Context & Research

### Relevant Code and Patterns

- `src/embeddings.rs:42-44` — `PullProgress` struct, currently only `status: Option<String>`
- `src/embeddings.rs:76-99` — `pull_model` method, streams NDJSON, passes `status` to callback
- `src/provision.rs:19` — `provision` function, `on_progress: &dyn Fn(&str)` callback
- `src/provision.rs:83` — pull rendering:
  `client.pull_model(&|status| on_progress(&format!("  {status}")))`
- `src/main.rs:98-100` — `cmd_init` passes `eprintln!("{msg}")` as `on_progress`
- All CLI output uses `eprintln!` (stderr), consistent throughout `main.rs`

### Key Observations

- `PullProgress` ignores Ollama's `total` and `completed` NDJSON fields
- `pull_model` callback is `&dyn Fn(&str)` — only passes the status string
- `provision`'s `on_progress` serves both pull messages and non-pull messages (e.g., "Checking for
  Ollama...")
- `std::io::IsTerminal` is stable since Rust 1.70, available at MSRV 1.85
- The crate has `unsafe_code = "deny"`, so all approaches must be safe

## Key Technical Decisions

- **Pull progress rendered directly in `provision`, not through `on_progress`:** The
  `on_progress: &dyn Fn(&str)` callback is designed for line-oriented status messages.
  Carriage-return rendering requires control over newlines that a `&str` callback cannot provide.
  Pull progress will write directly to stderr inside `provision`; all other status messages continue
  through `on_progress`. This is the minimal change that preserves R6 while enabling R1.

- **Structured callback for `pull_model`:** Change `pull_model`'s callback from `&dyn Fn(&str)` to a
  struct with `status`, `completed`, `total` so the caller can decide how to render.

- **TTY detection via `std::io::IsTerminal`:** No external crate needed. Check once before the pull
  loop.

- **`format_bytes` helper:** Simple function to format byte counts as human-readable strings (B, KB,
  MB, GB). No external crate.

## Open Questions

### Resolved During Planning

- **Where to detect TTY?** In `provision`, once, before calling `pull_model`. The `pull_model`
  callback closure captures the detection result.

- **How to handle the trailing cursor after TTY progress?** Print `\n` via `eprintln!()` after the
  pull loop completes to move past the progress line.

### Deferred to Implementation

- **Exact Ollama NDJSON field names:** Verify `total` and `completed` are the actual field names by
  examining Ollama's API response. Adjust `PullProgress` field names if different.

## Implementation Units

- [x] **Unit 1: Enrich pull data model**

  **Goal:** Parse `total` and `completed` from Ollama's pull NDJSON so callers have structured
  progress data.

  **Requirements:** R2, R3

  **Dependencies:** None

  **Files:**
  - Modify: `src/embeddings.rs`

  **Approach:**
  - Add `total: Option<u64>` and `completed: Option<u64>` fields to `PullProgress`
  - Make `PullProgress` public (currently private) so `provision` can use it
  - Change `pull_model` callback from `&dyn Fn(&str)` to `&dyn Fn(&PullProgress)`
  - Inside the NDJSON loop, pass the deserialized `PullProgress` directly to the callback instead of
    extracting just the status string

  **Patterns to follow:**
  - Existing serde derive pattern on `PullProgress`
  - Existing `on_progress` callback pattern in `pull_model`

  **Test scenarios:**
  - Happy path: `PullProgress` deserializes NDJSON with all three fields (status, total, completed)
  - Edge case: `PullProgress` deserializes NDJSON with only `status` (total and completed are None)
  - Edge case: `PullProgress` deserializes NDJSON with `completed` but no `total`

  **Verification:**
  - `cargo test` passes
  - `cargo clippy --all-targets -- -D warnings` clean

- [x] **Unit 2: TTY-aware pull rendering in provision**

  **Goal:** Render pull progress as a single updating line on TTY, or time-throttled periodic lines
  on non-TTY.

  **Requirements:** R1, R2, R3, R4, R5, R6

  **Dependencies:** Unit 1

  **Files:**
  - Modify: `src/provision.rs`

  **Approach:**
  - Import `std::io::IsTerminal` and `std::time::Instant`
  - Before calling `pull_model`, detect TTY via `std::io::stderr().is_terminal()`
  - In the `pull_model` closure:
    - **When `total` and `completed` are present:**
      - TTY: `eprint!("\r  Pulling '{model}': {completed_fmt} / {total_fmt} ({pct}%)")`
      - Non-TTY: same content but `eprintln!`, throttled by elapsed time (first after 1s, then every
        10s), tracked via `Instant`
    - **When `total` is absent (status-only phases like "verifying"):**
      - TTY: `eprint!("\r  {status}")` with trailing spaces to clear previous line
      - Non-TTY: print the status text, subject to same time throttle
  - After `pull_model` returns: if TTY, `eprintln!()` to move past the progress line
  - Add a `format_bytes(bytes: u64) -> String` helper (private function in `provision.rs`) for
    human-readable byte formatting
  - All other `on_progress` calls in `provision` remain unchanged

  **Patterns to follow:**
  - Existing `eprintln!` usage throughout the crate for stderr output
  - Existing `Duration` and timing patterns in `provision.rs` (the Ollama startup wait loop already
    uses `thread::sleep` with `Duration`)

  **Test scenarios:**
  - Happy path: `format_bytes` returns "0 B" for 0, "512 B" for 512, "1.5 KB" for 1536, "274.0 MB"
    for a large value, "1.2 GB" for a GB-scale value
  - Edge case: `format_bytes` handles exact boundary values (1024 -> "1.0 KB", 1048576 -> "1.0 MB")

  **Verification:**
  - `cargo test` passes
  - `cargo clippy --all-targets -- -D warnings` clean
  - Manual: `lore init` on a terminal shows a single updating progress line
  - Manual: `lore init 2>&1 | cat` shows a handful of periodic lines

## System-Wide Impact

- **Interaction graph:** Only `provision::provision` calls `pull_model`. No other callers exist.
  `cmd_init` and `cmd_status` call `provision` but `cmd_status` does not trigger pulls.
- **Unchanged invariants:** The `on_progress: &dyn Fn(&str)` signature on `provision()` does not
  change. `cmd_init` does not change. Ingest, search, serve, and MCP server paths are unaffected.

## Risks & Dependencies

| Risk                                                       | Mitigation                                                                         |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Ollama NDJSON field names differ from expected             | Verify against actual API response; `Option` fields gracefully handle missing data |
| `\r` line clearing leaves artifacts if new line is shorter | Pad with trailing spaces or clear to end-of-line                                   |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-03-30-pull-progress-requirements.md](../brainstorms/2026-03-30-pull-progress-requirements.md)
- Related code: `src/embeddings.rs` (PullProgress, pull_model), `src/provision.rs` (provision)
- Ollama API: `/api/pull` endpoint streams NDJSON with progress fields
