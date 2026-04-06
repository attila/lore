# Lore

Your engineering wisdom, always in context.

Lore is a local semantic search engine for your software patterns and conventions, exposed as an MCP
tool for Claude Code. Your knowledge lives as markdown files in a git repository. Lore indexes them
with hybrid full-text and vector search, then serves results over MCP so your AI coding agent
consults your patterns before writing code.

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

- [Rust](https://rustup.rs/) (latest stable, pinned via `rust-toolchain.toml`)
- [just](https://github.com/casey/just) — task runner (`cargo install just`)
- [Ollama](https://ollama.com) — `brew install ollama` or see install options

### Install

Prebuilt binaries and package manager installs are planned. For now, build from source:

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
# Point lore at a directory of markdown files (git repository recommended)
lore init --repo ~/my-patterns

# Test a search
lore search "error handling"

# Check health
lore status
```

The `init` command verifies Ollama is running, pulls the embedding model (`nomic-embed-text`,
~270MB), creates `lore.toml` and the knowledge database, and runs the first ingestion.

### Use with Claude Code

Install the lore plugin to get the MCP server, lifecycle hooks, and the `/search-lore` skill:

```sh
claude --plugin-dir /path/to/lore/integrations/claude-code/
```

The plugin assumes `lore` is on PATH and uses the default config (`~/.config/lore/lore.toml`). If
you use a custom config path, either edit `integrations/claude-code/mcp.json` to add your `--config`
flag, or add the MCP server manually:

```sh
claude mcp add --scope user --transport stdio lore -- \
  lore serve --config /path/to/lore.toml
```

The manual approach gives only the MCP server. The plugin also includes hooks that inject relevant
patterns before edits and a `/search-lore` skill for on-demand queries.

## Commands

| Command                   | Purpose                                                   |
| ------------------------- | --------------------------------------------------------- |
| `lore init --repo <path>` | First-time setup: provision Ollama, create config, ingest |
| `lore ingest`             | Re-index the knowledge base after editing markdown files  |
| `lore serve`              | Start the MCP server (stdio transport for Claude Code)    |
| `lore search <query>`     | Search from the command line                              |
| `lore status`             | Check health of all components                            |

## MCP Tools

The server exposes five tools:

| Tool                | Purpose                                                                         |
| ------------------- | ------------------------------------------------------------------------------- |
| `search_patterns`   | Semantic + keyword search across all patterns                                   |
| `add_pattern`       | Create a new pattern file, index it, and commit if the base is a git repository |
| `update_pattern`    | Replace an existing pattern's content, re-index, and commit if git is in use    |
| `append_to_pattern` | Add a section to an existing pattern, re-index, and commit if git is in use     |
| `lore_status`       | Report knowledge base health: git status, indexed counts, last commit           |

## Knowledge Base Format

Your knowledge base is a directory of markdown files. Any structure works:

```
my-patterns/
├── error-handling.md
├── testing/
│   ├── unit-tests.md
│   └── integration-tests.md
├── api-design.md
└── code-style.md
```

Only files with a `.md` or `.markdown` extension are ingested. Other files (`.txt`, `.mdx`, `.rst`,
etc.) are silently skipped — they will not appear in search results.

Git is recommended but not required. Lore works against a plain directory, but delta ingest, the
inbox branch workflow, and version history are all unavailable without a git repository. See
[Configuration Reference → Git Integration](docs/configuration.md#git-integration) for the full
picture.

Files are chunked by heading — each `## Section` becomes a separate searchable unit. YAML
frontmatter tags are extracted and searchable.

To exclude non-pattern files such as `README.md`, `CONTRIBUTING.md`, or a `drafts/` directory from
indexing, place a `.loreignore` file at the repository root. The syntax matches `.gitignore` and
supports negation patterns. See the [Configuration Reference](docs/configuration.md#loreignore) for
details.

```markdown
---
tags: [error-handling, rust, result-types]
---

# Error Handling with Result Types

Always use Result<T, E> for fallible operations...
```

## Search

- **Hybrid** (default): Combines FTS5 lexical search and sqlite-vec vector similarity using
  Reciprocal Rank Fusion. Title and tag matches are weighted above body text, so domain-scoped
  queries return the right patterns first.
- **FTS-only**: Set `hybrid = false` in `lore.toml` to skip Ollama at query time.

## Documentation

| Guide                                                                 | Description                                                  |
| --------------------------------------------------------------------- | ------------------------------------------------------------ |
| [Pattern Authoring Guide](docs/pattern-authoring-guide.md)            | How to write patterns that agents actually follow            |
| [Search Mechanics Reference](docs/search-mechanics.md)                | Full search pipeline internals for debugging discoverability |
| [Hook Pipeline and Plugin Reference](docs/hook-pipeline-reference.md) | Hook lifecycle, plugin setup, and injection tuning           |
| [Configuration Reference](docs/configuration.md)                      | `lore.toml` options, environment variables, CLI flags        |

## Development

### Prerequisites

- [just](https://github.com/casey/just) — task runner
- [dprint](https://dprint.dev/install/) — formatter
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) — dependency auditor
- [git-cliff](https://git-cliff.org) — changelog generator

### Commands

```sh
just setup    # configure git hooks (run once after clone)
just ci       # run the full quality gate pipeline
```

| Command          | What it does                               |
| ---------------- | ------------------------------------------ |
| `just setup`     | Configure git hooks (run once after clone) |
| `just fmt`       | Check formatting                           |
| `just fmt-fix`   | Fix formatting                             |
| `just clippy`    | Run clippy lints                           |
| `just test`      | Run tests (no Ollama needed)               |
| `just deny`      | Run dependency audits                      |
| `just doc`       | Build documentation                        |
| `just changelog` | Regenerate CHANGELOG.md from git history   |
| `just ci`        | Run the full pipeline                      |

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).
