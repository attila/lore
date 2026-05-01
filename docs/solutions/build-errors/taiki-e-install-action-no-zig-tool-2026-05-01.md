---
title: "taiki-e/install-action does not ship zig — install separately via mlugg/setup-zig"
date: 2026-05-01
category: build-errors
module: ci
problem_type: build_error
component: ci
symptoms:
  - "ERROR Fatal error: For crate zig: zig is not found"
  - "INFO resolve: Resolving package: 'zig@=0.13.0'"
  - "##[error]Process completed with exit code 76 in cross-compile workflow job"
  - "All matrix legs (4 targets) fail in <20s before any actual cargo build runs"
root_cause: tool_registry_missing_entry
resolution_type: action_swap
severity: high
related_pr: 35
related_commit: 5f307a4
tags:
  - github-actions
  - taiki-e-install-action
  - zig
  - cargo-zigbuild
  - cross-compile
  - mlugg-setup-zig
  - agent-hallucination
  - tool-registry
---

# `taiki-e/install-action` does not ship `zig` — install separately via `mlugg/setup-zig`

## Problem

PR #35's cross-compile CI job tried to install both zig and cargo-zigbuild in a single
`taiki-e/install-action@v2` step using the comma-separated `tool:` syntax:

```yaml
- uses: taiki-e/install-action@v2
  with:
    tool: zig@0.13.0,cargo-zigbuild@0.20.0
```

All four matrix legs (linux-gnu, linux-musl, both apple-darwin) failed in 11-16 seconds with:

```
INFO resolve: Resolving package: 'zig@=0.13.0'
ERROR Fatal error:
  × For crate zig: zig is not found
  ╰─▶ zig is not found
##[error]Process completed with exit code 76.
```

The action's tool registry includes `cargo-zigbuild` but not `zig` itself. When given an unknown
tool name, install-action falls through to its crates.io fallback path (effectively
`cargo install <name>`); there is no crate named `zig` on crates.io, so the fallback fails with the
message above. Cargo-zigbuild requires `zig` on PATH at build time, so without it nothing further
would have worked anyway.

## Root cause

`taiki-e/install-action` ships a curated tool manifest. Inclusion criteria favour Rust ecosystem
tools that distribute prebuilt binaries via GitHub Releases under conventional naming. zig itself is
published by the Zig project on `ziglang.org` (and its own GitHub Releases), not in install-action's
manifest. The action treats unknown names as crates.io fallbacks rather than as missing-tool errors,
which makes the failure mode look like a network-or-version issue rather than a "this tool isn't
supported" issue.

The decision to use install-action for zig in the original workflow was load-bearing on a
feasibility-review agent's claim during plan-deepening that the action had "first-class entries for
both zig and cargo-zigbuild". The claim was wrong but plausible and was not verified against the
action's TOOLS.md before encoding in the workflow.

## Working fix

Split the install into two steps: `mlugg/setup-zig` for zig, `taiki-e/install-action` for
cargo-zigbuild only.

```yaml
- uses: mlugg/setup-zig@d1434d08867e3ee9daa34448df10607b98908d29 # v2.2.1
  with:
    version: 0.13.0
- uses: taiki-e/install-action@6ef672efc2b5aabc787a9e94baf4989aa02a97df # v2.70.3
  with:
    tool: cargo-zigbuild@0.20.0
- run: cargo zigbuild --release --target ${{ matrix.target }}
```

`mlugg/setup-zig@v2.2.1` (commit `d1434d08867e3ee9daa34448df10607b98908d29`, published 2026-01-19)
is the actively maintained fork of `goto-bus-stop/setup-zig`. It downloads zig from `ziglang.org`
verified against the project's published checksums, places it on PATH, and is the standard pattern
in real-world Rust release pipelines that use cargo-zigbuild.

After this swap, all four cross-compile matrix legs in PR #35 succeeded (commit `5f307a4`).

## Why not pip install ziglang

`pip install ziglang==0.13.0` is the dominant pattern in some Rust release pipelines (uv, ruff) and
is recommended in `cargo-zigbuild`'s own README. The project this fix landed in deliberately avoids
introducing a Python dependency to a Rust+zig project, even for a one-line install convenience.
`mlugg/setup-zig` is the action-shaped alternative that respects that constraint.

## Prevention

**Verify load-bearing agent claims about specific registries before encoding in shippable code.**
Agent claims about "is X in tool registry Y" or "does action Z support feature W" are exactly the
kind of claim that hallucinates plausibly. Check the actual TOOLS.md / action documentation when the
claim is going to be committed. This particular failure cost one CI roundtrip; the cost would scale
with pipeline-test-cycle time on more complex workflows.

**Pin `taiki-e/install-action` tool entries explicitly with versions.** The `@<version>` suffix on
each tool name (e.g. `cargo-zigbuild@0.20.0`) gives install- action a chance to fail-fast on a
registry-miss in some cases where an unversioned name might be ambiguous. Versioned pins also keep
CI deterministic across registry updates.

**Treat fast matrix failures as different from slow ones.** A 4-target matrix that all fails in
under 20 seconds means the build never started — usually an install or config issue, not a real
cross-compile bug. A 4-target matrix where one target fails in 1-2 minutes is a real compile failure
(like the sibling sqlite-vec musl issue,
[`sqlite-vec-musl-cross-compile-u_int8_t-typedef-2026-05-01.md`](sqlite-vec-musl-cross-compile-u_int8_t-typedef-2026-05-01.md)).
The two failure modes have different debug paths and the timing is the cheapest signal.

## Related

- Sibling cross-compile issue:
  [`sqlite-vec-musl-cross-compile-u_int8_t-typedef-2026-05-01.md`](sqlite-vec-musl-cross-compile-u_int8_t-typedef-2026-05-01.md)
  — surfaced by the same PR after this fix unblocked the actual build.
- Adjacent CI/build-action gotcha:
  [`rust-toolchain-action-does-not-read-toml.md`](rust-toolchain-action-does-not-read-toml.md) —
  same problem class (CI action's behaviour silently differs from assumed contract).
- Origin plan:
  [`docs/plans/2026-04-30-001-feat-release-process-plan.md`](../../plans/2026-04-30-001-feat-release-process-plan.md)
  — the deepening pass that hallucinated install-action's zig support.
- mlugg/setup-zig: <https://github.com/mlugg/setup-zig>
- taiki-e/install-action TOOLS.md (canonical list of supported tools):
  <https://github.com/taiki-e/install-action/blob/main/TOOLS.md>
