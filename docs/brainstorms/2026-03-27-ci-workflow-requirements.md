---
date: 2026-03-27
topic: ci-workflow
---

# GitHub Actions CI Workflow

## Problem Frame

Lore has a local CI pipeline (`just ci`) but no automated CI. Branch protection rules need a
required status check to enforce, and squash-merge-only + up-to-date-with-base policies need a
passing workflow to gate on.

## Requirements

- R1. GitHub Actions workflow with separate jobs for each gate (fmt, clippy, test, deny, doc)
- R2. A `ci` gateway job that depends on all gate jobs — branch protection points at this single
  status check
- R3. Workflow triggers on PRs targeting `main` and pushes to `main`
- R4. Rust toolchain installed from `rust-toolchain.toml` (1.85 + clippy + rustfmt)
- R5. Dev tools installed in CI: `just`, `dprint`, `cargo-deny` (only in jobs that need them)
- R6. Cargo dependency and build caching for reasonable build times
- R7. Linux x86_64 only (matching `deny.toml` targets), no matrix builds

## Success Criteria

- PR to `main` triggers the workflow and all steps pass
- The workflow name/job is available as a required status check in branch protection settings
- A formatting or clippy violation in a PR causes the workflow to fail

## Scope Boundaries

- **In:** Single CI workflow file with fan-out jobs, tool installation, caching
- **Out:** Release automation, nightly builds, macOS/Windows matrix, deploy steps, branch protection
  configuration (manual step by user after CI is verified)

## Key Decisions

- **Squash merges only + require up-to-date:** User will configure this in GitHub repo settings /
  branch protection after CI is verified working. The workflow just needs to exist and pass.
- **`just` recipes as job commands:** Each CI job runs the corresponding `just` recipe rather than
  duplicating shell logic. If local CI passes, remote CI passes.
- **Gateway job pattern:** The `ci` job has no steps of its own — it only depends on all gate jobs
  via `needs:`. Branch protection requires only this one check, so adding/removing gate jobs doesn't
  require updating branch protection rules.

## Next Steps

→ `/ce:plan` for implementation planning, or proceed directly to work given the lightweight scope.
