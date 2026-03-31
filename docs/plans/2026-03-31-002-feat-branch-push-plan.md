---
title: "feat: Add branch push for agent submissions"
type: feat
status: completed
date: 2026-03-31
origin: docs/brainstorms/2026-03-31-branch-push-requirements.md
deepened: 2026-03-31
---

# feat: Add branch push for agent submissions

## Overview

When configured, lore's write operations (`add_pattern`, `update_pattern`, `append_to_pattern`)
create per-submission branches forked from HEAD using git plumbing commands — without switching HEAD
or the working tree — then push each branch to the remote. This gives the human a review gate for
agent-submitted content, eliminates concurrency issues, and keeps the local checkout undisturbed.

## Problem Frame

Lore's pattern repo uses trunk-based development. Agent submissions currently land on whatever
branch is checked out (typically trunk), mixing untrusted agent content with curated human content.
The human needs an inbox: per-submission branches where each agent submission lands for independent
review before curation into trunk. Multiple sessions (worktrees, different clones) must be able to
submit concurrently without races. (see origin:
docs/brainstorms/2026-03-31-branch-push-requirements.md)

## Requirements Trace

- R1. Optional config field `inbox_branch_prefix`; absent = current behavior
- R2. All three write operations use the branch push flow when configured
- R3. Per-submission branches forked from HEAD via git plumbing — no working tree or HEAD changes
- R4. Update/append read from HEAD (checked-out branch); inbox-only files not supported
- R5. Push after commit; push failure is a hard error
- R6. Branch names: `<prefix><slug>`, disambiguated on collision
- R7. Collision disambiguator for duplicate branch names
- R8. No local indexing when branch push is active
- R9. Indexing unchanged when config is absent
- R10. Content searchable only after human curates to trunk and re-indexes
- R11. MCP response distinguishes "pushed to branch" from "committed locally"

## Scope Boundaries

- No PR creation — branches are a simple inbox
- No building on previous inbox submissions — update/append only works on trunk files
- No local indexing of inbox content
- No automatic cleanup of merged inbox branches
- Cherry-pick conflicts are the human's responsibility

## Context & Research

### Relevant Code and Patterns

- `src/git.rs` — current module: `add_and_commit` (porcelain), `is_git_repo`. Uses a
  `let git = |args| -> Result<()>` closure that shells out via `Command::new("git")` and checks
  `status.success()`. The plumbing functions need a variant that captures stdout (SHA hashes).
- `src/ingest.rs` — write operations: `add_pattern`, `update_pattern`, `append_to_pattern`. All
  follow: validate → build content → `std::fs::write` → `index_single_file` → `try_commit` →
  `WriteResult`. The branch push path replaces write/index/commit with plumbing/push.
- `src/config.rs` — `Config` struct with derive `Serialize, Deserialize`. Nested tables:
  `OllamaConfig`, `SearchConfig`, `ChunkingConfig`. No `#[serde(deny_unknown_fields)]`, so adding
  `Option<GitConfig>` with `#[serde(default)]` is backward-compatible.
- `src/server.rs` — MCP handlers (`handle_add`, `handle_update`, `handle_append`) call ingest
  functions and format `WriteResult` into response text. `ServerContext` holds `&Config`.
- `WriteResult` — `{ file_path, chunks_indexed, committed: bool,
  embedding_failures }`. The
  `committed` field needs to become richer to express "pushed to branch X".

### Institutional Learnings

- Three error patterns are intentional: `anyhow::Result` for internal ops, `IngestResult.errors` for
  partial-failure bulk ingest, `JsonRpcResponse` errors for MCP protocol. New code follows the same
  pattern.
- Config round-trip test (`round_trip_save_and_load`) must pass with the new optional field both
  present and absent.

## Key Technical Decisions

- **Per-submission branches:** Each write operation creates a uniquely-named branch forked from
  HEAD. No shared ref to race on — concurrent sessions, worktrees, and different clones all work
  safely. (see origin: Key Decisions — "Per-submission branches, not a single long-lived branch")
- **Git plumbing via shell commands:** Consistent with existing `git.rs` pattern. Use
  `Command::new("git")` with plumbing subcommands (`hash-object`, `commit-tree`, `update-ref`,
  `push`). No `git2` crate.
- **Temporary index file:** Use `GIT_INDEX_FILE` env var pointing to `.git/lore-tmp-index` to build
  trees without touching the real index or working tree. Deterministic path under `.git/` avoids
  adding `tempfile` as a runtime dependency (it is dev-only today). Stale files from interrupted
  runs are harmless — `read-tree` replaces the entire index.
- **`WriteResult.committed` → enum:** Replace `committed: bool` with a `CommitStatus` enum
  (`NotCommitted`, `Committed`, `Pushed { branch }`) to cleanly express the three states. Use
  `NotCommitted` (not `None`) to avoid confusion with `Option::None`.
- **`push_branch` parameter on write functions:** Add `push_branch: Option<&str>` to the three
  ingest write functions. When `Some`, take the plumbing path; when `None`, current behavior
  preserved.
- **Always fork from HEAD:** Each submission starts from the checked-out commit's tree. No need to
  resolve existing branch state. Simpler than the long-lived branch approach.
- **Remote is `origin`:** Hardcoded for v1. Configurable remote deferred.
- **Branch name collision:** If `refs/heads/<prefix><slug>` already exists, append `-<N>` where N
  increments from 2. Simple, deterministic, readable.

## Open Questions

### Resolved During Planning

- **Git plumbing sequence:** `hash-object -w --stdin` → read HEAD tree via temp index →
  `update-index --cacheinfo` → `write-tree` → `commit-tree` → `update-ref` → `push`. Temp index via
  `GIT_INDEX_FILE` pointing to `.git/lore-tmp-index`.
- **`hash-object --stdin` requires stdin piping:** Unlike the existing `git` closure pattern,
  `hash-object -w --stdin` needs content written to the child process's stdin. Use `.spawn()` +
  write to stdin handle + `.wait_with_output()`. This is a distinct helper from `git_output`.
- **Branch initialization:** Always fork from HEAD. No existing branch state to resolve. Each
  submission is an independent branch.
- **Config placement:** `[git]` section with `inbox_branch_prefix` field, matching the existing
  pattern of nested config tables.
- **Slug derivation:** `add_pattern` uses the title slug (same as filename generation).
  `update_pattern` and `append_to_pattern` use the source file stem (without directory or
  extension).
- **Collision strategy:** Check if `refs/heads/<prefix><slug>` exists via `git show-ref --verify`.
  If so, try `<prefix><slug>-2`, `-3`, etc. Cap at a reasonable limit (e.g., 100) and bail if
  exhausted.

### Deferred to Implementation

- Exact error messages for push failures — git stderr is propagated via `anyhow::bail!`, which gives
  natural messages
- Whether `git_init` test helpers across modules should be consolidated (existing duplication, not
  caused by this feature)
- `knowledge_dir` is assumed to be the git repo root (existing assumption in all git operations;
  pre-existing limitation)

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

```
Git plumbing commit sequence (no working tree changes):

  content (in memory)
      │
      ▼
  hash-object -w --stdin ──► blob_sha       (pipe content via stdin)
      │
      ▼
  resolve HEAD ──► parent_sha
      │
      ▼
  GIT_INDEX_FILE=.git/lore-tmp-index
  read-tree HEAD^{tree}                     ◄── populate temp index from HEAD
  update-index --add --cacheinfo 100644,<blob_sha>,<rel_path>
  write-tree ──► tree_sha
      │
      ▼
  commit-tree <tree_sha> -p <parent_sha> -m "<message>" ──► commit_sha
      │
      ▼
  generate unique branch name: <prefix><slug>[-N]
      │
      ▼
  update-ref refs/heads/<branch> <commit_sha>
      │
      ▼
  push origin <branch>
```

```
Branch naming:

  prefix = "inbox/"   (from config)
  slug   = "error-handling"   (from title or file stem)
      │
      ▼
  candidate = "inbox/error-handling"
      │
  exists?  ──no──► use it
      │
     yes
      │
  candidate = "inbox/error-handling-2"
      │
  exists?  ──no──► use it
      │
     yes
      │
     ...
```

## Implementation Units

- [ ] **Unit 1: Config — add `[git]` section**

  **Goal:** Add optional `GitConfig` with `inbox_branch_prefix` field to `Config`.

  **Requirements:** R1, R9

  **Dependencies:** None

  **Files:**
  - Modify: `src/config.rs`

  **Approach:**
  - Add `GitConfig` struct with `pub inbox_branch_prefix: String`
  - Add `#[serde(default)] pub git: Option<GitConfig>` to `Config`
  - Update `Config::default_with` (leave as `None`)
  - A convenience method on `Config` to access the prefix (e.g.,
    `pub fn inbox_branch_prefix(&self) -> Option<&str>`)

  **Patterns to follow:**
  - Existing nested config structs: `OllamaConfig`, `SearchConfig`, `ChunkingConfig`
  - Derive same traits: `Debug, Clone, PartialEq, Serialize, Deserialize`

  **Test scenarios:**
  - Happy path: Config with `[git] inbox_branch_prefix = "inbox/"` round- trips through save/load
  - Happy path: Config without `[git]` section loads successfully with `git: None`
  - Happy path: `inbox_branch_prefix()` returns `Some("inbox/")` when configured, `None` when absent
  - Edge case: Config with empty `inbox_branch_prefix = ""` loads (validation belongs to the write
    path, not config)

  **Verification:**
  - `round_trip_save_and_load` passes for configs with and without `[git]`
  - Existing tests unchanged

- [ ] **Unit 2: Git plumbing — commit to per-submission branch**

  **Goal:** Add git plumbing functions that create a commit on a uniquely-named branch forked from
  HEAD, without touching the working tree, index, or HEAD.

  **Requirements:** R3, R5, R6, R7

  **Dependencies:** None (pure git module additions)

  **Files:**
  - Modify: `src/git.rs`

  **Approach:**
  - Add a `git_output` helper (parallel to existing `git` closure) that returns trimmed stdout on
    success
  - Add a `git_stdin` helper for commands that need content piped to stdin (specifically
    `hash-object -w --stdin`). Uses `.spawn()` + write to stdin + `.wait_with_output()`
  - Add
    `commit_to_new_branch(repo_dir, prefix, slug, file_path,
    content, message) -> Result<String>`:
    the core plumbing sequence — generate unique branch name, hash-object, temp index at
    `.git/lore-tmp-index`, read-tree HEAD, update-index, write-tree, commit-tree with HEAD as
    parent, update-ref. Returns the branch name.
  - Add `push_branch(repo_dir, branch) -> Result<()>`: pushes the named branch to `origin`
  - Branch name generation: try `<prefix><slug>`, check existence via
    `git show-ref --verify --quiet`, if exists try `-2`, `-3`, etc.

  **Patterns to follow:**
  - Existing `add_and_commit` pattern: closure-based git invocation, `current_dir(repo_dir)`,
    `anyhow::bail!` with stderr on failure
  - Use `Command::new("git").env("GIT_INDEX_FILE", ...)` for temp index

  **Test scenarios:**
  - Happy path: `commit_to_new_branch` creates branch `inbox/test-file` with a new file;
    `git show inbox/test-file:<file>` returns the content; HEAD and working tree unchanged
  - Happy path: Two calls with different slugs create two independent branches, each forked from
    HEAD
  - Happy path: `push_branch` pushes to a bare remote; content verified
  - Edge case: Branch name collision — first call creates `inbox/foo`, second call with same slug
    creates `inbox/foo-2`
  - Edge case: Multiple collisions — `inbox/foo`, `inbox/foo-2` both exist, third call creates
    `inbox/foo-3`
  - Error path: `push_branch` fails when no remote configured — returns error with stderr context
  - Error path: `commit_to_new_branch` on a non-git directory — fails cleanly

  **Verification:**
  - All plumbing operations work without changing HEAD or working tree
  - Tests use `tempdir` + `git init` + bare remote pattern
  - Test helpers disable GPG signing (use the `ingest.rs` variant with `commit.gpgsign false`)

- [ ] **Unit 3: WriteResult — replace `committed: bool` with enum**

  **Goal:** Extend `WriteResult` to express three commit states: not committed, committed locally,
  pushed to a branch.

  **Requirements:** R11

  **Dependencies:** None (data type change)

  **Files:**
  - Modify: `src/ingest.rs` (struct + `try_commit` return)
  - Modify: `src/server.rs` (response formatting)
  - Modify: `tests/e2e.rs` (update `result.committed` field access)

  **Approach:**
  - Add `CommitStatus` enum: `NotCommitted`, `Committed`, `Pushed { branch: String }`
  - Replace `committed: bool` in `WriteResult` with `commit_status: CommitStatus`
  - Update `try_commit` to return `CommitStatus::Committed` or `CommitStatus::NotCommitted`
  - Update all three server handlers' response formatting to match on `CommitStatus` — `Committed`
    shows current text, `Pushed` shows "pushed to <branch>, pending review", `NotCommitted` shows no
    commit note

  **Patterns to follow:**
  - Existing response formatting in `handle_add`, `handle_update`, `handle_append`

  **Test scenarios:**
  - Happy path: Server snapshot tests for `handle_add` still pass (the `Committed` variant produces
    the same text as `committed: true`)
  - Happy path: New snapshot test for `Pushed` variant response text includes branch name and
    "pending review"

  **Verification:**
  - All existing server and e2e tests pass without text changes
  - Response text for pushed patterns is clearly distinguishable

- [ ] **Unit 4: Ingest — branch push path in write operations**

  **Goal:** Add the branch push code path to `add_pattern`, `update_pattern`, and
  `append_to_pattern`.

  **Requirements:** R2, R3, R4, R8

  **Dependencies:** Unit 1 (config), Unit 2 (git plumbing), Unit 3 (CommitStatus)

  **Files:**
  - Modify: `src/ingest.rs`

  **Approach:**
  - Add `inbox_branch_prefix: Option<&str>` parameter to all three write functions.
  - When `inbox_branch_prefix` is `Some(prefix)`:
    - **add_pattern**: validate slug, build content in memory, `commit_to_new_branch` with title
      slug, `push_branch`, return `WriteResult` with `CommitStatus::Pushed`, `chunks_indexed: 0`
    - **update_pattern**: read existing content from HEAD via `std::fs::read_to_string` (file must
      exist on working tree), extract title, build new content, `commit_to_new_branch` with file
      stem as slug, `push_branch`, return
    - **append_to_pattern**: same read pattern, append heading + body, `commit_to_new_branch`,
      `push_branch`, return
  - When `inbox_branch_prefix` is `None`: current behavior unchanged
  - Skip `index_single_file` when branch push is active (R8)
  - For `add_pattern`: do NOT write file to disk in inbox mode — the file goes only to the branch
  - For `update_pattern`/`append_to_pattern`: read from disk (file must exist on trunk), but do not
    write modifications back — they go to the branch
  - **Path validation:** For `update_pattern`/`append_to_pattern` in inbox mode, the existing
    `validate_within_dir` works because the file must exist on the working tree (trunk). For
    `add_pattern`, `validate_slug` works as today (component-based, no canonicalize needed).
  - The `db` and `embedder` params are unused in the branch push path — acceptable as they are
    needed for the default path

  **Patterns to follow:**
  - Existing validation logic in each function (slug, path traversal)
  - `build_file_content` and `extract_title` helpers (reused as-is)

  **Test scenarios:**
  - Happy path: `add_pattern` with prefix creates branch, file visible via `git show`, not on
    working tree, not indexed
  - Happy path: `update_pattern` with prefix reads trunk file, pushes modified version to new
    branch, trunk file unchanged
  - Happy path: `append_to_pattern` with prefix reads trunk file, appends section on new branch,
    trunk file unchanged
  - Happy path: With `prefix = None` behaves exactly as before (file on disk, indexed, committed
    locally)
  - Edge case: `add_pattern` twice with same title — second gets disambiguated branch name
  - Edge case: `update_pattern` with prefix for file not on trunk — bails with "File not found"
  - Error path: push failure propagates as `anyhow::Error`

  **Verification:**
  - Working tree and HEAD unchanged after all branch push operations
  - SQLite DB has no chunks for branch-pushed content
  - Default (no prefix) behavior identical to before

- [ ] **Unit 5: Server handlers — thread config through**

  **Goal:** Wire `inbox_branch_prefix` from config into the ingest function calls and ensure MCP
  responses reflect the commit status.

  **Requirements:** R2, R11

  **Dependencies:** Unit 1 (config), Unit 3 (CommitStatus), Unit 4 (ingest changes)

  **Files:**
  - Modify: `src/server.rs`

  **Approach:**
  - In `handle_add`, `handle_update`, `handle_append`: pass `ctx.config.inbox_branch_prefix()` as
    the new parameter to the ingest functions
  - Response formatting already handled by Unit 3 (match on `CommitStatus`)

  **Patterns to follow:**
  - Existing handler pattern: extract args, call ingest, format response

  **Test scenarios:**
  - Happy path: `TestHarness` with prefix configured produces "pushed to inbox/..., pending review"
    response via snapshot test
  - Happy path: `TestHarness` without prefix produces current response text (existing snapshot tests
    unchanged)

  **Verification:**
  - Snapshot tests cover both configured and unconfigured cases

- [ ] **Unit 6: Integration tests**

  **Goal:** End-to-end tests proving the branch push flow works with real git repos, including push
  to a bare remote.

  **Requirements:** R1–R11

  **Dependencies:** All prior units

  **Files:**
  - Create: `tests/branch_push.rs`

  **Approach:**
  - Test fixture: `tempdir` with `git init`, initial commit on default branch, bare repo as remote
    (`git init --bare`), `git remote add origin`. Disable GPG signing — use the `ingest.rs` test
    helper pattern (with `commit.gpgsign false`).
  - Tests exercise the full path: config → ingest function → git plumbing → push → verify remote

  **Patterns to follow:**
  - `tests/e2e.rs` — temp dir setup, git init, FakeEmbedder, in-memory DB

  **Test scenarios:**
  - Happy path: `add_pattern` creates `inbox/<slug>` on remote; working tree and HEAD unchanged;
    file content correct on remote branch
  - Happy path: `update_pattern` pushes modified trunk file to new branch; trunk file unchanged
  - Happy path: `append_to_pattern` pushes appended trunk file to new branch
  - Happy path: Two `add_pattern` calls with different titles create two independent branches on
    remote
  - Happy path: Two `add_pattern` calls with same title create `inbox/<slug>` and `inbox/<slug>-2`
  - Integration: Full sequence: add → update → append (different patterns), all on separate
    branches, verify content on remote
  - Edge case: No `[git]` config — default behavior preserved, no push
  - Error path: Push to nonexistent remote fails with clear error

  **Verification:**
  - `git show` on the bare remote confirms file content at each step
  - `git rev-parse HEAD` in working repo unchanged after each operation
  - No files created or modified in working tree during branch push ops

## System-Wide Impact

- **Interaction graph:** The change touches ingest write functions → git module → server response
  formatting. No callbacks, middleware, or observers are affected. The search path
  (`search_patterns`) is completely unaffected.
- **Error propagation:** Push failures propagate as `anyhow::Error` from `git::push_branch` through
  the ingest function to the server handler, which converts them to JSON-RPC error responses. This
  differs from the current `try_commit` pattern (which swallows errors as `bool`), but R5 requires
  push failures to be hard errors.
- **State lifecycle risks:** Each submission creates an independent branch ref. If push fails, the
  local ref exists but the remote doesn't — the branch name is "taken" locally, but the next
  submission gets a new name anyway. No shared state is corrupted.
- **Concurrency:** Per-submission branches eliminate all concurrency concerns. Multiple sessions,
  worktrees, and clones can submit simultaneously without coordination. The only theoretical
  collision is two sessions generating the same branch name, which the disambiguator handles locally
  and the remote rejects (non-fast-forward) if it happens cross-clone.
- **API surface parity:** The MCP tool schemas (input parameters) are unchanged. Only the response
  text changes (R11). Clients calling `add_pattern`, `update_pattern`, `append_to_pattern` need no
  changes.
- **Unchanged invariants:** `search_patterns` behavior is completely unaffected. The
  `ingest_directory` bulk ingest function is not modified. The `lore init` command is not modified.

## Risks & Dependencies

| Risk                                                              | Mitigation                                                              |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------- |
| Git plumbing edge cases (empty repo, detached HEAD)               | Comprehensive test coverage in Unit 2                                   |
| Push requires non-interactive auth (SSH key or credential helper) | Document as prerequisite                                                |
| Branch proliferation on remote                                    | Human cleans up after review; same as PR workflow                       |
| Cross-clone branch name collision (two clones pick same name)     | Remote rejects non-fast-forward; agent gets error, retries get new name |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-03-31-branch-push-requirements.md](docs/brainstorms/2026-03-31-branch-push-requirements.md)
- Related code: `src/git.rs`, `src/ingest.rs`, `src/config.rs`, `src/server.rs`
- Related tests: `tests/e2e.rs` (integration test pattern)
