---
title: "`lore ingest --force` has two unrelated meanings depending on `--file`"
priority: P2
category: cli-readiness
status: ready
created: 2026-04-06
source: ce-review (feat/single-file-ingest)
files:
  - src/main.rs:57-89
  - src/ingest.rs:711-825
related_pr: feat/single-file-ingest
---

# `lore ingest --force` has two unrelated meanings depending on `--file`

## Context

The single-file ingest PR introduced `lore ingest --file <path>` and reused the existing `--force`
flag for a second purpose: when `--file` is present, `--force` overrides `.loreignore` for that one
file. When `--file` is absent, `--force` still means "full re-index". Three reviewers flagged this
independently during ce-review (correctness, maintainability, cli-readiness), merging to a P2 with
~0.95 confidence.

Current invocation matrix:

| Invocation                          | Meaning                                         |
| ----------------------------------- | ----------------------------------------------- |
| `lore ingest`                       | Delta ingest since last commit                  |
| `lore ingest --force`               | Full re-index of the whole knowledge base       |
| `lore ingest --file foo.md`         | Index one file without a git commit             |
| `lore ingest --file foo.md --force` | Index one file, overriding `.loreignore` for it |

The PR documented the overload via an `after_help` block on the `Ingest` subcommand with EXAMPLES
and EXIT CODES sections, which is a reasonable short-term mitigation. The deeper issue is that
`--force` now means "do something aggressive" where the "something" is context-dependent, which is
exactly the shape of flag that gets misused. A user habituated to "`--force` means blow away and
rebuild" who runs `lore ingest --force --file draft.md` gets a completely different operation
without warning.

## Proposed fix

Split the flag:

1. Add a dedicated `--override-ignore` (or `--no-ignore`, matching ripgrep convention) for the
   single-file `.loreignore` bypass. Accept only in combination with `--file`.
2. Keep `--force` meaning strictly "full re-index" and make it `conflicts_with = "file"` via clap.
3. For one release, keep accepting `--force` alongside `--file` with a deprecation warning so anyone
   who learned the current shape is not broken immediately.

```rust
Ingest {
    #[arg(long, conflicts_with = "file")]
    force: bool,

    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,

    /// Override .loreignore for --file. Only valid with --file.
    #[arg(long, requires = "file")]
    override_ignore: bool,
},
```

In `dispatch_ingest`, translate `override_ignore || (file.is_some() && force)` into the
`force_override_ignore` argument for `ingest_single_file`, and emit a deprecation warning on stderr
when the old `--force --file` combination is used.

## Test surface

Add CLI-binary tests in `tests/smoke.rs`:

1. `ingest_force_conflicts_with_file_errors_cleanly` — `lore ingest --force --file x.md` without the
   deprecation grace period eventually exits with a clap conflict error.
2. `ingest_override_ignore_without_file_errors` — `lore ingest --override-ignore` alone is a usage
   error.
3. `ingest_override_ignore_with_file_indexes_loreignored_file` — the new flag behaves the same as
   the current `--file --force` combination.

Update `tests/smoke.rs::ingest_help_shows_file_flag_and_exit_codes` to also check for
`--override-ignore` in the help output.

## Trade-offs

- **Three coupled changes.** CLI enum, dispatch logic, help text all move together.
- **Deprecation window is awkward** — either accept the old form for a release (requires a
  feature-flag or version check) or break it immediately and note in the changelog.
- **Low practical impact today.** The overload is documented in `after_help`; agents reading the
  help see the four invocation shapes spelled out. A user who passes `--force --file` and gets the
  override-ignore behaviour is technically getting what was documented.

## When to do this

Defer until either:

- A user reports confusion about `--force --file` doing something unexpected
- A new "force" semantic needs room (e.g., "force re-embedding even if content hash unchanged")
- The next PR that touches the `Ingest` subcommand's flag surface

## References

- Plan: `docs/plans/2026-04-06-002-feat-single-file-ingest-plan.md` (Key Technical Decisions, Open
  Questions — Deferred to Implementation)
- ce-review run artifact:
  `.context/compound-engineering/ce-review/2026-04-06-single-file-ingest/summary.md`
- `src/main.rs:57-89` — current flag definitions with `after_help` documentation
