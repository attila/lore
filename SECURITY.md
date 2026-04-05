# Security

## Threat Model

Lore is a **local single-user CLI tool** and MCP server. It runs under the invoking user's
permissions, communicates over stdio (not network sockets), and connects only to a localhost Ollama
instance for embeddings. There is no authentication, no remote API surface, and no multi-user
access.

The primary threats are:

- **Resource exhaustion** from unbounded input (oversized MCP payloads, large transcript files)
- **Unintended file access** from unvalidated paths in hook input
- **FTS5 query injection** from unsanitised special characters
- **Path traversal** in pattern write operations

## Trust Boundaries

| Input Surface              | Trust Level                            | Validation                                                               |
| -------------------------- | -------------------------------------- | ------------------------------------------------------------------------ |
| CLI arguments              | Trusted (user-invoked)                 | Clap argument parsing                                                    |
| MCP tool arguments (stdio) | Partially trusted                      | Input length limits per field, path validation via `validate_within_dir` |
| Hook input (agent payload) | Partially trusted                      | Session ID hashed for filenames, transcript path validated under `$HOME` |
| Transcript file content    | Partially trusted                      | Bounded tail-read (last 32KB), lossy UTF-8 conversion                    |
| Markdown knowledge files   | Trusted (user-controlled, git-tracked) | Extension filter (`.md`/`.markdown` only)                                |
| Ollama API responses       | Trusted (localhost)                    | Error handling on malformed responses                                    |
| Config file (`lore.toml`)  | Trusted (user-authored)                | TOML parsing via `serde`                                                 |

## Security Measures

### Input Validation

- **MCP tool arguments**: All string inputs are length-limited (`query` ≤ 1KB, `body` ≤ 256KB,
  `title`/`heading`/`source_file` ≤ 512 bytes). `top_k` capped at 100. `tags` array capped at 8KB
  serialised. Oversized inputs return a JSON-RPC error without processing.

- **FTS5 query sanitisation**: All user input is sanitised before FTS5 MATCH queries via
  `sanitize_fts_query()`. Operator characters (`. / \ : { } [ ] "
  ' * ^ -`) are replaced with
  spaces. Leading minus (NOT operator) is stripped. See `src/database.rs`.

- **Path traversal protection**: Pattern write operations (`add_pattern`, `update_pattern`,
  `append_to_pattern`) validate file paths via `validate_within_dir()` (canonicalize + starts_with)
  and `validate_slug()` (component inspection). See `src/ingest.rs`.

### Hook Pipeline

- **Transcript path validation**: The `transcript_path` from hook input is validated to resolve
  under `$HOME` before reading. Paths that fail canonicalisation or resolve outside `$HOME` are
  silently skipped.

- **Bounded transcript read**: Only the last ~32KB of transcript files are read, preventing OOM on
  long sessions. Partial UTF-8 sequences and partial JSONL lines at the buffer boundary are safely
  handled.

- **Session deduplication file integrity**: Session IDs are hashed (FNV-1a, 16 hex chars) for
  deduplication filenames, preventing collisions from character-level sanitisation. Deduplication
  file access uses advisory file locking (`fd-lock`) across the full read-filter-write sequence to
  prevent TOCTOU races.

### Code Safety

- `unsafe_code = "deny"` enforced globally (one justified exception for sqlite-vec FFI registration
  in `src/database.rs`)
- All SQL uses parameterised queries — no string concatenation
- All subprocess calls use `std::process::Command` with explicit argument lists — no shell
  invocation
- Dependencies audited via `cargo-deny` in CI (advisories, licenses, bans)
- Clippy pedantic lints enabled at warn level

## Assumptions

- Lore runs as a local tool under the user's own permissions. It does not elevate privileges or
  access resources beyond what the user already has.
- Agent hook callers (Claude Code, Cursor, Opencode) provide legitimate `transcript_path` values
  under `$HOME`. The path validation is a defence-in-depth check, not a primary security boundary.
- Ollama runs on localhost. Non-localhost Ollama configurations are not security-hardened.
- The MCP transport is stdio — there is no network listener.

## Reporting Vulnerabilities

If you discover a security vulnerability, please report it through
[GitHub Security Advisories](../../security/advisories/new) or contact the maintainer directly. Do
not report security vulnerabilities through public channels such as GitHub Discussions.

Lore is not currently accepting external contributions (see [CONTRIBUTING.md](CONTRIBUTING.md)), but
security reports are always welcome.
