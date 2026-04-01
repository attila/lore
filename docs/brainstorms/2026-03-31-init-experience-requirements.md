---
date: 2026-03-31
topic: init-experience
---

# Improve Init Experience: Config Location and MCP Setup Output

## Problem Frame

Lore is now installable as a system tool, but its config (`lore.toml`) and database (`knowledge.db`)
default to the current working directory. This means MCP setup instructions require `--config` with
an absolute path, and the config is not discoverable from other directories. Additionally, the init
output only shows the JSON MCP config block — users who prefer the `claude mcp add` CLI command have
to construct it themselves.

## Requirements

**Config Location**

- R1. Default config location is `$XDG_CONFIG_HOME/lore/lore.toml`, falling back to
  `~/.config/lore/lore.toml` when `$XDG_CONFIG_HOME` is unset. Same resolution on all platforms
  (Linux and macOS both use `~/.config/`). When neither the XDG variable nor `$HOME` is set,
  commands fail with a clear error directing the user to pass `--config` explicitly.
- R2. Default database location is `$XDG_DATA_HOME/lore/knowledge.db`, falling back to
  `~/.local/share/lore/knowledge.db` when `$XDG_DATA_HOME` is unset. Same `$HOME` fallback behavior
  as R1.
- R3. Config resolution does not search CWD. The only override is the `--config` CLI flag.
- R4. `lore init` creates parent directories for both config and database paths if they don't exist.
  Re-running `lore init` overwrites the existing config file at the target location.

**CLI Overrides**

- R5. The global `--config` flag overrides the default config location for all commands. When not
  explicitly provided, commands use the XDG default.
- R6. `lore init` accepts a `--database` flag to override the default database location at creation
  time.
- R7. (Existing behavior, preserved) The `database` field in `lore.toml` stores the absolute
  database path used by all non-init commands. A `--database` CLI flag is not needed on other
  commands since the config already stores the absolute path.

**Init Output**

- R8. After init completes, display both the JSON MCP config block and the equivalent
  `claude mcp add` CLI command. The `--database` flag does not affect the MCP config output since
  the database path is stored in `lore.toml` and resolved at serve time.
- R9. When config is at the default XDG location, omit `--config` from both the JSON args and the
  CLI command (lore finds it automatically, so the output is simpler).
- R10. When config is at a non-default location (user passed `--config`), include
  `--config <absolute-path>` in both output snippets. The path is always absolute regardless of what
  the user typed.

## Success Criteria

- `lore init --repo /path/to/kb` creates config at `~/.config/lore/lore.toml` and database at
  `~/.local/share/lore/knowledge.db` without any extra flags
- The `database` field in the generated `lore.toml` contains an absolute path (not relative), so
  commands work from any working directory
- `lore serve` (no flags) finds and loads the config from the XDG default location
- `lore search <query>` (no flags) resolves the database location from the config file and returns
  results
- `lore init` output shows both JSON and CLI setup instructions, with paths appropriate to whether
  defaults were used
- When `lore init --config /custom/path.toml --repo ...` is used, both the JSON and CLI output
  snippets include `--config /custom/path.toml`
- Existing `--config /custom/path.toml` workflow continues to work

## Scope Boundaries

- Single global config — multi-project use requires explicit `--config` per project (may revisit
  later)
- No CWD search fallback (may revisit later)
- No `LORE_CONFIG` environment variable
- No `--database` flag on commands other than `init` (the config file handles this)
- No migration of existing CWD-based configs — users re-run `lore init` (breaking change, acceptable
  pre-1.0)
- No new crate dependencies — use `std::env::var` + `$HOME` for path resolution
- `knowledge_dir` resolution is unchanged — `--repo` is canonicalized to an absolute path and stored
  in the config as-is

## Key Decisions

- **XDG with manual resolution over `dirs` crate**: Check `$XDG_CONFIG_HOME`/`$XDG_DATA_HOME` env
  vars, fall back to `$HOME/.config/` and `$HOME/.local/share/`. No platform-native divergence —
  `~/.config/` on both Linux and macOS. Zero new dependencies.
- **No CWD in search path**: Keeps behavior predictable. `--config` is the only override mechanism.
- **Database location configurable at two levels**: Default XDG data dir, overridable by
  `--database` flag on `init` (which writes it into config), then stored as absolute path in
  `lore.toml` for all subsequent commands.
- **`--config` as `Option<PathBuf>` in clap**: Change from `default_value_os_t` to
  `Option<PathBuf>`, resolve to XDG default in code when `None`. This enables R9/R10 conditional
  output and distinguishes "user passed --config" from "using default." Resolve once after parse,
  before dispatching to command functions.

## Outstanding Questions

### Deferred to Planning

- [Affects R1, R2][Technical] Whether path resolution helpers should live in `config.rs` or a new
  module.

## Next Steps

-> `/ce:plan` for structured implementation planning
