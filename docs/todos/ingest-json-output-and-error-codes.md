---
title: "`lore ingest` has no --json output; errors lack machine-readable codes"
priority: P2
category: cli-readiness
status: ready
created: 2026-04-06
source: ce-review (feat/single-file-ingest)
files:
  - src/main.rs:264-420
  - src/ingest.rs:62-76
related_pr: feat/single-file-ingest
---

# `lore ingest` has no --json output; errors lack machine-readable codes

## Context

`Cli` defines a global `--json` flag whose help text reads "Output results as JSON (for search and
list commands)". `search` and `list` honour it; `ingest` does not. During ce-review of the
single-file ingest PR, the cli-readiness reviewer flagged this as a P2 — single-file ingest is the
exact command shape an agent will invoke most often (edit → ingest → search loop), and it is the
most valuable place to emit structured output.

Two related issues:

1. **No structured output.** An agent running `lore ingest --file draft.md` gets
   `Done (single-file): draft.md → 3 chunks` on stderr and nothing on stdout. To extract the one
   datum that matters (`was this file indexed, and how many chunks?`), the agent has to regex the
   summary line, which will break the first time the human-facing copy changes.

2. **No error codes.** Error messages are genuinely specific — file-not-found, wrong extension,
   path-escapes-knowledge-dir, excluded-by-loreignore — but they are all free-form English prose
   with no stable machine-readable tag. An agent that wants to decide "retry with `--force`?" vs.
   "give up because file doesn't exist?" vs. "rename to `.md`?" must substring-match on the English
   error text, which is the second-most fragile thing agents do (after parsing tables).

Both issues are tied: adding `--json` unlocks a natural place to embed error codes.

## Proposed fix

1. Derive `Serialize` on `IngestResult`, `IngestMode`, and a new `IngestError` struct:

   ```rust
   #[derive(Serialize)]
   pub struct IngestError {
       pub code: IngestErrorCode,
       pub message: String,
       pub file: Option<String>,
   }

   #[derive(Serialize)]
   #[serde(rename_all = "snake_case")]
   pub enum IngestErrorCode {
       FileNotFound,
       NotRegularFile,
       UnsupportedExtension,
       OutsideKnowledgeDir,
       IgnoredByLoreignore,
       EmbeddingFailed,
       IndexWriteFailed,
   }
   ```

2. Change `IngestResult::errors` from `Vec<String>` to `Vec<IngestError>` so every error site
   classifies its failure at the point of origin. Internal call sites (walk-based ingest,
   reconciliation, single-file ingest) each tag their errors.

3. Honour `--json` in `cmd_ingest`:
   - Suppress `print_ingest_summary` and the `on_progress` callback's stderr writes.
   - After the ingest returns, print one JSON object on **stdout** at the end with `mode`, `path`,
     `files_processed`, `chunks_created`, `reconciled_removed`, `reconciled_added`, `errors[]`, and
     a stable top-level `status: "ok" | "partial" | "failed"`.

4. Update the global `--json` flag help to drop the `(for search and list commands)` restriction.

## Test surface

Add CLI-binary tests in `tests/smoke.rs`:

1. `ingest_json_outputs_valid_object_on_success` — `lore ingest --json --file <path>` emits a
   parseable JSON object on stdout with `status == "ok"` and the expected `chunks_created`.
2. `ingest_json_outputs_error_code_on_extension_rejection` — asserts
   `errors[0].code ==
   "unsupported_extension"`.
3. `ingest_json_outputs_error_code_on_loreignore_rejection` — asserts
   `errors[0].code == "ignored_by_loreignore"`.
4. `ingest_json_suppresses_stderr_progress` — stderr is empty when `--json` is set.

## Trade-offs

- **Breaking change to `IngestResult::errors`.** Going from `Vec<String>` to `Vec<IngestError>`
  touches every call site that populates the vec — `ingest`, `full_ingest`, `delta_ingest`,
  `reconcile_ignored`, `ingest_single_file`, `process_change`. Not huge, but not local either.
- **Error codes are a commitment.** Once the enum variants are exposed via `--json`, renaming them
  is a backwards-incompatible change. Need to pick names that will age well.
- **Test isolation.** The existing `smoke.rs` tests hit the CLI binary without a running Ollama
  embedder. `--json` tests for the happy path of `ingest` need to avoid the embed call (cover via
  the library-level `ingest_single_file` with `FakeEmbedder` and a CLI test that exercises only
  error paths, or gate the happy-path CLI test behind Ollama availability).

## When to do this

Defer until either:

- The Pattern QA skill (Up Next on the roadmap) needs structured ingest output to parse results
- A user reports an agent harness regex-ing summary lines and breaking on a copy change
- Any PR that touches `IngestResult` for another reason, so the `Vec<String>` → `Vec<IngestError>`
  migration rides along

## References

- ce-review run artifact:
  `.context/compound-engineering/ce-review/2026-04-06-single-file-ingest/summary.md`
- Plan: `docs/plans/2026-04-06-002-feat-single-file-ingest-plan.md` (Key Technical Decisions)
- `docs/todos/lore-init-json-flag-support.md` — same pattern for `lore init`
- `src/main.rs:264-420` — `cmd_ingest`, `dispatch_ingest`, `print_ingest_summary`
- `src/ingest.rs:62-76` — `IngestResult` struct
