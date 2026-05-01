---
title: "sqlite-vec build fails on x86_64-unknown-linux-musl due to missing BSD u_int*_t typedefs"
date: 2026-05-01
category: build-errors
module: database
problem_type: build_error
component: database
symptoms:
  - "sqlite-vec.c:68:9: error: unknown type name 'u_int8_t' (and u_int16_t, u_int64_t)"
  - "cargo zigbuild --release --target x86_64-unknown-linux-musl fails with exit 101"
  - "Other 3 release targets (linux-gnu, both apple-darwin) build cleanly with the same source"
  - "27 warnings and 3 errors generated. error: failed to run custom build command for sqlite-vec v0.1.7"
root_cause: bsd_compat_dependency
resolution_type: build_config_workaround
severity: high
related_pr: 35
related_commit: 0248c54
upstream_issue: https://github.com/asg017/sqlite-vec/issues/156
tags:
  - cross-compile
  - musl
  - sqlite-vec
  - cargo-zigbuild
  - c-portability
  - bsd-types
  - u_int8_t
  - cflags
  - cc-rs
  - release-pipeline
  - bundled-sqlite
---

# sqlite-vec build fails on `x86_64-unknown-linux-musl` due to missing BSD `u_int*_t` typedefs

## Problem context

While wiring up the tag-triggered release pipeline for `lore` (PR #35), we needed to cross-compile
from a Linux host to four targets via `cargo-zigbuild`: `x86_64-unknown-linux-gnu`,
`x86_64-unknown-linux-musl`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`. Three of the four
targets built cleanly. The musl target failed during the `sqlite-vec v0.1.7` build script with:

```
warning: sqlite-vec@0.1.7: sqlite-vec.c:68:9: error: unknown type name 'u_int8_t'
warning: sqlite-vec@0.1.7: sqlite-vec.c:69:9: error: unknown type name 'u_int16_t'
warning: sqlite-vec@0.1.7: sqlite-vec.c:70:9: error: unknown type name 'u_int64_t'
warning: sqlite-vec@0.1.7: 27 warnings and 3 errors generated.
error: failed to run custom build command for `sqlite-vec v0.1.7`
```

The offending source (`sqlite-vec.c` lines 60-74, in the upstream-vendored amalgamation pulled in
via the `sqlite-vec` crate) sits inside a guard that excludes Windows, Emscripten, Cosmopolitan, and
WASI but applies to every other platform:

```c
#ifndef _WIN32
#ifndef __EMSCRIPTEN__
#ifndef __COSMOPOLITAN__
#ifndef __wasi__
typedef u_int8_t uint8_t;
typedef u_int16_t uint16_t;
typedef u_int64_t uint64_t;
#endif
#endif
#endif
#endif
```

`<stdint.h>` is included on line 10, so the standard `uint8_t`/`uint16_t`/`uint64_t` types are
already in scope. The typedefs assume the BSD-style `u_int*_t` aliases also exist — a portability
assumption that holds on glibc (which exposes them via `<sys/types.h>` for BSD compatibility) and on
the macOS libc, but not on musl, which provides only the C99-standard `uint*_t` names. That mismatch
is exactly why `linux-gnu` and both `apple-darwin` targets compiled without complaint while
`linux-musl` did not — the bug is in the C source's portability assumptions, not in `cargo-zigbuild`
or the zig C frontend.

The solution space was constrained by an existing project convention: the `rust/sqlite.md` rule (and
the `project_binary_size_investigation.md` auto-memory note) mandates the `bundled` feature on
`rusqlite` and explicitly forbids depending on a system SQLite library, in order to eliminate
version drift between developer machines and CI. That meant patching out the bundled C source,
swapping to a system SQLite/sqlite-vec, or introducing a `system-sqlite` feature flag were all off
the table. The workaround had to operate within the bundled-C-source constraint and apply only to
the musl build (so we wouldn't disturb the three already-passing targets), which pointed at
`cc-rs`'s target-scoped `CFLAGS_<target>` env-var hook as the right surface to inject the missing
macro definitions.

> **Upstream tracking.** The same bug was reported upstream in
> [asg017/sqlite-vec#156](https://github.com/asg017/sqlite-vec/issues/156) ("Fail compiled with Rust
> linux-musl") in January 2025 against version 0.1.6, with the identical
> `u_int8_t/u_int16_t/u_int64_t undefined under musl` error. The issue is closed but a code fix did
> not ship in 0.1.7, so the workaround is still required for the version this project pins. The repo
> also has [#226](https://github.com/asg017/sqlite-vec/issues/226) (open) asking whether the project
> is still maintained — relevant context for whether a real upstream fix is likely before we can
> drop the workaround.

## Solution

### Root cause

`sqlite-vec` 0.1.7 contains defensive typedefs at lines 68-70 of `sqlite-vec.c` that go the wrong
direction: `typedef u_int8_t uint8_t;` (and the 16/64-bit variants) define the standard C99 names
_from_ the BSD `u_int*_t` names. glibc's `<sys/types.h>` ships the `u_int*_t` aliases as a
historical BSD-compat gesture, so the code happens to compile on every glibc-based Linux. musl is
strict POSIX and ships only the C99 `uint*_t` names, so on `*-linux-musl` the `u_int*_t` identifiers
are unknown and compilation fails with three errors plus a cascade of warnings. The typedefs are
also redundant on any modern toolchain — `<stdint.h>` is already included on line 10, so `uint8_t`
etc. are in scope before line 68 runs.

### Working fix

A single project-local `.cargo/config.toml` resolves it for both local cross-builds and CI without
touching upstream:

```toml
# Cargo build env vars.
#
# Scoped by env-var name (CFLAGS_<target>) so each entry only takes effect for
# the matching target. Native and other-target builds are unaffected.

[env]
# sqlite-vec 0.1.7 sqlite-vec.c lines 68-70 do `typedef u_int8_t uint8_t;`
# (etc.), assuming the BSD-style `u_int*_t` typedefs are present. glibc provides
# them via <sys/types.h>; musl does not. Predefine them as the standard C99
# `uint*_t` (already in scope from <stdint.h> on line 10) so the typedefs become
# redundant self-typedefs, which C11 §6.7p3 explicitly allows.
#
# Drop this when sqlite-vec ships a fix or we move off the 0.1 line.
CFLAGS_x86_64_unknown_linux_musl = "-Du_int8_t=uint8_t -Du_int16_t=uint16_t -Du_int64_t=uint64_t"
```

`sqlite-vec`'s `build.rs` invokes the `cc` crate (`cc-rs`) to compile the bundled C amalgamation.
`cc-rs` reads `CFLAGS_<target>` where `<target>` is the Rust target triple with hyphens converted to
underscores, so this entry is appended to the C compiler invocation only when building for
`x86_64-unknown-linux-musl`. The preprocessor then rewrites `typedef u_int8_t uint8_t;` to
`typedef uint8_t uint8_t;` before the parser ever sees the BSD names.

### Why the redundant self-typedef is safe

C11 §6.7p3 explicitly permits a typedef name to be redefined within the same scope provided each
declaration names the same type. `typedef uint8_t uint8_t;` satisfies that exactly — same
identifier, same underlying type — so it is a well-formed no-op declaration. Clang (which `zig cc`
uses under the hood for `cargo-zigbuild`) accepts it under C99 mode as well, so the workaround does
not depend on a `-std=c11` bump.

### Why `.cargo/config.toml [env]` is the right home

Cargo's `[env]` table applies to every `cargo` invocation against the workspace, so the same
one-line config fixes local developer builds and the GitHub Actions matrix without per-environment
YAML duplication. Putting `env: { CFLAGS_x86_64_unknown_linux_musl: ... }` on the workflow build
step would split the workaround across the repo and make it invisible to anyone running
`cargo zigbuild` locally; it also runs into Claude's don't-ask Bash permission matcher, which
doesn't whitelist env-var-prefixed commands like `CFLAGS_X=Y cargo …`. A patched `sqlite-vec` fork
would mean carrying a vendored crate just for three lines, and switching to the system SQLite is
forbidden by the project's `rust/sqlite.md` convention (bundled SQLite is mandatory). The
`.cargo/config.toml` approach keeps the fix scoped to a single target, travels with the project, and
is trivially removable once sqlite-vec ships a real fix or we move off the 0.1 line.

### Verification

1. Reproduce: `cargo zigbuild --release --target x86_64-unknown-linux-musl` fails with 27 warnings,
   3 errors, exit 101.
2. Apply: create `.cargo/config.toml` with the block above.
3. Confirm the musl build: re-run the same command — completes clean in ~45 s.
4. Confirm no regression on other targets:
   `cargo zigbuild --release --target x86_64-unknown-linux-gnu` builds clean (the env var is
   target-scoped and ignored here); same for both `*-apple-darwin` targets.
5. CI: PR #35 flipped from red-on-musl / green-elsewhere to all-green on commit `0248c54`.

### Approaches that did not work

- **Bumping `sqlite-vec`** — 0.1.7 is the latest stable on the 0.1 line; 0.1.10-alpha.3 exists but
  is alpha-quality. No safe upgrade.
- **Switching to system SQLite** — forbidden by the project's `rust/sqlite.md` convention; bundled
  SQLite is mandatory.
- **Defining `_WIN32` / `__EMSCRIPTEN__` to skip the offending `#ifndef` block** — those macros gate
  other conditional code paths; flipping them would mis-compile the rest of the amalgamation.
- **Setting `_BSD_SOURCE` / `_DEFAULT_SOURCE`** — musl does not honour these feature-test macros the
  way glibc does, so `u_int*_t` still never enters scope.

## Prevention

The single most leveraged prevention here is what PR #35 already added: a **PR-time cross-compile
smoke job for every release target**. This bug failed only on `x86_64-unknown-linux-musl`; native
development on glibc Linux or macOS would have shipped the regression straight to a release tag. The
cost of a smoke matrix is small (each target is a `cargo build` against a thin set of features), but
the value is exactly this scenario — surfacing musl portability bugs in C dependencies at PR time,
when they cost a config tweak, instead of at tag-push, when they break the release pipeline. Treat
the matrix as load-bearing infrastructure: any future addition of a release target must be mirrored
in the PR smoke job, and any failure in the matrix blocks merge rather than being
retried-until-green.

The secondary prevention is **pattern recognition for any `cc`-compiled crate in the dependency
tree**. `sqlite-vec`, `rusqlite` (bundled), `ring`, `libloading`, `zstd-sys`, and similar crates
ship vendored C that is compiled at build time by `cc-rs`. These are the candidates for musl
portability failures, and the symptom is consistent: the crate builds cleanly on glibc and fails on
musl with `unknown type name '...'` or `implicit declaration of '...'`. The underlying cause is
almost always reliance on glibc's BSD-compat surface that musl deliberately omits.

The specific identifiers to flag during dependency review or upstream-source skimming:

- `u_int8_t`, `u_int16_t`, `u_int32_t`, `u_int64_t`
- `u_long`, `u_short`, `u_char`
- `caddr_t`, `quad_t`, `daddr_t`, `register_t`

None of these are POSIX. All are conditionally provided by glibc's `<sys/types.h>` for historical
BSD compatibility; musl provides none of them. A particularly diagnostic anti-pattern is
`typedef X stdname;` where `X` is a BSD-style identifier and `stdname` is a C99 type already defined
by `<stdint.h>` (e.g., `typedef u_int8_t uint8_t;`) — that line is upstream-buggy on its face, even
if it happens to compile on the maintainer's machine, because the standard name is already in scope
from `<stdint.h>`.

**Default workaround pattern** when a similar issue is found: predefine the BSD type as its C99
equivalent via target-scoped CFLAGS in `.cargo/config.toml [env]`:

```toml
[env]
CFLAGS_x86_64_unknown_linux_musl = "-Du_int8_t=uint8_t -Du_int16_t=uint16_t -Du_int32_t=uint32_t -Du_int64_t=uint64_t"
```

The env-var name itself (`CFLAGS_<target>`) scopes the override to the target triple, so glibc and
Apple builds are untouched. Always pair the workaround with a comment that names (a) the upstream
project and version, (b) a link to the upstream issue if filed, and (c) the explicit drop condition
("remove when sqlite-vec >= 0.1.8 ships" or "remove when we move off the 0.1.x line"). Workarounds
without a written exit condition are how `.cargo/config.toml` accumulates archaeological CFLAGS no
one dares delete.

**When to escalate past the workaround**:

- File an upstream issue with a minimal reproduction — an alpine or `musl/musl-cross` docker
  container plus the failing C snippet. Many C library authors don't run musl in CI and will accept
  a clean patch.
- If upstream is unmaintained, fork or vendor the C source with the offending typedef removed; this
  is preferable to carrying flag overrides indefinitely.
- If the dependency is replaceable, evaluate alternatives — but never at the cost of project-level
  conventions. For lore specifically, bundled SQLite is non-negotiable per `rust/sqlite.md`, so
  "swap out rusqlite" is not on the table even if it would sidestep a future bundled-C portability
  bug.

**Regression net**: the cross-compile smoke matrix in `.github/workflows/ci.yml` _is_ the regression
test for this bug; no separate test is needed as long as the matrix continues to exercise all four
release targets (linux-gnu, linux-musl, aarch64-apple-darwin, x86_64-apple-darwin). Two practices
reinforce it:

- When bumping `sqlite-vec` (or any `cc`-compiled crate), add a CHANGELOG line of the form "verified
  compiles for x86_64-unknown-linux-musl" — this puts the musl regression risk in the reviewer's
  field of view rather than relying on CI to be the only check.
- Treat any musl-only CI failure as a release-blocker by default, never a flake. The class of
  failure is deterministic, not transient.

## Generalised takeaway: `.cargo/config.toml [env]` for per-target build flags

Whenever per-target build flags are required (CFLAGS, LDFLAGS, RUSTFLAGS, linker selection), prefer
`.cargo/config.toml` over inline workflow YAML, Makefile prefixes, or shell `ENV=value cmd`
invocations. The config-file form has three concrete advantages:

- It travels with the project — local dev, contributor machines, and CI all see the same flags
  without each surface having to remember to set them.
- It applies uniformly across `cargo build`, `cargo test`, `cargo run`, and any tool that shells out
  to cargo, with no per-invocation discipline required.
- It survives shell permission allowlists and sandboxing that may block `ENV=value cmd` syntax —
  including the agent's bash sandbox, which makes inline env prefixes unreliable for reproduction
  steps.

The rule of thumb: if a build flag needs to be set more than once or in more than one place, it
belongs in `.cargo/config.toml`, scoped to its target, with a comment naming the upstream issue and
the drop condition.

## Related

- Sibling sqlite-vec build issue:
  [`sqlite-vec-no-rust-export-register-via-ffi.md`](sqlite-vec-no-rust-export-register-via-ffi.md) —
  same crate, different layer (Rust FFI registration vs. C compilation). Reading both gives the full
  picture of integrating sqlite-vec into a Rust project.
- Adjacent CI/build-error pattern:
  [`rust-toolchain-action-does-not-read-toml.md`](rust-toolchain-action-does-not-read-toml.md) —
  same problem class (silent toolchain assumption that breaks one specific environment).
- Origin plan:
  [`docs/plans/2026-04-30-001-feat-release-process-plan.md`](../../plans/2026-04-30-001-feat-release-process-plan.md)
  — the release-pipeline work that surfaced this bug in PR #35.
- Maintainer runbook: [`docs/release-process.md`](../../release-process.md) — the broader
  cross-compile pipeline this fix unblocked.
- Upstream issue: <https://github.com/asg017/sqlite-vec/issues/156>
