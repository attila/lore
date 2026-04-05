# Hook Pipeline and Plugin Reference

Lore integrates with Claude Code through two mechanisms: **hooks** that inject pattern context at
key moments during a session, and an **MCP server** that exposes search and pattern management as
callable tools. The MCP server works with any MCP-compatible client, but the hook integration is
currently Claude Code-specific.

This document explains the hook lifecycle, the plugin structure, and how to tune injection
behaviour. For the full search pipeline internals, see the
[Search Mechanics Reference](search-mechanics.md). For configuration options, see the
[Configuration Reference](configuration.md).

## Hook Lifecycle

Hooks fire at four points during an agent session. Each hook invokes `lore hook`, which reads the
event payload from stdin and writes structured JSON to stdout.

| Event        | Output field        | Matcher             | Purpose                                                                |
| ------------ | ------------------- | ------------------- | ---------------------------------------------------------------------- |
| SessionStart | `systemMessage`     | All tools           | Primes the session with a pattern index and meta-instruction           |
| PreToolUse   | `additionalContext` | `Edit\|Write\|Bash` | Searches for relevant patterns and injects them before the tool runs   |
| PostToolUse  | `additionalContext` | `Bash`              | Searches for patterns related to Bash errors (non-zero exit code only) |
| PostCompact  | `systemMessage`     | All tools           | Re-primes the session after context compression                        |

### SessionStart

Fires once at the beginning of every session. The hook creates (or truncates) the session
deduplication file, then returns a `systemMessage` containing:

- A meta-instruction telling the agent that patterns are injected automatically and should be
  followed as default conventions
- A compact index listing every pattern by title and tags

This gives the agent awareness of the knowledge base without injecting full content upfront.

### PreToolUse

Fires before every Edit, Write, or Bash tool invocation. This is the primary injection point. The
hook:

1. Extracts search terms from the tool input (file path, bash command, transcript context)
2. Searches the knowledge base for matching patterns
3. Expands results to include all sibling chunks from matched source files
4. Filters out patterns already injected in this session (deduplication)
5. Formats the results as imperative directives and returns them in `additionalContext`

The output format groups chunks by source file:

```
PROJECT CONVENTIONS (source: error-handling.md)
Apply these patterns when writing this code:

[full body text of each chunk from that source file]
```

### PostToolUse

Fires after Bash commands that exit with a non-zero status code. The hook extracts terms from the
stderr output and searches for patterns that might address the error. This is how lore surfaces
relevant conventions after a failure — for example, if a Bash command is blocked by permission
settings, lore can inject the pattern explaining the correct approach.

PostToolUse does not fire for successful commands or for non-Bash tools.

### PostCompact

Fires when the agent's context window is compressed (a natural event during long sessions). The hook
truncates the deduplication file and re-emits the same content as SessionStart — the full pattern
index and meta-instruction. This ensures the agent retains awareness of the knowledge base even
after earlier injections have been compressed away.

## The One-Tool-Call Delay

Patterns injected via `additionalContext` in PreToolUse enter the agent's transcript alongside the
result of the tool execution — after the agent has already decided its approach for that tool call.
This creates a one-call delay: the first Edit, Write, or Bash in a session executes without the
benefit of injected patterns, because the agent has not yet seen them.

From the second tool call onward, the patterns injected by the first hook are visible in the
transcript, and the agent follows them. This delay occurs once per session (and again after
PostCompact resets context). It is an architectural property of how `additionalContext` works in
Claude Code, not a bug.

## Subagent Behaviour

All agent types receive the pattern index from SessionStart — they know which patterns exist and
their tags. However, PreToolUse injection is skipped for Explore and Plan subagents because they are
read-only and do not edit files.

If you need a subagent to consult specific patterns (for example, a Plan subagent drafting an
implementation approach), prompt it to use the `search_patterns` MCP tool directly. The tool is
available to all agent types regardless of hook filtering.

## Session Deduplication

Lore tracks which patterns have already been injected within a session to avoid redundant injections
that waste context window space.

**Lifecycle:**

1. **SessionStart** creates or truncates the deduplication file at
   `/tmp/lore-session-{fnv1a_hash(session_id)}`
2. **PreToolUse** reads the file to check which chunk IDs have been injected, filters them from the
   current results, then appends the newly injected IDs
3. **PostCompact** truncates the file, allowing all patterns to be re-injected after context
   compression

The read-filter-write sequence is protected by an exclusive advisory file lock to prevent concurrent
hook invocations from losing writes.

**Important:** Deduplication is gated on file existence. If no SessionStart has run (for example,
during manual `lore hook` invocations from the command line), the deduplication file does not exist
and deduplication is skipped entirely.

## Error Contract

The hook must never break the agent. `lore hook` catches all errors, logs them to stderr, and exits
with status code 0 regardless. If the search engine fails, if the knowledge base is unavailable, or
if the deduplication file is locked — the hook exits silently and the agent continues unimpeded.

## Plugin Structure

The Claude Code plugin lives at `integrations/claude-code/` with this structure:

```
integrations/claude-code/
├── .claude-plugin/
│   └── plugin.json          # Plugin manifest: name, version, skill/MCP paths
├── hooks/
│   └── hooks.json           # Hook definitions for all four lifecycle events
├── mcp.json                 # MCP server configuration (stdio transport)
└── skills/
    └── search-lore/
        └── SKILL.md         # Manual search skill (user-invocable)
```

### Plugin Manifest

`plugin.json` declares the plugin's identity and points to the skills directory and MCP
configuration:

```json
{
  "name": "lore",
  "version": "0.1.0",
  "description": "Deterministic coding convention injection via lore knowledge base",
  "skills": "./skills/",
  "mcpServers": "./mcp.json"
}
```

### Hook Configuration

Hooks are auto-loaded from `hooks/hooks.json` by convention. Do not reference them in `plugin.json`
— doing so causes duplicate registration.

Each hook definition specifies the command, timeout, and an optional matcher:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit|Write|Bash",
        "hooks": [
          {
            "type": "command",
            "command": "lore hook",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

Hooks fork a fresh process for every event, so they always use the latest `lore` binary on PATH. If
you run `just install` to update the binary, hooks pick up the new version immediately — no session
restart required.

### MCP Server

`mcp.json` configures the MCP server for stdio transport:

```json
{
  "lore": {
    "command": "lore",
    "args": ["serve"],
    "type": "stdio"
  }
}
```

Unlike hooks, the MCP server is a long-running process started when the session begins. After
updating the `lore` binary, the MCP server continues running the old version until the session is
restarted. If you run `just install`, you must exit and relaunch the agent session for MCP tools to
use the updated binary.

As of Claude Code 2.1.92, the `/reload-plugins` command refreshes hooks but does not restart MCP
server processes.

### Skills

The `search-lore` skill provides a user-invocable search command (`/search-lore`) for manual pattern
lookups. It is configured with `disable-model-invocation: true` because hooks already handle
automatic injection — the skill exists for explicit, user-initiated searches only.

## Query Extraction from the Agent's Perspective

The hook reads different signals depending on the tool type:

| Tool        | Primary signal                           | Language detection                  | Additional signals |
| ----------- | ---------------------------------------- | ----------------------------------- | ------------------ |
| Edit, Write | `file_path` → extension + filename terms | From file extension                 | Transcript tail    |
| Bash        | `description` (fallback: `command`)      | From command text (cargo, npm, pip) | Transcript tail    |
| Read        | `file_path`                              | From file extension                 | Transcript tail    |

The transcript tail (last user message, up to 200 bytes) provides supplementary context. It helps
when the tool input alone does not produce enough terms — for example, when a Bash command contains
only short or stop-word terms.

For the full details of term extraction, cleaning, and query assembly, see the
[Search Mechanics Reference](search-mechanics.md).

## Tuning Injection Behaviour

Four settings control how aggressively lore injects patterns. All are configured in `lore.toml`
under the `[search]` section unless noted otherwise.

### Relevance Threshold (`search.min_relevance`)

Default: `0.6`. Controls the minimum normalised score a pattern must reach to be injected.

- **Raise** to reduce noise — fewer patterns injected, but only high-confidence matches
- **Lower** to increase recall — more patterns injected, including weaker matches that might still
  be relevant
- Set to `0.0` to disable the threshold entirely (inject all results)

The threshold applies only to hybrid search with successful embedding. When the search falls back to
FTS5 only, no threshold is applied.

### Result Count (`search.top_k`)

Default: `5`. Controls how many top results are considered before sibling expansion and
deduplication.

More results mean more context injected per tool call, which increases the chance of surfacing a
relevant pattern but also consumes more of the agent's context window.

### Search Mode (`search.hybrid`)

Default: `true`. When enabled, lore combines FTS5 lexical search with Ollama vector search using
Reciprocal Rank Fusion.

Set to `false` to use FTS5 only. This is faster (no Ollama round-trip at query time) but loses
semantic matching. Patterns are found only through keyword overlap, not meaning.

### Force Re-Ingest (`lore ingest --force`)

When you change pattern content or lore's indexing configuration (such as the FTS5 tokeniser), run a
force re-ingest to rebuild the index from scratch:

```sh
lore ingest --force
```

This drops and recreates the FTS5 virtual table, ensuring schema changes (such as porter stemming)
take effect. Without `--force`, delta ingest only processes files that have changed since the last
ingestion and does not recreate the table.

## Troubleshooting

### Patterns Are Not Surfacing

1. **Check the search engine directly:**

   ```sh
   lore search "your expected query terms" --top-k 5
   ```

   If the pattern does not appear, the issue is vocabulary coverage — see the
   [Pattern Authoring Guide](pattern-authoring-guide.md).

2. **Trace the hook pipeline:**

   ```sh
   LORE_DEBUG=1 claude
   ```

   Debug output on stderr (prefixed `[lore debug]`) shows the extracted query, search results,
   deduplication decisions, and injected content. This tells you whether the pattern was found but
   already deduplicated, found but below the relevance threshold, or not found at all.

3. **Check deduplication:** If the pattern was injected earlier in the session, deduplication
   prevents re-injection. PostCompact resets deduplication, so patterns become available again after
   context compression.

### MCP Tools Are Stale After Binary Update

The MCP server is a long-running process. After `just install`, exit and relaunch the agent session.
As of Claude Code 2.1.92, `/reload-plugins` alone is not sufficient — it refreshes hooks but does
not restart MCP servers.

### Hook Errors

Hooks never surface errors to the agent. If something goes wrong, the hook exits silently with
status 0. Check stderr output with `LORE_DEBUG=1` to diagnose hook failures.
