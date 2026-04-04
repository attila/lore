---
date: 2026-04-04
topic: security-hardening
---

# Security Hardening

## Problem Frame

Lore is approaching its first release. While the codebase already has strong security foundations
(parameterised SQL, path traversal protection, FTS5 sanitisation, no shell execution, `unsafe`
denied globally), several gaps remain at trust boundaries. These need closing before release, and
the approach must establish security as a continuous development principle rather than a one-time
audit.

Lore runs as a local tool under the user's own permissions. It accepts input from agent hooks
(Claude Code today, Cursor and Opencode soon), MCP clients over stdio, CLI arguments, and markdown
files on disk. The primary threats are resource exhaustion from unbounded input and unintended file
access via unvalidated paths.

## Requirements

**Input Validation at MCP Boundary**

- R1. Enforce maximum length on all MCP tool string inputs: ~1KB for `query`, ~256KB for `body`,
  ~512 bytes for `title`, `heading`, and `source_file`. Cap `top_k` to a sensible maximum (e.g.
  100). Reject with a clear error if exceeded. Validation lives in each handler (matches existing
  per-handler pattern for required-field checks).
- R2. Add security-focused test cases for MCP input limits, inline in the server test module.

**Transcript Path Hardening**

- R3. Validate that `transcript_path` from hook input resolves to a path under `$HOME` before
  reading. Use `canonicalize` + `starts_with` (same pattern as `validate_within_dir`). This is
  agent-agnostic — no agent stores transcripts outside `$HOME`. If `canonicalize` fails (file
  doesn't exist yet, stale path), silently skip the transcript signal — consistent with the existing
  fallthrough behaviour where `last_user_message` returns `None`.
- R4. Bound the transcript read to the last ~32KB (seek to end minus 32KB, read forward). We only
  need the last user message; reading the full file is wasteful and an OOM vector on long sessions.
  The bounded reader must handle partial UTF-8 at the buffer start (skip to next valid char
  boundary) and discard the first partial JSONL line (everything before the first `\n`).
- R5. Add test cases for transcript path validation (reject paths outside `$HOME`) and bounded
  reading, inline in hook tests.

**Dedup File Integrity**

- R6. Prevent race conditions on the dedup file between concurrent hook invocations. The lock (or
  atomic operation) must be held across the entire read-filter-write sequence in
  `handle_pre_tool_use`, not within individual `read_dedup`/`write_dedup` calls — locking each
  function separately would not close the TOCTOU window. Two approaches to evaluate during planning:
  advisory file locking (`flock` via `fs2` or `fd-lock`) or atomic-write-via-rename (write to temp,
  rename over dedup file).
- R7. Hash session IDs (truncated deterministic hash, 16 hex chars) for dedup filenames instead of
  character-level sanitisation. This eliminates collision risk from different IDs sanitising to the
  same string and avoids leaking raw session IDs into `/tmp` filenames. The threat is filename
  collision, not cryptographic security — a non-cryptographic deterministic hash may suffice if it
  avoids adding a heavy dependency.
- R8. Add test cases for session ID hashing (determinism, collision resistance across similar
  inputs) and file locking behaviour, inline in hook tests.

**FTS5 Sanitisation Coverage**

- R9. Expand FTS5 sanitisation test coverage to close remaining gaps. Most operator characters
  already have dedicated tests in `database.rs`; the delta is: backslash (`\`) as an individual
  case, combined multi-operator sequences (e.g. `"foo/bar:baz"`), and any operators not yet
  isolated. Scope to what's missing, not a rewrite of existing coverage.

**Security Documentation**

- R10. Create `SECURITY.md` at repo root containing: trust boundaries and assumptions (which inputs
  are trusted, which are validated), threat model summary, and security reporting instructions. No
  contributor workflow — the project is closed to external contributions; only discussions and
  security reporting are available.

## Security Testing Principle

Security tests are not a separate suite. They live alongside the functionality they protect — path
traversal tests in ingest tests, FTS injection tests in search tests, input limit tests in server
tests. Every trust boundary gets test cases where the boundary is implemented. This is a standing
convention, not a one-time task.

## Success Criteria

- All MCP tool inputs reject oversized payloads with descriptive errors
- Transcript reads are bounded and path-validated; no file outside `$HOME` is readable via hook
  input
- Dedup files use hashed session IDs; concurrent access is safe under locking
- FTS5 sanitisation has explicit regression tests for every operator character
- `SECURITY.md` documents trust boundaries, threat model, and reporting process
- No new `tests/security.rs` file — all security tests are inline in their domain

## Scope Boundaries

- No global memory limit or custom allocator — bounded reads at each input point are sufficient
- No authentication or authorisation — lore is a local single-user tool
- No network hardening — MCP transport is stdio, Ollama is localhost
- No contributor security guidelines — project is closed to contributions
- Transcript path validation uses `$HOME` as the broad guard, not a config-driven per-agent
  allowlist

## Key Decisions

- **Baked-in security tests over separate suite**: security test cases live in the same module as
  the code they protect, as a standing convention
- **Transcript path guard uses `$HOME`**: agent-agnostic, no config needed, blocks the obviously
  wrong cases without coupling to any specific agent's directory structure
- **Session ID hashing over sanitisation**: truncated deterministic hash (16 hex chars) is
  collision-resistant and doesn't leak raw IDs into `/tmp`. Cryptographic strength not required —
  the threat is filename collision, not an adversary reversing the hash
- **Bounded tail-read over global memory cap**: reading last 32KB of transcript is simpler and more
  targeted than enforcing a process-wide memory limit

## Dependencies / Assumptions

- Claude Code (and future agents) always provide `transcript_path` values under `$HOME` — the
  `$HOME` guard is a defence-in-depth check, not a primary security mechanism
- Advisory file locking (`flock`) or atomic rename is available on all target platforms (macOS,
  Linux) — true for the current support matrix
- A deterministic hash function is available via an existing or lightweight new dependency

## Outstanding Questions

### Deferred to Planning

- [Affects R6][Technical] Advisory locking (`fs2` or `fd-lock`) vs atomic-write-via-rename? Locking
  requires a new dependency; rename is zero-dep but changes the write pattern. `std::os::unix` does
  not expose `flock` in stable Rust — it is not a candidate.
- [Affects R7][Needs research] Which deterministic hash? `sha2` (crypto, new dep), a lightweight
  non-crypto hash crate, or `std::hash::DefaultHasher` with a fixed seed? `DefaultHasher` is not
  guaranteed stable across Rust versions, but dedup files are ephemeral — only needs stability
  within a single binary build. Evaluate whether that's acceptable.
- [Affects R4][Technical] Exact seek strategy for bounded tail-read — `SeekFrom::End(-32768)` with
  fallback to `SeekFrom::Start(0)` for small files. Must skip partial UTF-8 at buffer start and
  discard first partial JSONL line.

## Next Steps

→ `/ce:plan` for structured implementation planning
