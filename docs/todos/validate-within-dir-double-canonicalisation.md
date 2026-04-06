---
title: "Eliminate double canonicalisation in `ingest_single_file` / `validate_within_dir`"
priority: P3
category: maintainability
status: ready
created: 2026-04-06
source: ce-review (feat/single-file-ingest)
files:
  - src/ingest.rs:711-825
  - src/ingest.rs:1082-1098
related_pr: feat/single-file-ingest
---

# Eliminate double canonicalisation in `ingest_single_file` / `validate_within_dir`

## Context

During ce-review of the single-file ingest PR, the maintainability reviewer noticed that
`ingest_single_file` performs four filesystem `canonicalize()` syscalls where two would suffice:

1. `file_path.canonicalize()` at `src/ingest.rs:733` → `canonical`
2. `validate_within_dir(knowledge_dir, &canonical)` at `src/ingest.rs:765`, which internally calls
   `knowledge_dir.canonicalize()` AND `canonical.canonicalize()` (lines 1087-1088)
3. `knowledge_dir.canonicalize()` at `src/ingest.rs:772` → `canonical_dir`

The result: two canonical values (`canonical`, `canonical_dir`) that are also computed inside
`validate_within_dir` and thrown away. A reader tracing the function has to juggle three separate
canonical values and confirm they all agree.

`canonicalize()` is not free — it walks parent directories and resolves symlinks. In tight loops
(e.g., a future batch `--file` mode, or the Pattern QA skill running dozens of single-file ingests
in sequence), the duplication becomes measurable.

This was rated low severity (0.9 confidence, advisory route) because the current code is correct and
the overhead is tiny in absolute terms. Logging it as a follow-up so it is not forgotten.

## Proposed fix

Refactor `validate_within_dir` to return both canonicalised paths so callers don't have to redo the
work:

```rust
/// Validate that `file_path` lies inside `knowledge_dir` and return the
/// canonicalised `(knowledge_dir, file_path)` pair.
fn canonicalize_within_dir(
    knowledge_dir: &Path,
    file_path: &Path,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let canon_dir = knowledge_dir.canonicalize()?;
    let canon_file = file_path.canonicalize()?;
    if !canon_file.starts_with(&canon_dir) {
        anyhow::bail!(
            "Path escapes the knowledge directory: {}",
            file_path.display()
        );
    }
    Ok((canon_dir, canon_file))
}
```

Then in `ingest_single_file`:

```rust
let canonical_dir;
let canonical;
match canonicalize_within_dir(knowledge_dir, file_path) {
    Ok((d, f)) => {
        canonical_dir = d;
        canonical = f;
    }
    Err(e) => {
        result.errors.push(e.to_string());
        return result;
    }
}
```

This removes the standalone `file_path.canonicalize()` and the standalone
`knowledge_dir.canonicalize()` from `ingest_single_file`, replacing them with a single call. The
is-file check and extension check move after the canonicalisation.

`add_pattern`, `update_pattern`, and `append_to_pattern` can either:

- Keep using the old `validate_within_dir` (keep it as a thin wrapper calling
  `canonicalize_within_dir(...).map(|_| ())`), or
- Migrate to the new helper and discard the canonical paths if they don't need them.

The CWD-hint error context from the single-file PR should move into `canonicalize_within_dir`'s
error path so relative-path diagnostics remain helpful.

## Test surface

Existing tests should pass unchanged. Add one new unit test:

1. `canonicalize_within_dir_returns_both_canonical_paths` — assert the returned tuple contains the
   canonical forms (not the input) of both arguments.

The existing `ingest_single_file_rejects_path_outside_knowledge_dir` test already covers the
containment check; the symlink-escape test from the single-file PR also still applies.

## Trade-offs

- **Signature change to an existing helper.** `validate_within_dir` is called from multiple write
  paths. The thin-wrapper approach preserves source compatibility; the full migration does not.
- **Error-path shape changes.** The current `ingest_single_file` has separate error messages for
  "cannot access file" vs. "cannot access knowledge directory" vs. "path escapes". The consolidated
  helper can still distinguish these via context chaining, but the callers need to pass the right
  context.
- **Low absolute impact.** Four syscalls vs. two is measurable only in microbenchmarks. The refactor
  is motivated by readability and defense against future drift, not performance.

## When to do this

Defer until either:

- A batch `--file` mode is added (multiple files per invocation), at which point the syscall
  overhead starts to matter
- Any PR that touches `validate_within_dir` for another reason
- A code-simplicity pass on `src/ingest.rs`

## References

- ce-review run artifact:
  `.context/compound-engineering/ce-review/2026-04-06-single-file-ingest/summary.md`
- `src/ingest.rs:711-825` — `ingest_single_file` (call site with duplication)
- `src/ingest.rs:1082-1098` — `validate_within_dir` (current helper)
- `src/ingest.rs:688-1044` — `add_pattern`, `update_pattern`, `append_to_pattern` (other consumers
  of `validate_within_dir`)
