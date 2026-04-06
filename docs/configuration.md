# Configuration Reference

Lore is configured through a TOML file, environment variables, and CLI flags. This document is the
complete reference for all available options.

For guidance on tuning search and injection behaviour, see the
[Hook Pipeline and Plugin Reference](hook-pipeline-reference.md).

## Configuration File

The configuration file is `lore.toml`, created by `lore init`. Its default location follows the XDG
Base Directory Specification (see [File Paths](#file-paths) below).

### Complete Example

```toml
knowledge_dir = "/path/to/your/patterns"
database = "/path/to/lore/knowledge.db"
bind = "localhost:3100"

[ollama]
host = "http://127.0.0.1:11434"
model = "nomic-embed-text"

[search]
hybrid = true
top_k = 5
min_relevance = 0.6

[chunking]
strategy = "heading"
max_tokens = 1024

[git]
inbox_branch_prefix = "inbox/"
```

### Field Reference

#### Top-Level Fields

| Field           | Type   | Required | Default            | Description                                                                                                                 |
| --------------- | ------ | -------- | ------------------ | --------------------------------------------------------------------------------------------------------------------------- |
| `knowledge_dir` | path   | Yes      | Set by `lore init` | Directory containing your markdown pattern files. A git repository is recommended; see [Git Integration](#git-integration). |
| `database`      | path   | Yes      | Set by `lore init` | Path to the SQLite database file. This is a derived artefact — safe to delete and rebuild with `lore ingest --force`.       |
| `bind`          | string | Yes      | `"localhost:3100"` | Bind address for future TCP transport (not yet implemented; MCP currently uses stdio).                                      |

Only files under `knowledge_dir` with a `.md` or `.markdown` extension are ingested. Other files
(`.txt`, `.mdx`, `.rst`, etc.) are silently skipped — they will not appear in search results. This
filter applies to both full and delta (git-based) ingest; for delta ingest, renaming a file across
the extension boundary (e.g. `.md` → `.txt`) is treated as a deletion.

#### `[ollama]` Section

| Field   | Type   | Default                    | Description                                                                                                                                                     |
| ------- | ------ | -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `host`  | string | `"http://127.0.0.1:11434"` | Ollama API endpoint. Change if Ollama runs on a different port or host.                                                                                         |
| `model` | string | `"nomic-embed-text"`       | Embedding model name. Must be available in Ollama. Other options include `mxbai-embed-large` (1024 dimensions) and `snowflake-arctic-embed2` (1024 dimensions). |

#### `[search]` Section

| Field           | Type  | Default | Description                                                                                                                                 |
| --------------- | ----- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `hybrid`        | bool  | `true`  | Enable hybrid search (FTS5 + vector via Ollama). Set to `false` for FTS5-only search, which is faster but lacks semantic matching.          |
| `top_k`         | usize | `5`     | Number of top results to consider before sibling expansion and deduplication. Higher values inject more context per tool call.              |
| `min_relevance` | float | `0.6`   | Minimum normalised score for a result to be injected. Applied only during hybrid search with successful embedding. Set to `0.0` to disable. |

#### `[chunking]` Section

| Field        | Type   | Default     | Description                                                                                                                   |
| ------------ | ------ | ----------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `strategy`   | string | `"heading"` | Chunking strategy. Currently only `"heading"` is implemented, which splits markdown files on headings (`#` through `######`). |
| `max_tokens` | usize  | `1024`      | Maximum tokens per chunk. Reserved for future token-based limiting (not yet implemented).                                     |

#### `[git]` Section (Optional)

This section is optional. When present, it enables the inbox branch workflow for pattern submissions
via MCP tools.

| Field                 | Type   | Default | Description                                                                                                                                                                      |
| --------------------- | ------ | ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `inbox_branch_prefix` | string | —       | Branch name prefix for per-submission branches created by `add_pattern`, `update_pattern`, and `append_to_pattern`. Each submission creates a branch like `inbox/pattern-title`. |

When the `[git]` section is absent, MCP write operations commit directly to the current branch — or
skip the commit entirely if the knowledge base is not a git repository. See
[Git Integration](#git-integration) below for the full picture.

## Git Integration

Lore works with or without git, but several features depend on the knowledge base being a git
repository. A git repository is strongly recommended, and is the assumed default throughout this
documentation.

### What works without git

- `lore init` — initialises the configuration and runs a full ingest against a plain directory
- `lore ingest` — runs a full re-index on every invocation and prints
  `Not a git repository — running full ingest`
- `lore search` — unaffected; searches the local database
- MCP write tools (`add_pattern`, `update_pattern`, `append_to_pattern`) — files are written to disk
  and indexed in SQLite, but are not committed. The returned `WriteResult` reflects this.

### What degrades or breaks without git

- **Delta ingest is unavailable.** Every `lore ingest` re-reads and re-embeds every markdown file in
  the knowledge base. On a large corpus this is slower and multiplies the number of Ollama embedding
  calls. Delta ingest uses `git diff` against the last successfully ingested commit SHA — without
  git, there is no baseline to diff against. `lore ingest --force` is the equivalent rebuild path
  inside a git repository when the database needs to be regenerated from scratch.
- **Inbox branch workflow breaks.** Setting `[git] inbox_branch_prefix` in `lore.toml` will cause
  `add_pattern`, `update_pattern`, and `append_to_pattern` to fail, because these commands call
  `git` unconditionally to create and push per-submission branches. Omit the `[git]` section
  entirely when the knowledge base is not a git repository.
- **No version history.** Without commits there is no `git log`, no `git blame`, no way to roll back
  a bad edit, and no way to review a diff of what changed. Patterns exist only as the current file
  contents on disk.

### Recommended setup

Initialise the knowledge base directory as a git repository before running `lore init`:

```sh
cd ~/my-patterns
git init
lore init --repo ~/my-patterns
```

This enables delta ingest from the first run and preserves a full history of every pattern change. A
remote is not required — lore's ingest and search features work entirely against the local
repository. Add a remote later if you want the inbox branch workflow or off-machine backup.

## Environment Variables

| Variable          | Purpose                                   | Values                          | Notes                                                                                                                          |
| ----------------- | ----------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `LORE_DEBUG`      | Enable verbose debug logging              | `1`, `true`, or `yes` to enable | Output writes to stderr with `[lore debug]` prefix. The value is read once on first check and cached for the process lifetime. |
| `XDG_CONFIG_HOME` | Override the configuration base directory | Any absolute path               | Defaults to `$HOME/.config` when unset or empty.                                                                               |
| `XDG_DATA_HOME`   | Override the data base directory          | Any absolute path               | Defaults to `$HOME/.local/share` when unset or empty.                                                                          |
| `HOME`            | Home directory (fallback for XDG)         | Set by the operating system     | Required when XDG variables are not set. If absent, lore reports an error suggesting `--config`.                               |

## File Paths

Lore follows the
[XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) for
default file locations.

### Resolution Order

For the configuration file:

1. If `--config <path>` is passed on the command line, use that path
2. If `$XDG_CONFIG_HOME` is set and non-empty, use `$XDG_CONFIG_HOME/lore/lore.toml`
3. Otherwise, use `$HOME/.config/lore/lore.toml`

For the database file:

1. The path in `lore.toml` (`database` field) is authoritative
2. `lore init` sets this to `$XDG_DATA_HOME/lore/knowledge.db` or
   `$HOME/.local/share/lore/knowledge.db` by default

### Default Paths

| File          | Default path                       |
| ------------- | ---------------------------------- |
| Configuration | `~/.config/lore/lore.toml`         |
| Database      | `~/.local/share/lore/knowledge.db` |

The `~` notation here represents `$HOME`. These are not valid TOML values — use absolute paths in
`lore.toml`.

If `$HOME` is not set and no XDG variable is provided, lore exits with an error message:

> Cannot determine config directory: $HOME is not set. Use --config to specify a path.

## CLI Flags

### Global Flags

These flags apply to all commands:

| Flag              | Description                                                                                                      |
| ----------------- | ---------------------------------------------------------------------------------------------------------------- |
| `--config <path>` | Path to `lore.toml`. Overrides XDG resolution.                                                                   |
| `--json`          | Output structured JSON to stdout. Suppresses all stderr diagnostics. Available for `search` and `list` commands. |

### Command-Specific Flags

| Command       | Flag                | Description                                                                                                                             |
| ------------- | ------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `lore init`   | `--repo <path>`     | Path to the knowledge base directory. A git repository is recommended; see [Git Integration](#git-integration).                         |
| `lore init`   | `--model <name>`    | Embedding model name (default: `nomic-embed-text`).                                                                                     |
| `lore init`   | `--bind <addr>`     | Bind address (default: `localhost:3100`).                                                                                               |
| `lore init`   | `--database <path>` | Database file path (overrides the XDG default).                                                                                         |
| `lore ingest` | `--force`           | Force full re-ingest: drops and recreates the FTS5 table, re-embeds all files. Required after schema changes such as tokeniser updates. |
| `lore search` | `<query>`           | Search query (positional argument).                                                                                                     |
| `lore search` | `--top-k <n>`       | Number of results to return (overrides configuration).                                                                                  |

## MCP Tool Input Limits

The MCP server enforces maximum sizes on tool arguments to prevent resource exhaustion. Oversized
inputs are rejected with a JSON-RPC `-32000` error before any processing occurs.

| Field               | Maximum size           |
| ------------------- | ---------------------- |
| `query`             | 1,024 bytes            |
| `title`             | 512 bytes              |
| `source_file`       | 512 bytes              |
| `heading`           | 512 bytes              |
| `body`              | 262,144 bytes (256 KB) |
| `tags` (serialised) | 8,192 bytes (8 KB)     |
| `top_k`             | 100                    |
