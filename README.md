# Lore

Your engineering wisdom, always in context.

Lore is a local semantic search MCP server for software patterns. It indexes your codebase's
architectural decisions, patterns, and conventions, then serves them as context to AI coding agents
via the Model Context Protocol.

> **Status:** Early development. The project skeleton and quality gates are in place but no features
> are implemented yet.

## Prerequisites

- [Rust](https://rustup.rs/) 1.85+ (pinned via `rust-toolchain.toml`)
- [just](https://github.com/casey/just) — task runner
- [dprint](https://dprint.dev/install/) — formatter
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) — dependency auditor

## Getting Started

```sh
git clone git@github.com:attila/lore.git
cd lore
just setup    # configure git hooks
just ci       # run the full quality gate pipeline
```

`just ci` runs formatting, clippy, tests, dependency audits, and doc checks in sequence. All five
must pass.

## Available Commands

| Command        | What it does                               |
| -------------- | ------------------------------------------ |
| `just setup`   | Configure git hooks (run once after clone) |
| `just fmt`     | Check formatting                           |
| `just fmt-fix` | Fix formatting                             |
| `just clippy`  | Run clippy lints                           |
| `just test`    | Run tests                                  |
| `just deny`    | Run dependency audits                      |
| `just doc`     | Build documentation                        |
| `just ci`      | Run the full pipeline                      |

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).
