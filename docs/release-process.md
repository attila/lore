# Release Process

How to cut a `lore` release. This runbook covers prerequisites, versioning, the cut procedure,
post-release verification, hotfixes, failure recovery, and yank/rollback.

## Overview

Releases are triggered by pushing a `v*` tag from `main`. The
[`release.yml`](../.github/workflows/release.yml) workflow runs the project quality gates
(`just ci`), cross-compiles four binary targets via `cargo-zigbuild`, computes a `SHA256SUMS` file,
and publishes a GitHub Release with the matching CHANGELOG section as the body. The publish step is
gated by a `release` GitHub Environment that requires owner approval — push permission alone cannot
ship a release.

## Prerequisites

One-time setup, performed by the repository owner:

1. **GitHub Environment**: Settings → Environments → New environment → name it `release` → add the
   repository owner as a required reviewer. The first tag push will pause the workflow indefinitely
   until this is configured.
2. **Local tooling**: `just`, `dprint`, `git-cliff`, and the GitHub CLI (`gh`) authenticated against
   the repo (`gh auth login`).
3. **Clean working tree** before starting any release procedure.

## Versioning rules pre-1.0

While the project is pre-1.0, semver constraints are intentionally loose.

| Version shape        | When to use                                                                   |
| -------------------- | ----------------------------------------------------------------------------- |
| `vX.Y.Z-alpha.N`     | Schema unstable, breaking changes expected. Default for early releases.       |
| `vX.Y.Z-beta.N`      | Feature-complete for the cycle, public testing welcome, no API guarantees.    |
| `vX.Y.Z-rc.N`        | No known blockers, last shake-out before stable.                              |
| `vX.Y.Z` (no suffix) | Stable release. Pre-1.0 still allows breaking changes between minor versions. |

**Bumping the minor (`v0.X.0`) vs the patch (`v0.0.Z`) pre-1.0:**

- **Minor** for any new user-facing capability or breaking schema change.
- **Patch** for fixes only.
- **Major** (`v1.0.0`) gates on a public stability commitment the project has not yet made.

**The first release**: cut as `v0.1.0-alpha.1` even though PRs #33 and #34 already added v2 schema
changes. Validating the pipeline against a low-stakes alpha tag is more valuable than matching the
version number to schema cardinality. The schema bumps still appear under `[0.1.0-alpha.1]` in the
CHANGELOG with their breaking notices intact.

## Cadence guidance

Solo-maintainer cadence — batch CHANGELOG entries to feature-completion boundaries, not per-PR.
Avoid a fixed weekly cadence; release-noise pressure cuts against the "process maturity over user
demand" framing this project was built on. Cut a release when there is something worth releasing.

## Cutting a release

1. **Decide the version** per the table above.

2. **Curate `CHANGELOG.md`**. The `[Unreleased]` block accumulates entries from merged PRs.
   Hand-edit it before cutting:

   - Confirm the format matches existing entries (Keep a Changelog 1.1.0 — see
     <https://keepachangelog.com/en/1.1.0/>).
   - Add or refine breaking notices, upgrade instructions, and `--force` advisories. The v2 schema
     entries from PRs #33 and #34 in `CHANGELOG.md` are the canonical examples.
   - **Do not run `just changelog`** — that recipe regenerates CHANGELOG from git-cliff and would
     clobber hand-curated breaking notices. The deliberate decision is documented in the
     release-process plan
     ([`docs/plans/2026-04-30-001-feat-release-process-plan.md`](plans/2026-04-30-001-feat-release-process-plan.md)).

3. **Patch-vs-minor exception**: `release-prep` rotates the _entire_ `[Unreleased]` block. For a
   hotfix patch where `[Unreleased]` contains entries unrelated to the hotfix, after running
   `release-prep` hand-edit the rotated CHANGELOG to move non-hotfix entries back into a fresh
   `[Unreleased]` block before committing. Concretely:

   ```sh
   just release-prep 0.1.1
   # CHANGELOG now has: [Unreleased] (empty) + [0.1.1] - <today> with everything inside.
   $EDITOR CHANGELOG.md
   # Move non-hotfix bullets from [0.1.1] back up to [Unreleased].
   ```

4. **Run `release-prep`**:

   ```sh
   just release-prep 0.1.0-alpha.1
   ```

   This bumps `Cargo.toml`, rotates the CHANGELOG, runs `dprint fmt`, and prints next-step
   instructions. Refuses on bad input (invalid semver, missing `[Unreleased]`, conflicting
   `[VERSION]`, or empty `[Unreleased]` block) before mutating any file.

5. **Open a release-prep PR**. Branch prefix `feat/` is wrong for a release commit; use the `chore:`
   conventional-commit type. The project's branch-naming and conventional-commit conventions live in
   the `workflows/git-branch-pr.md` universal pattern (injected into every Claude Code session via
   lore's hook):

   ```sh
   git checkout -b chore/release-v0.1.0-alpha.1   # branch prefix per convention
   git commit -am 'chore(release): cut v0.1.0-alpha.1'
   git push --set-upstream origin HEAD
   gh pr create --draft --title 'chore(release): cut v0.1.0-alpha.1' --body-file /tmp/pr-body.md
   ```

   Wait for CI green. Owner merges per the merge-ownership convention (also in
   `workflows/git-branch-pr.md`): only the repository owner merges PRs and cuts releases.

6. **Tag from `main` only**:

   ```sh
   git checkout main
   git pull origin main
   git tag v0.1.0-alpha.1
   git push origin v0.1.0-alpha.1
   ```

   Tagging from any other branch is forbidden — see Failure Modes §1 for why.

7. **Approve the publish job**. The workflow runs `verify` and `build` automatically. When the
   matrix completes, `publish` pauses for owner approval at the `release` Environment gate. Open the
   workflow run in the GitHub Actions UI and click "Review deployments → Approve".

## Post-release verification

After the workflow completes, verify the release end-to-end on at least one platform:

```sh
# Linux glibc example
curl -LO https://github.com/attila/lore/releases/latest/download/lore-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/attila/lore/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
tar xzf lore-x86_64-unknown-linux-gnu.tar.gz
./lore --version
./lore status   # against an existing knowledge base
```

Expected: SHA256 verification passes, binary executes, version string matches the tag.

## Hotfix path

Hotfixes follow the same merge-then-tag flow as regular releases, with one constraint: the hotfix
branches off the _tagged commit_ (not main HEAD), then merges to main, then is tagged from main.

```sh
git checkout v0.1.0
git checkout -b fix/critical-thing
# ... fix and commit ...
git push --set-upstream origin HEAD
gh pr create --draft --title 'fix: critical thing' --body-file /tmp/pr-body.md
# Owner reviews, merges to main.
git checkout main && git pull
just release-prep 0.1.1
# (move non-hotfix entries back to [Unreleased] per §3 above)
git commit -am 'chore(release): cut v0.1.1'
# ... PR + merge + tag from main as usual ...
```

Never tag directly from a hotfix branch — every released SHA must be reachable from `main`. If the
hotfix conflicts with main HEAD beyond a clean cherry-pick, escalate to a regular minor bump rather
than forcing a hotfix.

## Prerelease promotion

When stabilising an alpha/beta/rc into a stable release (e.g. `v0.1.0` after `v0.1.0-rc.2`):

- Do **not** delete or demote the prior prerelease tags. They are the public record of the
  stabilisation arc.
- The `latest` pointer automatically jumps to the new stable release because GitHub filters
  prereleases out of `latest`.
- README install snippets continue to resolve correctly.

## Failure modes

### 1. Tag pushed from a non-main commit

The workflow runs against the SHA the tag points at, not `main`. If the SHA is wrong, the release
will be cut from the wrong tree. Recovery follows the same procedure as §4 below.

### 2. `verify` fails on the tagged commit

`just ci` failed against the tagged commit. Do **not** retag the same version. Open a fix PR against
`main`, merge it, bump the patch (`vX.Y.Z+1`), and re-cut. Cleanup commands:

```sh
git push origin :refs/tags/v0.1.0-alpha.1   # delete remote tag
git tag -d v0.1.0-alpha.1                    # delete local tag
```

### 3. One build target fails (e.g. 3 of 4 green)

Two recovery options, with the trade-off named:

- **Ship a 3-target release**: edit the workflow `matrix.target` list on a follow-up commit to skip
  the broken target, document the gap in CHANGELOG (e.g. "macOS arm64 binary not available for
  v0.1.0, see v0.1.1"), bump to `vX.Y.Z+1` and re-cut. Affected users are gracefully steered to the
  next release.
- **Block the release until fixed**: delete the partial release + tag (commands below), fix the
  cross-compile in a PR, re-cut against the new patch version.

**Decision rule**: if the broken target is `aarch64-apple-darwin` (Apple Silicon Mac users are a
realistic install path), prefer blocking. Otherwise the 3-target ship is acceptable.

### 4. `gh release create` fails because release exists

A release with the tag already exists from a previous run (likely a partial-success retry). Delete
the release and tag in one step, bump version, re-cut:

```sh
gh release delete v0.1.0-alpha.1 --cleanup-tag --yes
# Then bump to v0.1.0-alpha.2 and re-cut.
```

If `--cleanup-tag` is unavailable on your gh version, fall back to:

```sh
gh release delete v0.1.0-alpha.1 --yes
git push origin :refs/tags/v0.1.0-alpha.1
git tag -d v0.1.0-alpha.1
```

**Never re-tag the same version.** The retag-fails-fast policy exists precisely because
retag-without-thinking is how broken artifacts ship.

### 5. Workflow re-run vs. retag

Do **not** use the GitHub UI's "re-run failed jobs" button on `release.yml`. The first run already
created (or partially created) state at github.com that the re-run will collide with. Always: delete
release + tag, bump version, re-cut. The re-run button is safe for `ci.yml`; it is **unsafe** for
`release.yml`.

## Yank / rollback

A release is "yanked" when a defect is discovered after publication. The mechanic is non-destructive
— artifacts stay on the release page (preserving checksum audit trail) but GitHub's `latest` pointer
skips them.

```sh
# 1. Demote the bad release from `latest`. Prerelease is the lighter touch than draft —
#    artifacts remain visible, but `latest` skips it.
gh release edit v0.1.0 --prerelease

# 2. Append a yank notice to the bad release body, pointing at the replacement.
$EDITOR /tmp/yanked-notice.md   # explain the defect; link to the fix release
gh release edit v0.1.0 --notes-file /tmp/yanked-notice.md

# 3. Cut a replacement release at the next patch version with the fix included.
#    (Follow the standard cut procedure above.)
```

After yanking, `releases/latest/download/...` URLs in README install snippets resolve to the
previous good release automatically. No README update required for a yank — only for the new fix
release, which doesn't change the snippet shape because it uses `releases/latest/`.

Update `CHANGELOG.md` retroactively only if the defect introduced a security or data safety risk
(rare). Otherwise the yank notice on the release page is sufficient.

## Auditability

Release writes via `GITHUB_TOKEN` are logged in repository audit logs. A suspicious release can be
traced to the workflow run that created it by cross-referencing the run ID with the release creation
timestamp.

## Why these choices

The deliberate design decisions behind this process — single-runner zigbuild, no GPG signing, no
git-cliff round-trip, retag-fails-fast, owner-approval Environment gate — are documented in
[`docs/plans/2026-04-30-001-feat-release-process-plan.md`](plans/2026-04-30-001-feat-release-process-plan.md).
Read that plan if you need to understand _why_ before changing the workflow.
