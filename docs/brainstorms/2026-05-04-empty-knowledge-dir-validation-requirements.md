---
date: 2026-05-04
topic: empty-knowledge-dir-validation
---

# Empty Knowledge‑Directory Validation

## Problem Frame

Lore expects a knowledge directory containing markdown files that are indexed into `knowledge.db`.
When the directory is empty the ingest pipeline proceeds as if the database were fully indexed,
leading to a silent failure: the tool reports success but provides no patterns. This situation can
arise in freshly‑initialised sandboxes, in CI pipelines that create a temporary knowledge directory,
or when a user inadvertently deletes the contents. A robust defence is required to detect the
condition early, inform the user, and optionally allow an explicit auto‑heal mode.

## Requirements

**R1 – Empty‑Directory Detection (Fail‑Fast)**\
The programme must, after configuration, inspect `knowledge_dir` and abort with a clear error if no
markdown files are present. The error text shall read:

> Knowledge directory is empty – run `lore init` or add at least one `.md` file.

The exit status must be non‑zero.

**R2 – Explicit Empty‑Directory Allowance Flag**\
A CLI flag `--allow-empty-knowledge` (or the equivalent configuration key) shall be introduced. When
this flag is supplied the programme will treat an empty knowledge directory as a valid state, skip
indexing, and return a successful ingest result with zero files processed. No placeholder file is
created; the ingest pipeline simply short‑circuits.

**R3 – Health Endpoint Reporting**\
The HTTP health endpoint (`/health`) shall expose two fields:

- `empty_knowledge_dir: bool` – true when the directory is empty.\
- `knowledge_dir_status: "empty" | "populated"` – a human‑readable status.

**R4 – Documentation**\
All user‑facing documentation (README, usage help, and the `just` help output) shall be updated to
describe the new validation step and the optional flag.

**R5 – Test Coverage**\

- Unit test `validation::ensure_nonempty` verifies that an empty directory returns the expected
  error and that a non‑empty directory returns `Ok(())`.\
- Integration test runs `lore ingest` on a temporary empty knowledge directory, asserts the process
  exits with the error message, and confirms that `--allow-empty-knowledge` allows the ingest to
  succeed with zero files processed and no placeholder file is created.\
- Health endpoint tests confirm the JSON fields reflect the correct state for both scenarios.

## Success Criteria

- Running `lore ingest` on an empty knowledge directory without the flag aborts with the prescribed
  error message.\
- Supplying `--allow-empty-knowledge` allows the ingest to succeed with zero files processed,
  without creating any placeholder file.\
- The health endpoint accurately reports the empty or populated status.\
- Documentation and `just` help output mention the validation and flag.\
- All new tests pass in CI.

## Scope Boundaries

- The validation concerns only the presence of markdown files; it does not enforce a minimum file
  count beyond one.\
- Permission errors (e.g., unreadable directory) are reported via the existing error handling path,
  not as a separate requirement.\
- No additional file‑system side‑effects are introduced aside from the optional placeholder file.\
- The feature does not alter the ingest algorithm beyond the initial guard.

## Key Decisions

- **Default behaviour is fail‑fast** – this aligns with the principle of failing loudly rather than
  proceeding silently.\
- **Explicit flag for auto‑heal** – provides CI pipelines a deterministic way to enable the
  convenience behaviour without compromising safety for end‑users.\
- **Empty‑directory allowance** – the ingest pipeline returns a zero‑file result; no placeholder
  file is needed.\
- **Health reporting** – chosen to integrate with existing health JSON rather than a separate
  endpoint.

## Dependencies / Assumptions

- The CLI parsing library (`clap` or equivalent) can accept the new flag without breaking existing
  options.\
- The `just` help system can be extended to expose the flag description.\
- No stub file creation is required; the ingest simply returns an empty result when the directory is
  empty.

## Outstanding Questions

- Should the flag be named `--allow-empty-knowledge` or `--auto‑heal-empty-knowledge`? The former is
  shorter but the latter is more explicit.\
- Are there any downstream components that assume at least one markdown file exists, or is the
  empty‑state handling sufficient?\
- Do we need to add a warning when the placeholder is created, or is silent creation acceptable?

## Next Steps

→ `/ce:plan` – produce a structured implementation plan that details the required source‑file
changes, test additions, documentation updates, and CI modifications.
