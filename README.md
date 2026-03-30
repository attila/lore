# Lore

Your engineering wisdom, always in context.

Lore is a local semantic search engine for your software patterns and conventions, exposed as an MCP
tool for Claude Code (and any MCP-compatible client). Your knowledge lives as markdown files in a
git repo. Lore indexes them with hybrid full-text and vector search, then serves results over MCP so
your AI coding agent consults your patterns before writing code.

Single Rust binary. No external database. Only runtime dependency is [Ollama](https://ollama.com)
for embeddings.

## How It Works

```
Markdown files (git repo, source of truth)
        │
        ▼  ingest
┌──────────────┐     ┌──────────┐
│    lore       │────▶│  Ollama  │  (embed chunks)
│  (Rust binary)│◀────│  :11434  │
└──────┬───────┘     └──────────┘
       │
       ▼
┌──────────────┐
│  SQLite      │  FTS5 (lexical) + sqlite-vec (vector)
│  (single     │  both compiled into the binary
│   .db file)  │
└──────┬───────┘
       │
       ▼  MCP over stdio
┌──────────────┐
│  Claude Code │
└──────────────┘
```

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.85+ (pinned via `rust-toolchain.toml`)
- [Ollama](https://ollama.com) — `brew install ollama` or see install options

### Install

```sh
just install
```

This runs `cargo install --path .`, placing the `lore` binary in `~/.cargo/bin/` (which rustup adds
to PATH during Rust installation). To build without installing:

```sh
cargo build --release
# binary at ./target/release/lore
```

### Initialize and Use

```sh
# Point lore at a directory of markdown files (must be a git repo)
lore init --repo ~/my-patterns

# Test a search
lore search "error handling"

# Check health
lore status
```

The `init` command verifies Ollama is running, pulls the embedding model (`nomic-embed-text`,
~270MB), creates `lore.toml` and `lore.db`, and runs the first ingestion.

### Use with Claude Code

```sh
claude mcp add --scope user --transport stdio lore -- \
  lore serve --config /path/to/lore.toml
```

Then add to your `CLAUDE.md`:

```markdown
Before implementing any new module, function, or test, use the search_patterns tool to check for
established patterns. Do not skip this step.
```

## Commands

| Command                   | Purpose                                                   |
| ------------------------- | --------------------------------------------------------- |
| `lore init --repo <path>` | First-time setup: provision Ollama, create config, ingest |
| `lore ingest`             | Re-index the knowledge base after editing markdown files  |
| `lore serve`              | Start the MCP server (stdio transport for Claude Code)    |
| `lore search <query>`     | Search from the command line                              |
| `lore status`             | Check health of all components                            |

## MCP Tools

The server exposes four tools:

| Tool                | Purpose                                                 |
| ------------------- | ------------------------------------------------------- |
| `search_patterns`   | Semantic + keyword search across all patterns           |
| `add_pattern`       | Create a new pattern file, index it, commit to git      |
| `update_pattern`    | Replace an existing pattern's content, re-index, commit |
| `append_to_pattern` | Add a section to an existing pattern, re-index, commit  |

## Knowledge Base Format

Your knowledge base is a directory of markdown files in a git repo. Any structure works:

```
my-patterns/
├── error-handling.md
├── testing/
│   ├── unit-tests.md
│   └── integration-tests.md
├── api-design.md
└── code-style.md
```

Files are chunked by heading — each `## Section` becomes a separate searchable unit. YAML
frontmatter tags are extracted and searchable:

```markdown
---
tags: [error-handling, rust, result-types]
---

# Error Handling with Result Types

Always use Result<T, E> for fallible operations...
```

## Search

- **Hybrid** (default): Combines FTS5 lexical search and sqlite-vec vector similarity using
  Reciprocal Rank Fusion. Finds semantically related patterns even when terminology differs.
- **FTS-only**: Set `hybrid = false` in `lore.toml` to skip Ollama at query time.

## Development

### Prerequisites

- [just](https://github.com/casey/just) — task runner
- [dprint](https://dprint.dev/install/) — formatter
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) — dependency auditor

### Commands

```sh
just setup    # configure git hooks (run once after clone)
just ci       # run the full quality gate pipeline
```

| Command        | What it does                               |
| -------------- | ------------------------------------------ |
| `just setup`   | Configure git hooks (run once after clone) |
| `just fmt`     | Check formatting                           |
| `just fmt-fix` | Fix formatting                             |
| `just clippy`  | Run clippy lints                           |
| `just test`    | Run tests (88 tests, no Ollama needed)     |
| `just deny`    | Run dependency audits                      |
| `just doc`     | Build documentation                        |
| `just ci`      | Run the full pipeline                      |

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).
