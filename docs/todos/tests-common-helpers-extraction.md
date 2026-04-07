---
title: "Extract shared integration-test helpers into `tests/common/mod.rs`"
priority: P3
category: maintainability
status: ready
created: 2026-04-06
source: ce-review (feat/single-file-ingest)
files:
  - tests/loreignore.rs:18-71
  - tests/single_file_ingest.rs:20-68
related_pr: feat/single-file-ingest
---

# Extract shared integration-test helpers into `tests/common/mod.rs`

## Context

`tests/single_file_ingest.rs` and `tests/loreignore.rs` duplicate four helper functions verbatim
(`memory_db`, `git_init`, `git_commit_all`, `write_md`) along with the same imports of
`lore::database::KnowledgeDB`, `lore::embeddings::FakeEmbedder`, and `tempfile::tempdir`. The
maintainability reviewer flagged this during ce-review at 0.95 confidence.

Future test files exercising the same "temp knowledge base with a fake embedder" pattern will either
copy the helpers again (drift risk) or require a refactor mid-flight. The GPG-signing tweak in
`git_init` already lives in both files; any future change (e.g., shared config tweaks, a switch off
the `format!` template in `write_md`) will have to be duplicated too.

Cargo's integration-test model supports shared helpers via a `tests/common/mod.rs` module: the `mod`
keyword resolves against sibling files in the same integration-test crate.

## Proposed fix

1. Create `tests/common/mod.rs` with the four helpers and the supporting imports:

   ```rust
   // SPDX-License-Identifier: MIT OR Apache-2.0

   //! Shared helpers for integration tests.

   use std::fs;
   use std::path::Path;
   use std::process::Command;

   use lore::database::KnowledgeDB;

   /// Open a 768-dimension in-memory KnowledgeDB matching production embedding shape.
   pub fn memory_db() -> KnowledgeDB { /* ... */
   }

   /// Initialise a git repository with a test identity and disabled GPG signing.
   pub fn git_init(dir: &Path) { /* ... */
   }

   /// Stage and commit all changes with the given message.
   pub fn git_commit_all(dir: &Path, message: &str) { /* ... */
   }

   /// Write a markdown file with title and body, creating parent directories.
   pub fn write_md(dir: &Path, name: &str, title: &str, body: &str) { /* ... */
   }
   ```

2. In each consumer:
   ```rust
   mod common;
   use common::{git_commit_all, git_init, memory_db, write_md};
   ```

3. Delete the duplicated helper blocks from `tests/single_file_ingest.rs:20-68` and
   `tests/loreignore.rs:18-71`.

4. Leave `tests/loreignore.rs`'s local `LoadedIgnore`-specific helpers in place if any exist — only
   extract the four shared ones.

## Test surface

The move is a pure refactor. Existing tests in both files must continue to pass. Add no new tests;
the existing suite is sufficient to verify the helpers still work.

Verification:

- `cargo test --features test-support --test loreignore` passes (9 tests)
- `cargo test --features test-support --test single_file_ingest` passes (12 tests)
- `just ci` green

## Trade-offs

- **Cargo integration-test module idiom is slightly unusual.** `mod common;` in a test file works,
  but some readers may not recognise it as the standard pattern. Document with a comment.
- **`dead_code` warnings.** If a helper is used by only one of the two files after extraction, the
  other file will emit `dead_code` warnings for unused items in `common`. Mark helpers with
  `#[allow(dead_code)]` or structure them as `pub fn` (which avoids the warning at crate level).
- **Low urgency.** Two files duplicating four helpers is a minor drift risk, not a bug. The
  maintenance cost today is close to zero. Defer until a third test file wants the same helpers or
  until the duplication causes a real drift incident.

## When to do this

Defer until either:

- A third integration-test file needs the same helpers (at that point extraction is clearly
  warranted)
- A drift incident where the two copies disagree and a test passes in one but not the other
- Any PR that adds a meaningful helper to either file, so the extraction rides along

## References

- ce-review run artifact:
  `.context/compound-engineering/ce-review/2026-04-06-single-file-ingest/summary.md`
- `tests/loreignore.rs:18-71` — original helper definitions
- `tests/single_file_ingest.rs:20-68` — duplicated helpers
- [Cargo book: integration tests](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests)
  on the `tests/common/mod.rs` convention
