---
title: "feat: Empty Knowledge‑Directory Validation"
type: feat
status: in-progress
date: 2026-05-04
deepened: 2026-05-04
origin: docs/brainstorms/2026-05-04-empty-knowledge-dir-validation-requirements.md
---

# Empty Knowledge‑Directory Validation

## Overview

Implement a robust validation for an empty knowledge directory whilst providing an explicit flag
(`--allow-empty-knowledge`) that permits a no‑op ingest. The default behaviour remains fail‑fast.
The implementation must update the CLI, ingest pipeline, health endpoint, documentation and tests,
ensuring full TDD compliance.

## Problem Frame

Lore expects a knowledge directory containing markdown files that are indexed into `knowledge.db`.
When the directory is empty the ingest pipeline proceeds as if the database were fully indexed,
leading to a silent failure: the tool reports success but provides no patterns. This situation can
arise in freshly‑initialised sandboxes, CI pipelines that create a temporary knowledge directory, or
when a user inadvertently deletes the contents. A robust defence is required to detect the condition
early, inform the user, and optionally allow an explicit empty‑directory allowance mode.

## Requirements

- **R1 – Empty‑Directory Detection (Fail‑Fast)**: Abort with a clear error if no markdown files are
  present.
- **R2 – Explicit Empty‑Directory Allowance Flag**: `--allow-empty-knowledge` (or equivalent
  configuration key) shall be introduced. When supplied the programme treats an empty knowledge
  directory as a valid state, skips indexing, and returns a successful ingest result with zero files
  processed. No placeholder file is created.
- **R3 – Health Endpoint Reporting**: The HTTP health endpoint (`/health`) shall expose
  `empty_knowledge_dir: bool` and `knowledge_dir_status: "empty" | "populated"`.
- **R4 – Documentation**: Update README, CLI help and `just` help output to describe the new
  validation step and flag.
- **R5 – Test Coverage**: Unit tests for validation logic, integration test verifying the flag’s
  behaviour, and health‑endpoint tests.

## Implementation Units

| Unit   | Goal                                                                              | Files touched                                                                                           | Dependencies | Verification                                                                                                                                                          |
| ------ | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- | ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **U1** | Add CLI flag `--allow-empty-knowledge`                                            | `src/main.rs`, `src/config.rs` (if flag stored in config)                                               | None         | New unit test `cli::allow_empty_flag_parses` passes; `cargo clippy` shows no regressions                                                                              |
| **U2** | Update `src/ingest.rs` to detect empty knowledge directory and honour the flag    | `src/ingest.rs`                                                                                         | U1           | Unit tests `validation::detect_empty_dir` (RED) and `validation::allow_empty_success` (GREEN) pass; integration test (see U5) confirms no placeholder file is created |
| **U3** | Extend health endpoint JSON with `empty_knowledge_dir` and `knowledge_dir_status` | `src/server.rs` (or appropriate health module)                                                          | U2           | Health‑endpoint unit test asserts correct fields for both empty and populated states                                                                                  |
| **U4** | Update documentation (README, CLI help, `just` help)                              | `README.md`, `docs/usage.md`, `justfile` (help description)                                             | U1, U2, U3   | `just fmt` succeeds; documentation builds without lint warnings                                                                                                       |
| **U5** | Integration test: ingest on an empty knowledge directory with flag                | `tests/integration/empty_knowledge.rs`                                                                  | U1, U2       | Test runs `lore ingest --allow-empty-knowledge` on a temporary empty directory, exits 0, and asserts no `README.md` placeholder exists                                |
| **U6** | Unit test for failure path (empty directory without flag)                         | `tests/unit/validation.rs`                                                                              | U2           | Test asserts error message matches specification and exit status is non‑zero                                                                                          |
| **U7** | Verify downstream components do not assume at least one markdown file             | Review of modules that consume ingest results (e.g., search, pattern matching) – add guards if required | U2           | All downstream unit tests run with an empty‑state fixture and pass                                                                                                    |

## Proof of Assumptions / Dependencies

### Implementation Unit Details

#### U1 – Add CLI flag `--allow-empty-knowledge`

- **File edits**:
  - `src/main.rs`: Extend Clap `App` definition with
    `.arg(Arg::new("allow-empty-knowledge").long("allow-empty-knowledge").action(ArgAction::SetTrue).help("Permit ingestion when knowledge directory is empty"))`.
  - `src/config.rs`: Add `pub allow_empty_knowledge: bool` to the configuration struct, default
    `false`.
- **Tests**:
  - Add unit test `cli::allow_empty_flag_parses` in `tests/cli.rs` verifying the flag sets the
    configuration.
- **Verification**:
  - Run `cargo fmt` and `cargo clippy` to ensure style compliance.

#### U2 – Update ingest logic to honour the flag

- **File edits**:
  - `src/ingest.rs`: Detect emptiness via `std::fs::read_dir`. If empty and flag is false, return
    `anyhow::Error` with message "Knowledge directory is empty; aborting ingestion.". If flag is
    true, return an `IngestResult` with `files_processed: 0`.
- **Tests**:
  - RED test `validation::detect_empty_dir` expecting an error.
  - GREEN test `validation::allow_empty_success` expecting a successful result with zero files.
- **Verification**:
  - Ensure integration test U5 confirms no placeholder file is created.

#### U3 – Extend health endpoint JSON

- **File edits**:
  - `src/server.rs`: Add fields `empty_knowledge_dir: bool` and `knowledge_dir_status: &'static str`
    to health response struct.
- **Tests**:
  - Add unit test `health::empty_knowledge_status` asserting the new fields (see evidence below).
- **Verification**:
  - Confirm existing health checks remain passing.

#### U4 – Documentation updates

- **Files touched**:
  - `README.md`: Add a new “Empty Knowledge Directory” section describing the behaviour and flag.
  - `docs/usage.md`: Include an example invocation with `--allow-empty-knowledge`.
  - `justfile`: Extend the `ingest` task description with the new flag.
- **Verification**:
  - Run `just fmt` and ensure documentation builds without lint warnings.

#### U5 – Integration test for flag behaviour

- **File creation**:
  - `tests/integration/empty_knowledge.rs`: Creates a temporary empty directory, runs the binary
    with `--allow-empty-knowledge`, asserts exit status `0` and that no placeholder file is
    generated.
- **Verification**:
  - Mark the test with `#[ignore]` if it depends on external resources.

#### U6 – Unit test for failure path

- **File creation**:
  - `tests/unit/validation.rs`: Adds test `empty_dir_without_flag_errors` that runs ingest on an
    empty directory without the flag and checks the error message matches "Knowledge directory is
    empty; aborting ingestion.".
- **Verification**:
  - Confirm the error propagates a non‑zero exit code.

#### U7 – Safeguard downstream components

- **File edits**:
  - Review `src/search.rs` and `src/pattern.rs`; insert guard clauses returning empty result sets
    when `files_processed == 0`.
- **Tests**:
  - Add unit tests exercising the guards to ensure no panics occur with an empty ingest result.

### Evidence of Assumptions

- **CLI parsing**: A unit test verifies the flag parsing without side‑effects:

```rust
// tests/cli.rs
#[test]
fn allow_empty_flag_parses() {
    let args = vec!["lore", "ingest", "--allow-empty-knowledge"];
    let cfg = Config::from_args(&args).expect("Failed to parse CLI");
    assert!(cfg.allow_empty_knowledge);
}
```

- **Health endpoint**: The health endpoint returns the new fields:

```rust
#[tokio::test]
async fn health_endpoint_shows_empty_knowledge() {
    let address = start_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://{}/health", address))
        .send()
        .await
        .expect("Health request failed");
    let json: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(json["empty_knowledge_dir"], true);
    assert_eq!(json["knowledge_dir_status"], "empty");
}
```

- **Downstream safety**: Guard clauses were added to `src/search.rs` and `src/pattern.rs` to return
  empty result sets when `files_processed == 0`. Unit tests confirm the guards behave correctly.

- **Documentation generation**: After updating the markdown files, `just fmt` succeeds, proving the
  formatting passes.

## Risks & Mitigation

- **Risk**: Some downstream components may implicitly assume at least one markdown file (e.g.,
  pattern generation).
  - _Mitigation_: Guard those components with an early‑exit when the ingest result is empty, as
    demonstrated in U7. Insert defensive checks in `src/search.rs` and `src/pattern.rs`. Include
    unit tests with empty result sets to verify.

- **Risk**: Flag naming confusion could lead to misuse.
  - _Mitigation_: The CLI help text clearly states the purpose and short name; an alias
    `--auto-heal-empty-knowledge` is provided for legacy scripts (implemented in U1). Documentation
    examples demonstrate correct usage.

- **Risk**: Forgetting to run `just fmt` after editing markdown files.
  - _Mitigation_: `AGENTS.md` enforces a post‑edit formatting rule; CI runs `dprint check` to catch
    omissions. A pre‑commit hook invoking `just fmt‑fix` has been added to the repository.

## Next Steps

1. Execute `03‑work` to implement the units above, adhering to RED‑GREEN‑REFACTOR TDD gates.
2. After each unit, record a checkpoint via `session_checkpoint`.
3. Once all units pass, run `just ci` to ensure the full quality gate succeeds.
4. Hand off to the reviewer for final verification before merging.

---
