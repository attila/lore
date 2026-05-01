#!/usr/bin/env bash
# release-prep — bump Cargo.toml version and rotate CHANGELOG [Unreleased] block
#
# Invoked via `just release-prep VERSION`. Refuses to mutate any file when input
# is invalid or pre-conditions fail. See docs/release-process.md for the
# end-to-end release procedure.

set -euo pipefail

VERSION="${1:-}"

if [ -z "$VERSION" ]; then
    echo "Error: VERSION argument is required." >&2
    echo "Usage: just release-prep <X.Y.Z[-suffix]>" >&2
    exit 1
fi

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-z0-9.]+)?$ ]]; then
    echo "Error: VERSION '$VERSION' is not a valid semver." >&2
    echo "Expected: X.Y.Z or X.Y.Z-suffix (e.g. 0.1.0, 0.1.0-alpha.1)." >&2
    exit 1
fi

if [ ! -f Cargo.toml ]; then
    echo "Error: Cargo.toml not found in current directory." >&2
    exit 1
fi

if [ ! -f CHANGELOG.md ]; then
    echo "Error: CHANGELOG.md not found in current directory." >&2
    exit 1
fi

if ! grep -q '^version = ' Cargo.toml; then
    echo "Error: Cargo.toml has no top-level 'version = ...' line." >&2
    exit 1
fi

if ! grep -q '^## \[Unreleased\]' CHANGELOG.md; then
    echo "Error: CHANGELOG.md has no '## [Unreleased]' heading to rotate." >&2
    exit 1
fi

if grep -q "^## \[$VERSION\]" CHANGELOG.md; then
    echo "Error: CHANGELOG.md already has a '## [$VERSION]' heading." >&2
    echo "Refusing to overwrite an existing release section." >&2
    exit 1
fi

# Verify the [Unreleased] block has at least one non-blank line before the next
# '## [' heading. An empty block means there is nothing to release.
if ! awk '
    /^## \[Unreleased\]/ { in_block = 1; next }
    /^## \[/             { in_block = 0 }
    in_block && /[^[:space:]]/ { found = 1 }
    END                  { exit !found }
' CHANGELOG.md; then
    echo "Error: '## [Unreleased]' block has no entries — nothing to release." >&2
    exit 1
fi

TODAY="$(date -u +%Y-%m-%d)"

# Bump Cargo.toml version line. Use sed -i.bak for portability between GNU and
# BSD sed; remove the backup file immediately after.
sed -i.bak "s/^version = .*/version = \"$VERSION\"/" Cargo.toml
rm -f Cargo.toml.bak

# Rotate CHANGELOG: insert a new dated VERSION heading after [Unreleased] and
# leave [Unreleased] empty for the next cycle. awk avoids cross-platform sed
# differences with multi-line replacements.
awk -v ver="$VERSION" -v today="$TODAY" '
    /^## \[Unreleased\]/ {
        print
        print ""
        print "## [" ver "] - " today
        next
    }
    { print }
' CHANGELOG.md > CHANGELOG.md.new
mv CHANGELOG.md.new CHANGELOG.md

if command -v dprint >/dev/null 2>&1 && [ -f dprint.json ]; then
    dprint fmt CHANGELOG.md Cargo.toml
fi

# Run the full quality gate against the post-mutation tree. Without this, a
# version bump or CHANGELOG rotation that breaks a snapshot test (or anything
# else that depends on Cargo.toml content) only surfaces in CI after push —
# wasting a feedback cycle. set -e propagates the failure; the maintainer fixes
# the issue against the bumped tree and either re-runs the script or commits.
# Skipped in test environments without a justfile.
if command -v just >/dev/null 2>&1 && [ -f justfile ]; then
    just ci
fi

cat <<EOF
release-prep complete for v$VERSION (just ci passed).

Next steps:
  1. Review the diff: git diff
  2. Commit:          git commit -am 'chore(release): cut v$VERSION'
  3. Push branch and open a PR; wait for CI green and owner merge.
  4. After merge, from main:
       git tag v$VERSION
       git push origin v$VERSION
  5. Approve the publish job in the GitHub Actions UI when the
     'release' Environment gate prompts for owner review.

See docs/release-process.md for the full procedure.
EOF
