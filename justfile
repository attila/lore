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
deny:
    cargo deny check

# Build documentation
doc:
    cargo doc --no-deps

# Install lore to ~/.cargo/bin
install:
    cargo install --path .

# Regenerate CHANGELOG.md from git history
changelog:
    git cliff -o CHANGELOG.md
    dprint fmt CHANGELOG.md

# Run the full CI pipeline
ci: fmt clippy test deny doc
