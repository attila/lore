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
    cargo clippy --all-targets -- -D warnings

# Run tests
test:
    cargo test

# Run dependency audits
deny:
    cargo deny check

# Build documentation
doc:
    cargo doc --no-deps

# Install lore to ~/.cargo/bin
install:
    cargo install --path .

# Run the full CI pipeline
ci: fmt clippy test deny doc
