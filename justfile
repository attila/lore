# List available recipes
default:
    @just --list

# Configure git hooks (run once after clone)
setup:
    git config core.hooksPath .githooks

# Check formatting
fmt:
    dprint check

# Fix formatting
fmt-fix:
    dprint fmt

# Run clippy lints
clippy:
    cargo clippy --all-targets --features test-support -- -D warnings

# Run tests
test:
    cargo test --features test-support

# Run dependency audits
#
# `GIT_CONFIG_*` overrides neutralise URL rewrites before gix (used by
# cargo-deny) fetches the RustSec advisory DB. Without them, a `[url] insteadOf`
# rule in `~/.gitconfig` or sandbox-injected `GIT_CONFIG_KEY_*` rewrites can
# turn the HTTPS clone into an SSH one and fail in environments where port 22
# is blocked.
deny:
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_COUNT=0 cargo deny check

# Build documentation
doc:
    cargo doc --no-deps

# Install lore to ~/.cargo/bin
install:
    cargo install --path .

# Regenerate CHANGELOG.md from git history.
#
# Do NOT run this as part of release-prep — git-cliff regeneration clobbers
# hand-curated breaking notices. Use `just release-prep VERSION` instead, which
# rotates the existing CHANGELOG block in place. See docs/release-process.md.
changelog:
    git cliff -o CHANGELOG.md
    dprint fmt CHANGELOG.md

# Bump Cargo.toml version and rotate CHANGELOG [Unreleased] for a release.
# See docs/release-process.md for the full procedure.
release-prep VERSION:
    bash scripts/release-prep.sh {{ VERSION }}

# Run integration tests that require Ollama
test-integration:
    cargo test --features test-support -- --ignored

# Run the full CI pipeline
ci: fmt clippy test deny doc
