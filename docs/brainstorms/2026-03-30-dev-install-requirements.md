---
date: 2026-03-30
topic: dev-install
---

# Development Install

## Problem Frame

After building lore, the binary lives at `./target/release/lore`. The README Quick Start and MCP
config examples reference this path, which is fragile and unfriendly. Developers need a single
command to put `lore` on PATH so it works from anywhere and the MCP config doesn't need absolute
paths to the build directory.

This is the first half of the install story. Release distribution (prebuilt binaries, Homebrew tap,
cross-compilation) is deferred until after MVP.

## Requirements

**Install command**

- R1. Add a `just install` recipe that runs `cargo install --path .`
- R2. The recipe should build in release mode (cargo install does this by default)

**README update**

- R3. Update Quick Start to show `just install` then use bare `lore` commands instead of
  `./target/release/lore`. Note that `~/.cargo/bin/` must be on PATH (rustup sets this up by
  default)
- R4. Update the MCP config example to use `lore` as the command (assumes PATH)
- R5. Keep `cargo build --release` in the README as context for contributors who want to build
  without installing

## Success Criteria

- `just install` puts a working `lore` binary on PATH via `~/.cargo/bin/`
- README examples use `lore` without path prefixes
- Recipe is visible in `just --list`

## Scope Boundaries

- No release process, prebuilt binaries, or Homebrew tap (deferred to post-MVP)
- No changes to the binary itself or its behavior
- No `just uninstall` recipe (cargo uninstall lore is obvious enough)

## Next Steps

-> /ce:plan for structured implementation planning
