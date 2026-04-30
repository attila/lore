---
title: "feat: Release process — prebuilt binaries via cargo-zigbuild and GitHub Releases"
type: feat
status: active
date: 2026-04-30
deepened: 2026-04-30
---

# feat: Release process — prebuilt binaries via cargo-zigbuild and GitHub Releases

## Summary

Cut releases by pushing a `v*` tag. CI cross-compiles `lore` for four targets via `cargo-zigbuild`
from a single Linux runner, packages binaries into per-target tarballs with license + README,
generates a `SHA256SUMS` file, and publishes a GitHub Release with the Unreleased section of
`CHANGELOG.md` as the body. A pre-tag CI job smoke-builds the same four targets on every PR so
cross-compile breakage surfaces before tag time. Adds a `release-prep` just recipe that bumps the
Cargo version, rolls the CHANGELOG `[Unreleased]` heading to the new version, and reopens an empty
`[Unreleased]` block. README install and a `docs/release-process.md` runbook document the end-to-end
flow including checksum verification.

---

## Problem Frame

`lore` has no release artifact today. Users build from source via `just install`, which requires a
Rust toolchain and a network round-trip to crates.io. Even friendly users who agreed to try it have
hit toolchain installs as a friction point. PR #33 (universal patterns) and PR #34
(DB-as-sole-read-surface) shipped designs that already presume a release boundary exists —
schema-mismatch refusal-to-start, breaking-notice CHANGELOG entries, "after upgrading" advisories —
but there is currently no way to _be_ on a specific version. Building the release process now
(before there are real users) lets the workflow mature when mistakes are cheap; process maturity is
the primary deliverable, prebuilt binaries are the secondary deliverable.

---

## Requirements

- R1. Pushing a `v*` tag to GitHub triggers a release workflow that produces prebuilt binaries for
  four targets and publishes them to a GitHub Release.
- R2. Supported release targets: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`,
  `aarch64-apple-darwin`, `x86_64-apple-darwin`.
- R3. Each target binary ships as a `.tar.gz` archive containing the binary, both license files, and
  a copy of the README.
- R4. A single `SHA256SUMS` file is published alongside the tarballs containing SHA-256 hashes for
  every release artifact in the standard `sha256sum` two-space format so users can verify with
  `sha256sum -c SHA256SUMS`.
- R5. The release body is populated automatically from the `[Unreleased]` section of `CHANGELOG.md`
  at tag time. The author can edit it after the fact via the GitHub UI.
- R6. PR / push CI gains a cross-compile smoke job that builds all four targets with
  `cargo-zigbuild` (no upload, no release) so cross-compile regressions are caught before a tag is
  cut. The new job participates in the existing gateway-job allowlist pattern.
- R7. A `just release-prep VERSION` recipe bumps `Cargo.toml`, renames the CHANGELOG `[Unreleased]`
  heading to `[VERSION] - YYYY-MM-DD`, and inserts a fresh empty `[Unreleased]` block at the top of
  CHANGELOG. Idempotent failure modes — refuses to overwrite an existing `[VERSION]` heading or a
  non-empty `[Unreleased]`-conflict.
- R8. README "Install" section documents the prebuilt-binary path with copy-pasteable per-platform
  snippets (download → verify checksum → extract → place on PATH) alongside the existing
  build-from-source path.
- R9. A `docs/release-process.md` runbook describes: prerequisites, version bump procedure,
  CHANGELOG discipline, tag push, workflow monitoring, post-release verification, and rollback
  (yank) procedure.
- R10. The release workflow refuses to publish if the standard quality gates (`fmt`, `clippy`,
  `test`, `deny`, `doc`) fail for the tagged commit. Reuses the same `just ci` recipe that PR / push
  runs.
- R11. Release workflow is idempotent against accidental retag — re-pushing the same tag either
  fails fast or replaces artifacts cleanly without partial state.
- R12. The release workflow uses pinned action SHAs and reads the project Rust toolchain from
  `rust-toolchain.toml` via `actions-rust-lang/setup-rust-toolchain` (matching the existing CI
  convention).
- R13. First release cut from this work is `v0.1.0-alpha.1` to validate the pipeline against reality
  without committing the project to a stable-API milestone.
- R14. The release workflow ships behind a per-release owner-approval gate via a GitHub Environment
  named `release` (configured with the repository owner as required reviewer). The `publish` job
  pauses for explicit owner approval before any `gh release create` call runs. Push permission alone
  cannot ship a release — the gate is at the cloud-control-plane level, not in YAML. Survives future
  contributor onboarding without code changes.
- R15. Workflow least-privilege: workflow-level permissions default to `permissions: {}`; only the
  `publish` job grants `contents: write`. `verify` and `build` use the implicit `contents: read`
  they need for checkout, with `persist-credentials: false` on each checkout step.
- R16. Concurrency guard: workflow declares
  `concurrency: { group: release-${{ github.ref }}, cancel-in-progress: false }` so a
  double-tag-push does not race two publish runs.

---

## Scope Boundaries

- Homebrew tap, MacPorts, AUR, deb/rpm, Chocolatey, Scoop — not in scope. README install
  instructions point at GitHub Releases tarballs only.
- Code signing and notarization (Apple Developer ID, Microsoft Authenticode) — not in scope. macOS
  users will see the standard Gatekeeper "unidentified developer" warning on first run.
- Auto-update mechanism — not in scope. Users re-download.
- Windows targets (`x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`) — not in scope. The hook
  integration story is Unix-shaped today (XDG paths, `lore.toml` under `~/.config`, Claude Code on
  macOS/Linux primary). Windows can be added later as a discrete step.
- Cargo publish (`crates.io`) — not in scope. `lore` is a binary, not a library; crates.io install
  is a slow build-from-source path that GitHub Releases supersedes for end users.
- Plugin-marketplace listing (Claude Code marketplace) — not in scope; tracked separately on the
  roadmap.
- GPG/Sigstore signature over `SHA256SUMS` — not in scope. Defer until there is a credible threat
  model (real users, actual mirrors). Plain SHA-256 covers integrity.
- ARM64 Linux (`aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`) — not in scope. Add later
  if any user asks. zigbuild supports it; the only cost is workflow runtime.

### Deferred to Follow-Up Work

- Homebrew tap stub — separate PR once we have a stable v0.1.0 (not alpha).
- Plugin marketplace listing — separate roadmap item.
- Windows support — separate roadmap item.

---

## Context & Research

### Relevant Code and Patterns

- `.github/workflows/ci.yml` — existing CI workflow. Mirror its conventions: pinned SHAs via
  `taiki-e/install-action` for tools, `actions-rust-lang/setup-rust-toolchain@v1` for the Rust
  toolchain (reads `rust-toolchain.toml`), gateway `ci` job with `if: always()` and allowlist logic
  over `needs.*.result`.
- `justfile` — task-runner conventions. `just release-prep` belongs alongside the existing
  `just install`, `just changelog`, `just ci` recipes.
- `Cargo.toml` `[profile.release]` — already optimized: `lto = true`, `opt-level = "z"`,
  `strip = true`. No tuning needed.
- `rust-toolchain.toml` — pins channel to `stable` with `clippy` and `rustfmt`. Release workflow
  inherits this transparently via `actions-rust-lang/setup-rust-toolchain`.
- `cliff.toml` — `git-cliff` config. `just changelog` already regenerates CHANGELOG from
  conventional commits. The `release-prep` recipe edits the in-place `[Unreleased]` block; it does
  not invoke git-cliff (avoid round-tripping the body through git-cliff so that hand-curated
  breaking notices like the v2 schema bump are preserved).
- `CHANGELOG.md` — Keep a Changelog format already in use. `[Unreleased]` block is the authoring
  surface; `release-prep` rotates it.
- `deny.toml` — `cargo deny` config. The release workflow runs `just ci` which includes `just deny`,
  so license/advisory regressions block release.
- `agents/unattended-work.md` and `workflows/git-branch-pr.md` — universal patterns the agent
  receives at every tool call. Branch will be `feat/release-process`. Commits use restricted
  conventional vocabulary (`feat:`, `ci:`, `doc:`). PR body via `--body-file /tmp/...`.

### Institutional Learnings

- `rust/tooling.md` (universal): always run `just ci` before commit; `dprint` pinned to 0.53.1 in
  CI. Release workflow must pin the same version.
- `ci/github-actions-rust.md`: gateway job uses allowlist (`!= success` means fail) not denylist,
  because GitHub treats `skipped` jobs as neither failure nor success. Apply the same logic to the
  release workflow's final publish job.
- `rust/sqlite.md`: bundled SQLite via rusqlite is forbidden to swap to system. zigbuild must
  compile the `cc`-built C sources for SQLite and sqlite-vec on each target — verified feasible
  because zig provides clang under the hood. Zero dependencies on macOS frameworks in any of our
  crates means cross-compiling Apple targets from Linux is clean.
- `rust/error-handling.md`: no panics in library code. The `release-prep` recipe is a small shell
  flow inside `justfile`, not Rust code, but should still produce clear error output on bad input
  (`refuses if version invalid`, `refuses if [Unreleased] empty`).
- Phase 0.7 / 5.1.5 synthesis discipline: do not let the plan grow tangential CI cleanup. Bundling
  smoke-build into existing CI is in-scope; rewriting the existing CI workflow is not.

### External References

- `cargo-zigbuild` — wraps Cargo to use Zig as the linker, enabling cross-compilation (including
  Linux↔macOS) from a single Linux runner without per-target SDKs. Project documentation:
  <https://github.com/rust-cross/cargo-zigbuild>.
- Zig toolchain provides `clang` for C sources (rusqlite bundled, sqlite-vec). Verified by the
  `cargo-zigbuild` README — projects with `cc`-compiled C dependencies build cleanly as long as no
  Apple-framework linkage is required.
- `actions-rust-lang/setup-rust-toolchain` reads `rust-toolchain.toml` natively and includes cargo
  caching.
- `taiki-e/install-action` provides pinned binary installs for `just`, `dprint`, `cargo-deny`,
  `cargo-zigbuild`, `zig`. Same action family the existing CI already trusts.
- `softprops/action-gh-release` is a popular release-creator action; alternative is direct
  `gh release create` + `gh release upload` calls. Prefer `gh` CLI calls — fewer moving parts, no
  third-party action pin to maintain, matches the project's "thin tooling" bias.
- Standard `sha256sum` checksum format: `<hex-hash>  <relative-filename>` (two spaces). All modern
  verifiers (`sha256sum -c`, `shasum -a 256 -c`, Homebrew formula updaters) accept this format.

---

## Key Technical Decisions

- **Use `cargo-zigbuild` from a single `ubuntu-latest` runner for all four targets.** Avoids matrix
  sprawl across `macos-latest` runners, keeps build hermetic, and is the documented path for
  Linux↔macOS cross-compile. Cost: zigbuild + zig install on the runner (~30s cached). Alternative
  considered: per-OS matrix (`ubuntu-latest` for Linux gnu/musl, `macos-latest` for both Apple
  targets). Rejected — matrix complexity, slower cold starts on macOS runners, divergent toolchain
  provisioning per OS, and no benefit because we have no framework dependencies that would force
  native compilation.
- **Tarball not zip, even for cross-platform.** macOS and Linux both ship with `tar` and `gzip`; zip
  would imply Windows support that this plan does not deliver. Standard
  `lore-{version}-{target}.tar.gz` naming.
- **Single `SHA256SUMS` file, not per-tarball `.sha256` sidecars.** One-line
  `sha256sum -c
  SHA256SUMS` verifies all artifacts at once. Sidecars create five-file noise on the
  release page for no functional gain.
- **Release body sourced from CHANGELOG `[Unreleased]` block, not from git-cliff regeneration at tag
  time.** Hand-curated breaking notices and `--force` advisories must round-trip verbatim. The
  `release-prep` recipe rotates `[Unreleased]` → `[VERSION]` _before_ the tag is cut; the workflow
  extracts the new `[VERSION]` section and uses it as the release body.
- **Tag scheme: `v{semver}` (e.g., `v0.1.0-alpha.1`).** The `v` prefix is the Cargo / crates.io /
  GitHub convention and is what `git-cliff` already expects. Pre-1.0 alpha tags signal schema
  instability matching the current state of the project.
- **Workflow refuses to publish if `just ci` fails on the tagged commit.** Run quality gates in a
  `verify` job; the four `build-*` jobs `needs: [verify]`; the `publish` job
  `needs: [verify, build-*]`. Allowlist-style pass check before any release-create call.
- **Retag policy: workflow fails fast on existing release.** The `gh release create` call errors
  when a release for the tag already exists. Surface this as a clean failure rather than
  overwriting; recovery is to delete the broken release + tag and bump the version number for a
  fresh tag — a deliberate speed bump because retag-without-thinking is how broken artifacts ship.
  The runbook (U5) spells out the cleanup commands.
- **No GPG/Sigstore signing yet.** Adds release-time secret management (key custody, rotation, CI
  keyring config) for a project with no users. Revisit when there is a credible mirror /
  supply-chain story. SHA-256 + HTTPS-from-github.com covers integrity for the current threat model.
  Confirmed by security review: a MITM capable of substituting both tarball and SHA256SUMS has
  already broken TLS to GitHub, at which point signing buys little (they can swap the public key
  fetch too). The real threat GPG/Sigstore addresses is _GitHub itself or a compromised
  release-creator credential_ tampering with artifacts — explicitly out of scope.
- **Least-privilege workflow permissions.** Workflow-level `permissions: {}` (or `contents: read`);
  `contents: write` granted _only_ on the `publish` job. If a third-party action in `verify` or
  `build` is ever compromised (e.g. supply-chain compromise of `taiki-e/install-action`, a
  build-script credential exfiltration), the blast radius does not include release mutation.
- **Release ships behind a GitHub Environment gate.** The `publish` job declares
  `environment: release`, configured with the repository owner as a required reviewer. This is the
  canonical "only owner can ship" control: tag-push triggers the workflow, `verify` and `build` run
  unattended, but `publish` pauses for owner approval before any `gh release create` call. Survives
  future contributor onboarding without code changes — push permission alone cannot ship a release.
  Belt-and-suspenders: a `concurrency:` block on the workflow prevents double-tag-push races.
- **`release-prep` is a `just` recipe, not a separate Rust subcommand.** Release-mechanics scripting
  belongs in the task runner alongside `just changelog`, not in the binary's argument-parser
  surface. Keeps the binary small and the recipe inspectable.
- **CHANGELOG body is passed via `--notes-file`, never via `${{ }}` interpolation.** GitHub Actions
  expression interpolation happens _before_ shell parsing, so backticks or `$()` in CHANGELOG would
  execute if the body were inlined into a `run:` block. Reading the block to a temp file with `awk`
  and passing `--notes-file /tmp/release-body.md` is the safe pattern. Workflow comment forbids the
  unsafe alternative.

---

## Open Questions

### Resolved During Planning

- **Targets list — confirmed 4 (gnu, musl, both Apple).** ARM64 Linux deferred until requested.
  Windows out of scope.
- **Tooling — `cargo-zigbuild`.** No `cross` (Docker overhead, slower); no per-OS matrix.
- **Versioning — start at `v0.1.0-alpha.1`.** Validates pipeline against a low-stakes tag.
- **Checksums — `SHA256SUMS` single file.** Standard format.
- **Signing — none yet.** Defer.

### Deferred to Implementation

- **Exact `gh release create` flag set.** Likely
  `--draft=false --prerelease={true if -alpha
  in tag}`. Determine while writing the workflow.
- **Whether to compress the universal patterns shipped _inside_ the binary.** Out of scope — binary
  size is parked (see internal memory `project_binary_size_investigation.md`).
- **Whether the workflow should auto-bump `[Unreleased]` after publish.** Lean no — bumping is part
  of the _next_ release-prep, not this one. Avoids workflow needing write access to the repo's main
  branch.
- **macOS aarch64 build verification.** Cross from Linux _should_ work for our pure-C dependency
  set. If it fails at workflow time, fall back to a `macos-14` runner (M-series) for that one
  target. Captured as risk; not pre-emptively built.

---

## Output Structure

    .github/workflows/
      release.yml         (NEW)
      ci.yml              (MODIFIED — adds cross-compile smoke job)
    docs/
      release-process.md  (NEW)
    justfile              (MODIFIED — adds release-prep recipe)
    README.md             (MODIFIED — install section)
    CHANGELOG.md          (MODIFIED — adds release process entry under [Unreleased])
    Cargo.toml            (no change in this PR; bumped at release-prep time)

---

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

```
                              push tag v0.1.0-alpha.1
                                       │
                                       ▼
                             ┌──────────────────┐
                             │   release.yml    │
                             └────────┬─────────┘
                                      │
                       ┌──────────────┴──────────────┐
                       ▼                             ▼
              ┌──────────────────┐         (parallel build jobs)
              │  verify          │         ┌────────────────────────────┐
              │  (just ci)       │         │ build-x86_64-linux-gnu     │
              └────────┬─────────┘         │ build-x86_64-linux-musl    │
                       │                   │ build-aarch64-apple-darwin │
                       │                   │ build-x86_64-apple-darwin  │
                       │                   └────────────┬───────────────┘
                       │                                │
                       └────────────────┬───────────────┘
                                        ▼
                              ┌──────────────────┐
                              │     publish      │
                              │ (gh release      │
                              │  create + upload │
                              │  + SHA256SUMS)   │
                              └──────────────────┘

Each build-* job:
  1. checkout
  2. setup-rust-toolchain (reads rust-toolchain.toml)
  3. install zig + cargo-zigbuild via taiki-e/install-action
  4. cargo zigbuild --release --target $TARGET
  5. tar -czf lore-${VERSION}-${TARGET}.tar.gz \
       -C target/${TARGET}/release lore \
       -C ../../.. LICENSE-MIT LICENSE-APACHE README.md
  6. upload-artifact (intra-workflow handoff to publish job)

The publish job:
  1. download all artifacts
  2. compute SHA256SUMS over all .tar.gz files
  3. extract [VERSION] section from CHANGELOG.md → /tmp/release-body.md
  4. gh release create v${VERSION} \
       --title "v${VERSION}" \
       --notes-file /tmp/release-body.md \
       --prerelease=$( [[ $VERSION =~ -alpha|-beta|-rc ]] && echo true || echo false ) \
       *.tar.gz SHA256SUMS
```

---

## Implementation Units

- U1. **`release-prep` just recipe and version-bump tooling**

**Goal:** Add a `just release-prep VERSION` recipe that bumps `Cargo.toml` version, rotates the
CHANGELOG `[Unreleased]` block to `[VERSION] - YYYY-MM-DD`, and reopens an empty `[Unreleased]`
block. Idempotent failure on bad input.

**Requirements:** R7

**Dependencies:** None.

**Files:**

- Modify: `justfile`
- Test: `tests/release_prep.rs` (new — exercises the recipe via `assert_cmd` against a scratch
  worktree with synthetic Cargo.toml + CHANGELOG.md fixtures)

**Approach:**

- Recipe is a small shell-style block inside `justfile` that:
  - Validates the VERSION argument matches a permissive semver-ish regex
    (`^[0-9]+\.[0-9]+\.[0-9]+(-[a-z0-9.]+)?$`).
  - Updates `Cargo.toml` `version = "..."` line via `sed -i.bak` (then removes `.bak`).
  - Rewrites `CHANGELOG.md`: replaces the literal heading `## [Unreleased]` with
    `## [Unreleased]\n\n## [VERSION] - YYYY-MM-DD`. Refuses with a clear error if no
    `## [Unreleased]` heading is found, or if a `## [VERSION]` heading already exists.
  - Refuses if the `[Unreleased]` block is empty (no entries to release).
  - Runs `dprint fmt CHANGELOG.md Cargo.toml` to keep the formatter happy.
  - Prints next-step instructions: review the diff, commit with `chore(release): cut vX.Y.Z`, push
    the branch, merge, then push the tag from main.
- Treat shell as the implementation language because justfile recipes already shell out freely;
  introducing a Rust subcommand for release mechanics would couple the binary to release-time
  concerns. Use POSIX-portable constructs (no bashisms beyond what the existing `ci` recipe gateway
  uses).

**Patterns to follow:**

- Existing `just changelog` recipe (similar shape: orchestrates external tool + dprint pass).
- `rust/error-handling.md` discipline applied to shell exit codes — non-zero on bad input with
  descriptive `echo` messages to stderr.

**Test scenarios:**

- Happy path: VERSION=`0.1.0-alpha.1` against a Cargo.toml at version `0.1.0` and a CHANGELOG with a
  populated `[Unreleased]` block produces (a) Cargo.toml at `0.1.0-alpha.1`, (b) CHANGELOG with new
  empty `[Unreleased]` followed by `[0.1.0-alpha.1] - <today>` block containing the previous
  Unreleased contents.
- Edge case: VERSION already exists in CHANGELOG — recipe exits non-zero with a message naming the
  conflicting heading.
- Edge case: `[Unreleased]` block exists but is empty (only the heading) — recipe exits non-zero
  with a message saying nothing to release.
- Error path: invalid VERSION format (e.g., `1.0`, `v0.1.0`, `0.1.0_alpha`) — recipe exits non-zero
  with a message showing the expected format.
- Error path: no `## [Unreleased]` heading in CHANGELOG — recipe exits non-zero with a message
  naming the missing heading.
- Idempotency: running the recipe twice with the same VERSION fails the second run rather than
  corrupting state.

**Verification:**

- All test scenarios pass.
- Running `just release-prep 0.1.0-alpha.1` against a clean repo produces a single coherent diff
  (Cargo.toml version line + CHANGELOG block rotation), nothing else.

---

- U2. **Cross-compile smoke job in `ci.yml`**

**Goal:** Add a `cross-compile` job to the existing CI workflow that builds all four release targets
via `cargo-zigbuild` on every PR / push, without packaging or uploading. Catches cross-compile
breakage before tag time. Participates in the gateway `ci` allowlist.

**Requirements:** R6, R10, R12

**Dependencies:** None (independent of U1).

**Files:**

- Modify: `.github/workflows/ci.yml`

**Approach:**

- Add one new top-level job, `cross-compile`, that uses `runs-on: ubuntu-latest` and a
  `strategy.matrix.target` over the four release targets. Each matrix leg runs:
  - `actions/checkout` (pinned SHA)
  - `actions-rust-lang/setup-rust-toolchain@v1` with `rustflags: ""` (matches existing convention).
  - `taiki-e/install-action@v2` for `zig` and `cargo-zigbuild`.
  - `rustup target add ${{ matrix.target }}`
  - `cargo zigbuild --release --target ${{ matrix.target }} --features ""` (no `test-support`).
  - No artifact upload, no tarball, no upload-release-asset. Compile-only smoke.
- Extend the gateway `ci` job's `needs:` to include `cross-compile` and add its result to the
  allowlist `results` array. The gateway must remain `if: always()` and use the `!= success`
  allowlist pattern (per `ci/github-actions-rust.md`). Skipped matrix legs fail the gate.
- Pin all action SHAs to the same versions the existing `ci.yml` jobs already use.

**Patterns to follow:**

- Existing `ci.yml` job structure for fmt/clippy/test/deny/doc — pinned SHAs, identical
  `setup-rust-toolchain` invocation, identical `taiki-e/install-action` shape.
- Allowlist gateway pattern from `ci/github-actions-rust.md`.

**Test scenarios:**

- This is workflow-as-code — exercised by CI itself. The acceptance test is "the PR for this plan
  passes the new cross-compile job on all four targets" (covered in U6's verification).
- Test expectation for unit-test files: none — workflow code has no Rust unit-test surface; the
  workflow's first run on this PR is the integration test. Document this in the unit itself, not as
  missing coverage.

**Verification:**

- The new `cross-compile` job appears in the PR's CI checks with one matrix leg per target.
- All four legs go green.
- The gateway `ci` job lists `cross-compile` in its `needs` and includes its result in the allowlist
  check.
- A deliberately broken local `cargo zigbuild --target x86_64-unknown-linux-musl` reproduces the
  failure mode that would block release later.

---

- U3. **`release.yml` workflow on tag push**

**Goal:** New GitHub Actions workflow that triggers on `v*` tag push, runs the quality gates, builds
release artifacts for all four targets via `cargo-zigbuild`, computes `SHA256SUMS`, and publishes a
GitHub Release with the CHANGELOG `[VERSION]` block as the body.

**Requirements:** R1, R2, R3, R4, R5, R10, R11, R12

**Dependencies:** U1 (release-prep produces the CHANGELOG block this workflow extracts), U2 (smoke
job validates the cross-compile path the workflow depends on).

**Files:**

- Create: `.github/workflows/release.yml`

**Approach:**

- Trigger: `on: { push: { tags: ['v*'] } }`. No manual dispatch in v1; tagging is the authoritative
  action.
- **Workflow-level permissions: `permissions: {}`** (deny by default). Per-job permissions override
  only where needed — `verify` and `build` get nothing beyond the implicit `contents: read` they
  need for checkout; `publish` gets `contents: write` for `gh release create` + asset upload.
- **Concurrency guard**: workflow declares
  `concurrency: { group: release-${{ github.ref }}, cancel-in-progress: false }` so a
  double-tag-push (network retry, fat finger) does not race two publishes against each other.
  `cancel-in-progress: false` because in-flight publishes must complete cleanly, not be torn down
  mid-upload.
- **Owner-approval gate via GitHub Environment**: the `publish` job declares `environment: release`.
  The Environment is configured (one-time, by owner, in repo settings) with the repository owner as
  a required reviewer. This pauses execution of the publish job until the owner approves it in the
  GitHub UI. Push permission alone cannot ship a release — the gate is at the cloud-control-plane
  level, not in YAML.
- Three job tiers:
  1. **`verify`** — runs `just ci` against the tagged commit. If it fails, the workflow stops; no
     build, no publish. Checkout uses `persist-credentials: false` so git-push credentials don't
     live in `.git/config` for the rest of the job.
  2. **`build`** — matrix over the four release targets, `needs: [verify]`. Each leg:
     - Checkout (`persist-credentials: false`), setup-rust-toolchain (reads `rust-toolchain.toml`),
       install zig + cargo-zigbuild via `taiki-e/install-action` with both versions explicitly
       pinned.
     - `rustup target add ${{ matrix.target }}`.
     - `cargo zigbuild --release --target ${{ matrix.target }}`.
     - Package: `tar -czf lore-${VERSION}-${TARGET}.tar.gz` with the binary, both license files, and
       README.md inside. Use a stable internal layout (binary at archive root; license files at
       archive root).
     - `actions/upload-artifact` to hand the tarball off to the publish job.
  3. **`publish`** — `needs: [verify, build]`, `runs-on: ubuntu-latest`, `environment: release`,
     `permissions: { contents: write }`. Steps:
     - Checkout at default shallow depth (CHANGELOG.md is at HEAD of the tag — no `fetch-depth: 0`
       needed). `persist-credentials: false`.
     - `actions/download-artifact` pinned to a recent SHA, with `merge-multiple: true` and a fixed
       `path: dist/`. Avoid `*` globs that span the artifact root (zip-slip hardening).
     - Compute `SHA256SUMS`: `cd dist && sha256sum *.tar.gz > SHA256SUMS`.
     - Extract release body from CHANGELOG via `awk` to `/tmp/release-body.md`. Anchor the section
       regex on `^## \[` (literal bracket), not `^##`, so that `##` lines inside fenced code blocks
       in CHANGELOG entries do not truncate the body. The extracted file is treated as data: passed
       via `--notes-file /tmp/release-body.md` only. **Never** interpolated into a `run:` block via
       `${{ }}` — that would let `$(...)` or backticks in CHANGELOG content execute. A workflow
       comment forbids the unsafe alternative.
     - `gh release create ${{ github.ref_name }} --title ${{ github.ref_name }}
       --notes-file /tmp/release-body.md --prerelease=$(<conditional>)`.
       Conditional: prerelease=true when the tag matches `*-alpha*`, `*-beta*`, or `*-rc*`; false
       otherwise. Not draft — releases publish straight through after the Environment gate clears,
       so the U5 runbook should not mention an "untick draft" step.
     - `gh release upload ${{ github.ref_name }} dist/*.tar.gz dist/SHA256SUMS`. No `--clobber` —
       duplicate filenames fail fast (defense against partial-state overwrite on retag).
- VERSION variable: derived from `${{ github.ref_name }}` with the leading `v` stripped. Use a
  single `env:` block at workflow level so all jobs share it.
- Idempotency (R11): `gh release create` errors if the release already exists; allow the workflow to
  fail in that case rather than swallowing. `gh release upload` without `--clobber` errors on
  duplicate filenames; do not pass `--clobber`. Recovery requires human intervention (delete
  release + delete tag + bump version) — by design, documented in U5.
- Pinned SHAs for all third-party actions, matching `ci.yml` versions where actions overlap.
  `actions/download-artifact` and `actions/upload-artifact` pinned to the latest published SHA at
  plan-implementation time; documented as a `deps:` PR target whenever GitHub ships a security
  advisory for either.

**Patterns to follow:**

- `ci.yml` pinned-SHA discipline.
- Gateway allowlist semantics — `publish` job should explicitly check that `verify` and every
  `build` matrix leg produced `success` before invoking `gh release create`.
- `gh` CLI from the GitHub-hosted runner uses `GITHUB_TOKEN` automatically; no PAT configuration.

**Test scenarios:**

- Workflow code — same caveat as U2, no Rust unit-test surface. Acceptance is the end-to-end run in
  U6.
- Document the following expected behaviors as runbook checks (see U5):
  - Happy path: pushing `v0.1.0-alpha.1` produces a draft-published release with 4 tarballs
    - 1 SHA256SUMS file + the release body matching the CHANGELOG section.
  - Edge case: pushing `v0.1.0-alpha.1` a second time fails fast at `gh release create` with
    "release already exists".
  - Edge case: tag matching `vNON-SEMVER` (e.g., `vfoo`) — workflow runs but cargo-zigbuild has no
    version impact; the release body extractor will fail to find a matching CHANGELOG section,
    surfacing as a clean error.
  - Error path: `just ci` failure on the tagged commit — `verify` job fails, build and publish never
    run.
  - Integration: SHA256SUMS file content matches `sha256sum -c SHA256SUMS` against the downloaded
    tarballs.

**Verification:**

- The first `v0.1.0-alpha.1` tag push produces a populated GitHub Release with 4 tarballs and a
  SHA256SUMS file.
- Downloading any tarball, verifying its SHA, extracting it, and running `./lore --version` prints
  the expected version string.

---

- U4. **README install section update**

**Goal:** Replace the placeholder "Prebuilt binaries and package manager installs are planned" copy
with concrete per-platform install snippets that download from GitHub Releases, verify checksums,
extract, and place on PATH. Build-from-source path remains as the secondary option.

**Requirements:** R8

**Dependencies:** U3 (the workflow that produces the artifacts the README points at must exist; the
install snippet's URL shape is determined by U3).

**Files:**

- Modify: `README.md`

**Approach:**

- Restructure the existing `### Install` subsection under `## Quick Start` into:
  - Tabbed-style Markdown subheadings: `#### Linux (glibc)`, `#### Linux (Alpine / musl)`,
    `#### macOS (Apple Silicon)`, `#### macOS (Intel)`, `#### Build from source`.
  - Each prebuilt-binary subheading carries a copy-pasteable block: `curl -LO` the tarball from
    `https://github.com/.../releases/latest/download/lore-{target}.tar.gz`, `curl -LO` the
    SHA256SUMS, `sha256sum -c SHA256SUMS --ignore-missing` (with `shasum -a 256 -c` as the macOS
    variant), `tar xzf`, `mv lore /usr/local/bin/` (with a `~/.local/bin` note for no-sudo).
  - A short prose note pointing at the releases page for older versions.
  - **macOS Gatekeeper note**: a sentence explaining that browser-downloaded tarballs may carry the
    `com.apple.quarantine` attribute and require either right-click → Open or
    `xattr -d com.apple.quarantine ./lore` after extraction. `curl`-downloaded binaries do not get
    the attribute and run without intervention. No Apple Developer ID notarization is offered (Scope
    Boundaries) — this is the trade-off.
  - Build-from-source subheading retains the existing `just install` and `cargo build` instructions
    verbatim.
- Use `releases/latest/download/...` URL form so snippets do not need to be updated per release.
  GitHub redirects `latest/download/foo.tar.gz` to the latest release's `foo.tar.gz` asset.
  **Yank-resilience contract**: GitHub's `latest` pointer follows the most recent _non-draft
  non-prerelease_ release. If a release is yanked by being re-marked as draft or prerelease (per the
  U5 runbook), `latest` automatically falls back to the previous good release without requiring
  users to re-download the install snippets. This is why the runbook's yank procedure is
  non-destructive (demote to draft, do not delete the release outright).
- Run `dprint fmt README.md` after editing.

**Patterns to follow:**

- Existing README table style (consistent column widths, descriptive labels).
- `dprint` markdown formatting (textWrap: always, hard-wrap at 100 chars).

**Test scenarios:**

- Test expectation: none — pure documentation change. The functional test is "the snippets work
  end-to-end against a real release", verified manually as part of U6's checklist.

**Verification:**

- Snippets are syntactically valid shell.
- URLs follow the `releases/latest/download/...` shape.
- `dprint check` passes after edit.
- A reviewer can copy a snippet, paste it into a fresh shell on the matching platform, and end up
  with a working `lore` binary on PATH (covered in U6's manual checklist).

---

- U5. **Release process runbook (`docs/release-process.md`)**

**Goal:** Author the release runbook documenting end-to-end release mechanics so any future
maintainer (or returning author after a long gap) can cut a release without re-reading the workflow
YAML.

**Requirements:** R9

**Dependencies:** U1, U3 (runbook describes both the recipe and the workflow).

**Files:**

- Create: `docs/release-process.md`

**Approach:**

- Sections:
  1. **Overview** — who this is for, what the workflow does at a high level (one paragraph). Calls
     out the owner-approval gate (the GitHub Environment) so a future maintainer is not surprised
     when the publish job pauses.
  2. **Prerequisites** — write access to the repo, GitHub CLI (`gh`) authenticated, clean working
     tree, `just` installed, `dprint` installed. Plus the one-time setup of the `release`
     Environment in repo settings (required reviewers configured).
  3. **Versioning rules pre-1.0** — the version-decision table:
     - When to bump alpha → beta → rc → stable (alpha = unstable schema, breaking changes expected;
       beta = feature-complete, public testing; rc = no known blockers; stable = strip prerelease
       suffix).
     - Pre-1.0 minor vs. patch: minor for any new user-facing capability or breaking schema change;
       patch for fixes only. Major (1.0) gates on a stability commitment this project has not yet
       made.
     - Whether the _first_ release after PRs #33 and #34 (which already added v2 schema changes)
       should be `v0.1.0-alpha.1` (validates pipeline) or `v0.2.0-alpha.1` (signals the schema
       bumps). The plan picks alpha.1; runbook documents the reasoning so future judgment calls have
       a precedent.
  4. **Release cadence guidance** — for a solo maintainer, batch CHANGELOG entries to
     feature-completion boundaries rather than per-PR. Avoid a weekly cadence (creates release-noise
     pressure that contradicts the "process maturity over user demand" framing).
  5. **Cutting a release** — step-by-step:
     - Decide the version (per section 3).
     - Curate the CHANGELOG `[Unreleased]` block. Edit by hand to add breaking notices, upgrade
       instructions, etc. **Pointer (not restatement)**: see `https://keepachangelog.com/en/1.1.0/`
       and the breaking-notice format from PRs #33 and #34 in CHANGELOG.md history as the canonical
       examples.
     - **Patch-vs-minor exception**: for a hotfix patch (`vX.Y.Z+1`), `release-prep` rotates the
       _entire_ `[Unreleased]` block. If `[Unreleased]` contains entries unrelated to the hotfix,
       hand-edit the rotated CHANGELOG to move non-hotfix entries back into a fresh `[Unreleased]`
       block before committing. Concrete sed/diff guidance, not handwave.
     - Run `just release-prep VERSION` to rotate the CHANGELOG and bump Cargo.toml.
     - **Do not run `just changelog`** — it round-trips through git-cliff and would clobber
       hand-curated breaking notices. (Cross-link to Key Technical Decisions in this plan.)
     - Open a PR (`chore:` prefix per the conventional-commit vocabulary restriction; cross-link to
       `workflows/git-branch-pr.md`); confirm CI green.
     - Merge to main per merge-ownership convention — link to `workflows/git-branch-pr.md` rather
       than restate the rule.
     - From main: `git tag v$VERSION && git push origin v$VERSION`. (Tagging from anywhere other
       than main is forbidden; the runbook spells out why.)
     - Watch the release workflow at `.../actions/workflows/release.yml`.
     - **Approve the publish job** in the GitHub Actions UI when the Environment gate prompts for
       owner review.
  6. **Post-release verification** — download a tarball, verify checksum, extract, run
     `./lore --version`, do a quick `lore status` against a real knowledge base.
  7. **Hotfix path** — explicit choice: hotfix branches off the _tagged commit_ (not main HEAD), PR
     opens against main, merge to main, tag from main. No tagging directly from a hotfix branch —
     this preserves the invariant that every released SHA is reachable from main and avoids
     back-merge ambiguity. If the hotfix conflicts with main HEAD beyond a clean cherry-pick,
     escalate to a regular minor bump rather than forcing a hotfix.
  8. **Failure modes — decision trees** (replaces the v1 outline's flat list):
     - **`verify` fails on the tagged commit**: do not retag the same version. Open a fix PR, merge
       it, bump to next patch version (`vX.Y.Z+1`), repeat the cut. Spell out the cleanup commands:
       `git push origin :refs/tags/vX.Y.Z` (delete remote tag) + `git tag -d vX.Y.Z` (delete local
       tag).
     - **One build target fails (e.g. 3-of-4 green)**: do not retag. Two options, with the trade-off
       named:
       - (a) **Ship a 3-target release**: edit the workflow matrix to skip the broken target on a
         follow-up commit, document the gap in CHANGELOG (e.g. "macOS arm64 binary not available for
         vX.Y.Z, see vX.Y.Z+1"), bump to `vX.Y.Z+1` and re-cut. Affected users are gracefully
         steered to the next release.
       - (b) **Block the release until the target is fixed**: leave no published tag, delete the
         partial release + tag (commands below), fix the cross-compile in a PR, re-cut against the
         new patch version.
       - Decision rule: if the broken target is `aarch64-apple-darwin` and Apple Silicon Mac users
         are the realistic install path, prefer (b). Otherwise (a).
     - **`gh release create` fails because release exists**: a release with the tag already exists
       from a previous run (likely a partial-success retry). Delete the release via
       `gh release delete vX.Y.Z --cleanup-tag --yes` (which also deletes the tag), then bump to
       `vX.Y.Z+1` and re-cut. Never re-tag the same version.
     - **Workflow re-run vs. retag**: do _not_ use the GitHub UI's "re-run failed jobs" on
       `release.yml`. The first run already created (or partially created) state at github.com that
       the re-run will collide with. Always: delete release + tag, bump version, re-cut. The re-run
       button is safe for `ci.yml`; it is unsafe for `release.yml`.
     - **Tag pushed from a non-main commit**: the workflow will run against whatever SHA the tag
       points at. If the SHA is wrong, follow the same cleanup-and-bump procedure (delete release +
       tag, bump version, re-tag from main).
     - **Pruning failed-tag debris** — explicit commands:
       `gh release delete vX.Y.Z --cleanup-tag --yes` deletes both the release and the tag in one
       step (preferred); fallback `gh release delete vX.Y.Z --yes` +
       `git push origin :refs/tags/vX.Y.Z` + `git tag -d vX.Y.Z` if the `--cleanup-tag` flag is
       unavailable.
  9. **Yank / rollback procedure** — concrete mechanic:
     - Mark the bad release as **prerelease** via `gh release edit vX.Y.Z --prerelease`. This
       demotes it from `latest` without deleting the artifacts (preserves checksum audit trail).
       Editing to draft via `gh release edit vX.Y.Z --draft` is also an option but hides the
       artifacts entirely; prerelease is the lighter touch.
     - GitHub's `latest` pointer automatically falls back to the most recent non-draft
       non-prerelease release, so README install snippets continue to resolve to a working binary.
     - Append a `[Yanked]` notice to the bad release's body via
       `gh release edit vX.Y.Z --notes-file /tmp/yanked-notice.md`, explaining the defect and
       pointing at the replacement version.
     - Cut a replacement release at the next patch version with the fix included.
     - Update CHANGELOG retroactively only if the defect introduced a security or data safety risk
       (rare); otherwise the yank notice on the release is sufficient.
  10. **Prerelease promotion** — when an alpha/beta/rc validates and the next cut is stable (e.g.
      `v0.1.0` after `v0.1.0-rc.2`), do _not_ delete or demote the prior prerelease tags. Leave them
      in the release history as the public record of the stabilization arc. The `latest` pointer
      will jump to the new stable release automatically because prereleases are filtered.
  11. **Auditability** — release writes via `GITHUB_TOKEN` are logged in repository audit logs; a
      suspicious release can be traced to the workflow run that created it. No additional setup
      needed.
  12. **Why these choices** — short pointers back to this plan
      (`docs/plans/2026-04-30-001-feat-release-process-plan.md`) listing the specific decisions
      worth pointing at: single-runner zigbuild, no GPG signing, no git-cliff round-trip,
      retag-fails-fast, owner-approval Environment gate. Do not restate the rationale — link to the
      plan section.
- Keep the runbook concise — under ~350 lines (revised up from 250 to accommodate the decision-tree
  depth in section 8). Pointer-rich, not duplicative — every cross- reference uses a real file/URL
  link, never re-explains.
- Add `dprint fmt` after writing.
- Cross-link from `README.md` "Development" section as a one-line pointer.

**Patterns to follow:**

- Existing `docs/configuration.md` and `docs/hook-pipeline-reference.md` shape: clear section
  headings, code blocks for commands, prose between blocks for context.
- `Documentation Terminology Standards` (from local conventions): consistent use of "release",
  "tag", "artifact", "binary".

**Test scenarios:**

- Test expectation: none — pure documentation. Functional test is "a competent maintainer can follow
  it end-to-end without reading source", verified by self-walk during U6.

**Verification:**

- All commands in the runbook execute as written against a scratch repo.
- Cross-references resolve.
- `dprint check` passes.

---

- U6. **First release: prep, validate end-to-end, document outcome**

**Goal:** Cut `v0.1.0-alpha.1` to validate the full pipeline against reality. This is the
proof-of-life unit — without it, the previous five units are unverified theory. Per merge-ownership
convention, the _agent_ prepares everything up to the tag; the _owner_ pushes the tag.

**Requirements:** R13 (and acts as integration verification for R1–R5, R10, R11).

**Dependencies:** U1, U2, U3, U4, U5.

**Files:**

- Modify: `Cargo.toml` (version → `0.1.0-alpha.1`, via `just release-prep`)
- Modify: `CHANGELOG.md` (rotate `[Unreleased]` → `[0.1.0-alpha.1] - 2026-04-30`, via
  `just release-prep`; add a release-process entry to the rotated section noting this PR introduced
  prebuilt binaries)
- Modify: `ROADMAP.md` (move "Release process" from "Up Next" to "Completed")

**Approach:**

- **One-time owner setup before the first tag is pushed**: configure the `release` GitHub
  Environment in repo settings (Settings → Environments → New environment → name `release` →
  required reviewers: owner). This is a manual click-through; document it in U5 §2 as a
  prerequisite. The workflow will pause indefinitely on the first tag push if this is not done. The
  agent cannot configure Environments via gh CLI without elevated permissions the bot does not hold;
  surface this to the owner explicitly.
- After U1–U5 are merged on the same PR: run `just release-prep 0.1.0-alpha.1` locally on the
  feature branch.
- Manually inspect the diff: Cargo.toml version line; CHANGELOG block rotation; verify the
  universal-patterns and DB-as-sole-read-surface entries are now under `[0.1.0-alpha.1]`.
- Commit the prep changes on the same feature branch with `chore(release): cut v0.1.0-alpha.1`.
- Push the branch; CI runs the new `cross-compile` smoke job alongside existing gates.
- After PR merge to main, **stop**. The owner runs
  `git tag v0.1.0-alpha.1 && git push
  origin v0.1.0-alpha.1` from main. (The workflow itself is
  the integration test of the tag push; the agent does not push the tag.)
- Owner then verifies:
  - Workflow run completes green.
  - Release page populated with 4 tarballs + SHA256SUMS.
  - Download one tarball per OS available locally, run
    `sha256sum -c SHA256SUMS
    --ignore-missing`, extract, `./lore --version` reports
    `lore 0.1.0-alpha.1`.
- Document the verification outcome (links, command outputs) in the PR body or in a follow-up
  comment so it serves as the lived runbook for the next release.

**Execution note:** This unit's scope explicitly _stops at the PR merge_. The tag push is a release
action and falls under merge-ownership — only the owner cuts releases. Do not push the tag
autonomously.

**Patterns to follow:**

- Universal `workflows/git-branch-pr.md`: feature branch, draft PR, GPG-signed commits,
  `--body-file` for PR body.
- `just ci` before commit.

**Test scenarios:**

- Integration: `just release-prep 0.1.0-alpha.1` produces the expected diff against the current
  Cargo.toml + CHANGELOG (verified by inspection).
- Integration (post-tag, owner-driven): full workflow run produces a populated release.
- Integration (post-tag, owner-driven): downloaded tarball verifies against SHA256SUMS and binary is
  executable on the target platform.
- Edge case (post-tag, owner-driven): re-pushing the same tag (after
  `git push origin
  :refs/tags/v0.1.0-alpha.1` to delete remote) and re-pushing fresh: workflow
  re-runs but `gh release create` fails because the release still exists. Confirms R11.

**Verification:**

- PR merges with all CI green including the new cross-compile job.
- After owner pushes the tag: release page exists at
  `https://github.com/.../releases/tag/v0.1.0-alpha.1` with the expected artifacts.
- ROADMAP updated.

---

## System-Wide Impact

- **Interaction graph:** Three CI surfaces — existing `ci.yml` (gains a job), new `release.yml`
  (independent), and `justfile` (gains a recipe). Local-dev workflows are unaffected; `just ci`
  continues to behave as before.
- **Error propagation:** Quality-gate failures in `release.yml` `verify` job block publication
  cleanly. `release-prep` failures are local and produce no partial state (CHANGELOG and Cargo.toml
  are edited only on the success path; on early-exit the recipe exits before mutating files).
- **State lifecycle risks:** A partial release (some tarballs uploaded, others failed) is the main
  concern. Mitigation: artifacts are aggregated in the publish job _before_ the first
  `gh release create` call, so either all four tarballs are uploaded or none are. If the publish job
  fails after `gh release create` succeeds but before all uploads complete, the release exists but
  is incomplete; recovery is "delete the release manually, re-tag". Documented in U5 runbook.
- **API surface parity:** No runtime API changes. The binary is unchanged. Only build/release-time
  scaffolding.
- **Integration coverage:** End-to-end coverage for U2/U3 lives in CI itself. The first tag push is
  the integration test for the full release pipeline (U6).
- **Unchanged invariants:** Bundled SQLite via rusqlite (forbidden to swap to system) is preserved
  by zigbuild compiling the C sources for each target. The `[profile.release]` optimization settings
  are unchanged. Existing CI gates (`fmt`, `clippy`, `test`, `deny`, `doc`) run identically; the new
  `cross-compile` job is additive. The hook integration contract (DB as sole read surface, universal
  patterns injection) is irrelevant to the release workflow — the workflow does not interact with
  the runtime DB or any user data.

---

## Risks & Dependencies

| Risk                                                                                                                                                                             | Mitigation                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `aarch64-apple-darwin` cross-compile from Linux fails for our `cc`-compiled C sources (rusqlite bundled, sqlite-vec).                                                            | Feasibility-reviewed: the dependency surface (vanilla C99 SQLite amalgamation + sqlite-vec, no Apple-framework linkage) is exactly the case zigbuild's bundled libc + macOS SDK stubs handle cleanly. Verified in U2 smoke job before any tag is cut. If it nonetheless fails at workflow time, per-target `runs-on` override to `macos-14` (M-series) for that one target — matrix structure makes this a one-line edit. Documented in U5. |
| Less-trodden release path — most public Rust CLIs use `macos-latest` runners for Apple targets, not zigbuild from Linux. Support questions hit a smaller community.              | Acceptable. The pattern works (zig itself is mature, cargo-zigbuild is maintained, individual targets have production users). Trade-off accepted in exchange for hermetic single-runner builds. Documented for transparency.                                                                                                                                                                                                                |
| zig version drift (a zig upgrade breaks our build).                                                                                                                              | Pin `zig` and `cargo-zigbuild` versions in the `taiki-e/install-action` invocations. Treat upgrades as deliberate `deps:` PRs, not floating.                                                                                                                                                                                                                                                                                                |
| `gh release create` partial-state on workflow failure mid-publish.                                                                                                               | Aggregate all artifacts before the first publish call; document manual recovery in U5 (delete release + tag, bump version, re-cut — never retag the same version). Accept that fully-atomic release publication is not achievable through `gh` and the manual recovery path is good enough for a project at this scale.                                                                                                                     |
| `release-prep` recipe corrupts CHANGELOG on edge-case input.                                                                                                                     | Test scenarios in U1 cover invalid version, missing `[Unreleased]`, conflicting `[VERSION]`, and empty `[Unreleased]`. Recipe refuses with clear error before any file mutation.                                                                                                                                                                                                                                                            |
| Tag pushed from a non-main commit.                                                                                                                                               | Workflow runs against whatever the tag points to. Owner-approval Environment gate (U3) lets the owner abort before publish if the SHA looks wrong. U5 runbook explicitly says "tag from main only" and documents the cleanup-and-bump recovery procedure.                                                                                                                                                                                   |
| **Contributor with `write` access pushes a `v*` tag and forces a release** without owner authorization.                                                                          | GitHub Environment named `release` with the owner as required reviewer (U3). The publish job pauses for explicit owner approval in the GitHub Actions UI before any `gh release create` call. Push permission alone cannot ship a release. Survives future contributor onboarding without code changes.                                                                                                                                     |
| **Workflow-level `contents: write` over-privileges `verify` and `build` jobs**, exposing release mutation if any third-party action in those jobs is compromised (supply-chain). | Workflow defaults to `permissions: {}`; only the `publish` job grants `contents: write`. `verify` and `build` use the implicit `contents: read` they need for checkout. Per-job least-privilege follows GitHub's hardening guidance.                                                                                                                                                                                                        |
| **CHANGELOG body containing `$()`/backticks executes if interpolated into a `run:` block via `${{ }}`.**                                                                         | U3 mandates `--notes-file /tmp/release-body.md` (file-based handoff, never expression interpolation). Workflow comment forbids the unsafe alternative. The body extractor anchors on `^## \[` (literal bracket) so stray `##` inside fenced code blocks does not truncate.                                                                                                                                                                  |
| **Double-tag-push race** (network retry, fat finger) launches two concurrent publish runs.                                                                                       | Workflow declares `concurrency: { group: release-${{ github.ref }}, cancel-in-progress: false }`. The second run queues until the first completes; the queued run's `gh release create` then fails fast on the existing release, surfacing the duplicate cleanly.                                                                                                                                                                           |
| **Release-asset zip-slip via `actions/download-artifact`** (older versions had a CVE).                                                                                           | Pin to a recent SHA; use `merge-multiple: true` with a fixed `path: dist/` (no `*` globs that span artifact root).                                                                                                                                                                                                                                                                                                                          |
| Future need for code signing forces a workflow rewrite.                                                                                                                          | Acceptable. Signing is a known-deferred concern (Scope Boundaries). When it lands, the publish job grows a sign step before upload — additive change, not a rewrite.                                                                                                                                                                                                                                                                        |
| The release-page URL shape (`releases/latest/download/...`) changes upstream.                                                                                                    | Stable GitHub feature; risk essentially zero. The `latest` pointer's "skip prereleases and drafts" semantics is also stable and is what makes the U5 yank procedure non-destructive (see U4 yank-resilience contract).                                                                                                                                                                                                                      |
| **README install snippets break for users when a release is yanked**, because `releases/latest/download/...` would resolve to the yanked artifact.                               | Yank procedure (U5 §9) demotes the bad release to _prerelease_ (or draft), not delete. GitHub's `latest` pointer skips prereleases automatically and falls back to the previous good release. Snippets continue to resolve to a working binary.                                                                                                                                                                                             |
| Owner pushes the tag against a stale CHANGELOG (forgot to run `release-prep`).                                                                                                   | The release body extractor in U3 fails to find a matching `[VERSION]` section in CHANGELOG and the publish job exits non-zero (before any release is created). Recover by running `release-prep` on a follow-up PR, merging, then bumping to the next patch and re-cutting.                                                                                                                                                                 |
| Maintainer accidentally runs `just changelog` (git-cliff regeneration) and clobbers hand-curated breaking notices.                                                               | Documented prominently in U5 §5 ("Do not run `just changelog`") with cross-link to the Key Technical Decisions section explaining why. Consider in a follow-up: a guard comment in the `just changelog` recipe itself.                                                                                                                                                                                                                      |

---

## Documentation / Operational Notes

- README install section refresh (U4) is user-facing.
- `docs/release-process.md` runbook (U5) is maintainer-facing.
- ROADMAP "Up Next" entry "Release process (prebuilt binaries via `cargo-zigbuild`, GitHub
  releases)" moves to "Completed" as part of U6.
- No monitoring / alerting changes; release workflow failures are surfaced via GitHub's standard
  Actions notification path.
- Operational runbook in U5 is the single source of truth for "how do I cut a release".

---

## Sources & References

- ROADMAP entry: "Release process (prebuilt binaries via `cargo-zigbuild`, GitHub releases)" —
  `ROADMAP.md`
- Existing CI: `.github/workflows/ci.yml`
- Tooling conventions: `rust/tooling.md` (universal pattern, injected at every tool call)
- CI conventions: `ci/github-actions-rust.md` (universal pattern)
- Workflow conventions: `workflows/git-branch-pr.md` (universal pattern)
- SQLite invariant: `rust/sqlite.md` (bundled SQLite forbidden to swap)
- Internal memory: `project_binary_size_investigation.md` (binary size parked; release profile
  already optimized)
- `cargo-zigbuild` docs: <https://github.com/rust-cross/cargo-zigbuild>
- `actions-rust-lang/setup-rust-toolchain`: reads `rust-toolchain.toml`
- Keep a Changelog 1.1.0: <https://keepachangelog.com/en/1.1.0/>
