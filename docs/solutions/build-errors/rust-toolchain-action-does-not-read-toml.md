---
title: "dtolnay/rust-toolchain does not read rust-toolchain.toml automatically"
date: 2026-03-27
category: build-errors
module: .github/workflows
problem_type: configuration_issue
component: ci.yml
symptoms:
  - "CI jobs (clippy, test, doc) fail at the dtolnay/rust-toolchain step"
  - "Gateway ci job silently passes when upstream jobs are skipped"
root_cause: action_misconfiguration
resolution_type: configuration_change
severity: medium
tags:
  - github-actions
  - rust-toolchain
  - ci
  - toolchain-drift
  - cargo-cache
status: resolved
---

# dtolnay/rust-toolchain does not read rust-toolchain.toml automatically

## Problem

When configuring GitHub Actions CI for a Rust project, `dtolnay/rust-toolchain@master` requires an
explicit `toolchain` input and does not read `rust-toolchain.toml`. Removing the hardcoded version
to eliminate drift between CI and the project's toolchain spec broke 3 of 5 CI jobs.

## Symptoms

- CI jobs (clippy, test, doc) fail at the `dtolnay/rust-toolchain@master` step when the `toolchain`
  input is omitted
- Duplicate toolchain version hardcoded in workflow YAML (`toolchain: "1.85"`) alongside
  `rust-toolchain.toml` — a silent drift risk
- Gateway `ci` job reports success even when upstream jobs are skipped (not failure, not cancelled)

## What Didn't Work

- **Removing the `toolchain` input from `dtolnay/rust-toolchain@master`**: The action has no logic
  to parse `rust-toolchain.toml`. Without an explicit input, it errors out.
- **Using `dtolnay/rust-toolchain@stable` with `toolchain: file`**: The value `file` is not a
  recognized toolchain specifier. The action interprets it literally as a toolchain named "file".

## Solution

Replace `dtolnay/rust-toolchain` + `Swatinem/rust-cache` with
`actions-rust-lang/setup-rust-toolchain`, and fix the gateway job logic.

### Toolchain setup — before

```yaml
- uses: dtolnay/rust-toolchain@master
  with:
    toolchain: "1.85"
    components: clippy, rustfmt

- uses: Swatinem/rust-cache@v2
```

### Toolchain setup — after

```yaml
- uses: actions-rust-lang/setup-rust-toolchain@v1
  with:
    rustflags: ""
```

No `toolchain` or `components` input needed — the action reads them from `rust-toolchain.toml`. The
separate `Swatinem/rust-cache` step is also removed because `setup-rust-toolchain` includes built-in
cargo caching.

The `rustflags: ""` is critical: without it, the action sets `RUSTFLAGS="-D warnings"` by default,
which conflicts with passing `-- -D warnings` directly to clippy.

### Gateway job — before

```yaml
ci:
  needs: [fmt, clippy, test, deny, doc]
  steps:
    - run: |
        if [[ "${{ contains(needs.*.result, 'failure') }}" == "true" ]] || \
           [[ "${{ contains(needs.*.result, 'cancelled') }}" == "true" ]]; then
          exit 1
        fi
```

### Gateway job — after

```yaml
ci:
  needs: [fmt, clippy, test, deny, doc]
  if: always()
  steps:
    - run: |
        results=("${{ needs.fmt.result }}" "${{ needs.clippy.result }}" "${{ needs.test.result }}" "${{ needs.deny.result }}" "${{ needs.doc.result }}")
        for r in "${results[@]}"; do
          if [[ "$r" != "success" ]]; then
            echo "Job failed with result: $r"
            exit 1
          fi
        done
```

## Why This Works

**Toolchain:** `dtolnay/rust-toolchain` is a lightweight wrapper around `rustup` that takes an
explicit toolchain name. It has no file-detection logic. `actions-rust-lang/setup-rust-toolchain`
was specifically designed to honor `rust-toolchain.toml`, making the file the single source of truth
across local dev and CI.

**Gateway:** GitHub Actions job results include `skipped` as a possible value, which is neither
`failure` nor `cancelled`. The denylist approach silently passes on skipped jobs. The allowlist
approach (`!= "success"` means fail) treats any non-success state as a failure.

**Rustflags:** `setup-rust-toolchain` assumes most users want warnings-as-errors globally via
`RUSTFLAGS`. When clippy is invoked with its own `-- -D warnings`, the flags conflict. Setting
`rustflags: ""` gives full control to each job's invocation.

## Prevention

- **Single source of truth for toolchain:** Use `rust-toolchain.toml` as the canonical spec. CI
  actions must read from it, never hardcode a version.
- **Fail-closed gateway jobs:** Use allowlist logic (`result == "success"`) rather than denylist
  logic (`result != "failure"`). Any unexpected state should block merging.
- **Always add `if: always()` to gateway jobs:** Without it, the gateway is skipped when
  dependencies are skipped, which GitHub treats as a passing check.
- **Audit action defaults:** When adopting a new GitHub Action, read its docs for default
  environment variable injection. Override explicitly when they conflict with your configuration.

## Related Issues

- Requirements: `docs/brainstorms/2026-03-27-ci-workflow-requirements.md` (R4: toolchain from file)
- Foundational context: `docs/brainstorms/2026-03-24-phase0-project-infrastructure-requirements.md`
  (R2: toolchain pinning)
