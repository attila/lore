#!/usr/bin/env bash
# bump-tap-formula — point the Homebrew formula at the release just published
#
# Invoked by the publish job in .github/workflows/release.yml, never by hand. The
# archive names carry the version, so every download URL changes in two places,
# the tag directory and the archive name, and every checksum beside it changes
# too. Commits through the GitHub API so the commit carries GitHub's signature:
# the tap's default branch requires signed commits, and a commit pushed over git
# would be unsigned and rejected. See docs/release-process.md.
#
# Reads VERSION (the tag, e.g. v0.5.1), GH_TOKEN (a token for the tap), and a
# checksums file naming every archive in the release.

set -euo pipefail

VERSION="${VERSION:-}"
SUMS="${SUMS:-dist/SHA256SUMS}"
TAP_REPO="${TAP_REPO:-attila/homebrew-tap}"
FORMULA="${FORMULA:-Formula/lore.rb}"
# Empty means the tap's default branch. Set only to rehearse against a scratch
# branch, which a release never does.
BRANCH="${BRANCH:-}"

if [ -z "$VERSION" ]; then
    echo "Error: VERSION is empty; expected a tag such as v0.5.1." >&2
    exit 2
fi

if [ ! -s "$SUMS" ]; then
    echo "Error: no checksums at ${SUMS}." >&2
    exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

ref=""
if [ -n "$BRANCH" ]; then
    ref="?ref=${BRANCH}"
fi

gh api "repos/${TAP_REPO}/contents/${FORMULA}${ref}" --jq '.content' | base64 -d > "$work/before.rb"
blob="$(gh api "repos/${TAP_REPO}/contents/${FORMULA}${ref}" --jq '.sha')"

# One pass: a url line yields the old tag and archive name, takes the new tag
# and the archive name carrying the new version, and the sha256 line that
# follows takes that archive's checksum. Pairing by position rather than by
# pattern means an arm cannot take another arm's checksum. The version inside
# the archive name is replaced as a literal string, never as a regex, because
# its dots would otherwise match any character.
awk -v version="$VERSION" '
    NR == FNR { sum[$2] = $1; next }

    /url "https:\/\/github\.com\// {
        if (match($0, /\/releases\/download\/[^\/]+\/[^"]+"/) == 0) {
            print "Error: unrecognised download URL: " $0 > "/dev/stderr"
            exit 1
        }
        head = substr($0, 1, RSTART - 1)
        tail = substr($0, RSTART + RLENGTH)
        path = substr($0, RSTART, RLENGTH - 1)
        sub(/^\/releases\/download\//, "", path)
        old_tag = substr(path, 1, index(path, "/") - 1)
        name = substr(path, index(path, "/") + 1)

        old_mark = "-" substr(old_tag, 2) "-"
        at = index(name, old_mark)
        if (substr(old_tag, 1, 1) != "v" || at == 0) {
            print "Error: archive " name " does not carry the version of tag " old_tag > "/dev/stderr"
            exit 1
        }
        name = substr(name, 1, at - 1) "-" substr(version, 2) "-" substr(name, at + length(old_mark))

        if (!(name in sum)) {
            print "Error: no checksum published for " name > "/dev/stderr"
            exit 1
        }
        pending = name
        print head "/releases/download/" version "/" name "\"" tail
        next
    }

    /^[[:space:]]*sha256 "/ {
        if (pending == "") {
            print "Error: a sha256 line with no download URL before it: " $0 > "/dev/stderr"
            exit 1
        }
        sub(/"[0-9a-f]*"/, "\"" sum[pending] "\"")
        pending = ""
        print
        next
    }

    { print }
' "$SUMS" "$work/before.rb" > "$work/after.rb"

if cmp -s "$work/before.rb" "$work/after.rb"; then
    echo "The formula already points at ${VERSION}; nothing to bump."
    exit 0
fi

# Belt and braces: every arm must now carry this tag, this version in its
# archive name, and a checksum from this release. An arm left behind is
# internally consistent and installs the old version silently, so it is checked
# rather than trusted. `grep -F` because the version's dots are literal.
all_urls="$(grep -c "releases/download/" "$work/after.rb" || true)"
fresh_urls="$(grep -cF "releases/download/${VERSION}/lore-${VERSION#v}-" "$work/after.rb" || true)"
if [ "$all_urls" != "$fresh_urls" ]; then
    echo "Error: only ${fresh_urls} of ${all_urls} download URLs carry ${VERSION}." >&2
    exit 1
fi

while read -r line; do
    digest="${line#*\"}"
    digest="${digest%\"*}"
    if ! grep -q "^${digest}[[:space:]]" "$SUMS"; then
        echo "Error: checksum ${digest} is not in ${SUMS}." >&2
        exit 1
    fi
done < <(grep '^[[:space:]]*sha256 "' "$work/after.rb")

put=(-f "message=chore: lore ${VERSION}" -f "content=$(base64 < "$work/after.rb" | tr -d '\n')" -f "sha=$blob")
if [ -n "$BRANCH" ]; then
    put+=(-f "branch=$BRANCH")
fi

gh api -X PUT "repos/${TAP_REPO}/contents/${FORMULA}" "${put[@]}" \
    --jq '"Bumped the formula to '"$VERSION"' in \(.commit.sha)"'
