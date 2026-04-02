---
title: "Session dedup must gate on SessionStart, and deny-first-touch requires dedup"
date: 2026-04-02
category: logic-errors
module: hook
problem_type: logic_error
component: tooling
symptoms:
  - "Deny-first-touch hook creates infinite deny loop — every retry is also denied"
  - "Stale dedup files from CLI testing pollute real Claude Code sessions"
  - "Patterns re-injected after manual lore hook invocations create false dedup state"
root_cause: logic_error
resolution_type: code_fix
severity: medium
tags:
  - session-dedup
  - deny-first-touch
  - hooks
  - temp-files
  - infinite-loop
  - lifecycle
---

# Session dedup must gate on SessionStart, and deny-first-touch requires dedup

## Problem

Two related session state issues in the hook lifecycle: (1) deny-first-touch without dedup creates
an infinite loop, and (2) dedup without gating on SessionStart causes false state from CLI testing.

## Symptoms

- **Deny-first-touch loop**: Blocking the first Edit/Write with conventions as the deny reason
  forces Claude to retry — but without tracking which patterns were already denied, every retry is
  also blocked. Claude retries indefinitely.
- **Stale dedup from CLI**: Running `echo '{"hook_event_name":"PreToolUse",...}' | lore hook` from a
  terminal with session_id `"test"` creates a dedup file at `$TMPDIR/lore-session-test`. Later, a
  real Claude Code session with the same or similar session_id reads this stale file and skips
  injecting patterns that were never actually seen by Claude.

## What Didn't Work

- Deny-first-touch without any dedup mechanism — infinite loop on first edit
- Dedup that activates whenever a session_id is present — CLI testing creates persistent state that
  leaks into real sessions

## Solution

### 1. Gate dedup on SessionStart

Only activate dedup when the dedup file exists (meaning SessionStart has run and created it):

```rust
let dedup_path = session_dedup_path(input);

// Dedup only when SessionStart has run (file exists).
// Manual CLI calls and sessions without SessionStart skip dedup entirely.
let (results, dedup_active) = if let Some(ref path) = dedup_path
    && path.exists()
{
    let seen = read_dedup(path);
    let filtered = results.into_iter()
        .filter(|r| !seen.contains(&r.id))
        .collect();
    (filtered, true)
} else {
    (results, false)
};
```

The lifecycle is:

1. **SessionStart** creates the dedup file (empty) via `reset_dedup()`
2. **PreToolUse** checks `path.exists()` — only filters and appends if the file exists
3. **PostCompact** truncates the file and re-emits SessionStart content
4. **Manual CLI calls** never trigger SessionStart, so the file doesn't exist, and dedup is skipped

### 2. Defer deny-first-touch until dedup is solid

Deny-first-touch (blocking Edit/Write with conventions as deny reason) was validated as a stronger
compliance mechanism than `additionalContext`, but requires dedup to track which patterns have
already been denied:

```
Without dedup:
  Edit foo.ts → DENIED (conventions injected) → retry
  Edit foo.ts → DENIED (same conventions) → retry  ← infinite loop

With dedup:
  Edit foo.ts → DENIED (conventions injected, IDs recorded) → retry
  Edit foo.ts → (conventions already in dedup, no denial) → ALLOWED
```

This is deferred to v2 as a configuration option once the dedup mechanism is battle-tested.

## Why This Works

- **`path.exists()` gate** cleanly separates hook-managed sessions (SessionStart creates the file)
  from ad-hoc CLI invocations (no file created, no dedup applied)
- **Dedup file lifecycle** (create → append → truncate → re-create) maps exactly to the Claude Code
  session lifecycle (start → tool calls → compaction → resume)
- **Deny-first-touch + dedup** is a two-phase pattern: first deny injects conventions into the
  conversation, second pass (with IDs in dedup) allows the tool through. Without the second piece,
  the system has no memory that it already denied.

## Prevention

- Always test hook state management with both managed sessions (via Claude Code) and manual CLI
  invocations — they exercise different code paths
- When designing stateful hooks, gate state reads on explicit initialization (file existence, flag,
  or marker) rather than on the presence of input fields
- Document deny-based hooks as requiring a dedup/memory mechanism to avoid infinite loops
