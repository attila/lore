---
title: "MCP write-lock busy errors lack structured metadata"
priority: P2
category: agent-native
status: ready
created: 2026-04-06
source: ce-review (feat/loreignore, follow-up)
files:
  - src/server.rs:518-527
  - src/server.rs:585-594
  - src/server.rs:648-657
  - src/lockfile.rs
related_pr: feat/loreignore
---

# MCP write-lock busy errors lack structured metadata

## Context

The `.loreignore` PR introduced a cross-process write lock at `<database>.lock` that serialises the
three MCP write handlers (`add_pattern`, `update_pattern`, `append_to_pattern`) against each other
and against `lore ingest`. Lock acquisition uses a 5-second bounded poll so MCP write calls fail
fast when a long-running ingest is in flight, returning an error before the MCP client times out.

The current implementation surfaces the busy state as a plain `error_response`:

```rust
let _guard = match write_lock.acquire() {
    Ok(g) => g,
    Err(e) => return error_response(req, &format!("Failed to acquire write lock: {e}")),
};
```

The error message contains a human-readable string ("another lore write is in progress; please retry
in a few seconds"), but there is no structured metadata field that lets an agent distinguish:

- **Retry-able busy** — another write is in flight, retrying in a few seconds will succeed
- **Permanent failure** — the lock file is unreachable, permission denied, disk full, etc.

Both currently surface as the same shape: a JSON-RPC error response with a free-text message. Agents
have to string-match against the error text to decide whether to retry, which is brittle.

This violates the pattern documented in
`docs/solutions/best-practices/expose-mcp-conditional-outcomes-as-metadata-2026-04-06.md`
(introduced in PR #30): when an MCP tool has conditional behaviour, the conditional outcome should
surface as structured metadata so agents can detect it programmatically.

## Proposed fix

1. Distinguish "lock busy" from "lock open failed" at the call site. The simplest way is a typed
   error from `WriteLock::acquire`:

   ```rust
   pub enum AcquireError {
       Busy,
       Io(std::io::Error),
   }
   ```

   `Busy` is returned when the deadline elapses without acquisition. `Io` covers anything else.

2. In each write handler, wrap the busy case in a structured metadata response rather than a plain
   `error_response`. The MCP response stays an error (so the agent knows the operation did not
   succeed), but the metadata payload tells the agent why:

   ```json
   {
     "error": {
       "code": -32000,
       "message": "Another lore write is in progress; retry in a few seconds.",
       "data": {
         "lock_state": "busy",
         "retry_after_seconds": 5
       }
     }
   }
   ```

3. The `Io` case stays as a plain error response — there is no useful metadata for "permission
   denied on lock file".

4. Update the three write handler descriptions in the tools list to mention that `lock_state: busy`
   is a retry-able failure mode.

## Test surface

Two tests covering the discrimination:

1. **`write_lock_busy_returns_structured_metadata`** — hold the lock in a background thread, call
   `add_pattern`, verify the response has `error.data.lock_state == "busy"` and
   `retry_after_seconds == 5`.

2. **`write_lock_io_failure_returns_plain_error`** — point the lock file at a path that cannot be
   created (e.g., a parent directory with no write permission), verify the response is a plain error
   without `data.lock_state`.

## Trade-offs

- **More invasive than the current code.** Touches the `WriteLock` API, all three write handlers,
  and the tools list descriptions. Three coupled changes that must stay in sync.
- **Hard to reproduce in tests.** The current tests for write lock contention use threads; injecting
  a clean "another process holds it" scenario across processes is tricky. The background-thread
  approach works but is timing-sensitive.
- **Low real-world impact.** Concurrent writes are rare in normal use. The current free-text error
  is technically sufficient for an agent that retries on any write failure. The structured metadata
  is a quality-of-implementation improvement, not a correctness fix.

## When to do this

Defer until either:

- A user reports being unable to retry from an agent harness because the error message is parsed
  differently across MCP clients
- Concurrent write contention becomes a real failure mode in some workflow (e.g., a CI agent that
  fan-outs many MCP calls)
- The next PR that touches the write lock or the MCP write handlers, so the change rides along with
  related work

## References

- The MCP conditional-outcomes pattern:
  `docs/solutions/best-practices/expose-mcp-conditional-outcomes-as-metadata-2026-04-06.md`
- The write lock implementation: `src/lockfile.rs`
- The three write handlers: `handle_add`, `handle_update`, `handle_append` in `src/server.rs`
