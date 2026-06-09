# lore Codex CLI plugin — requirements

_Brainstorm output, 2026-06-08. Standard scope, Lightweight artifact._

**2026-06-09 outcome:** this remains a valid implementation direction for local experimentation, but
public shipping is blocked. Codex CLI currently renders hook `hookSpecificOutput.additionalContext`
visibly in the terminal transcript. lore's deterministic hook injection requires full pattern bodies
to be model-visible while hidden from the operator's conversation view, as they are in Claude Code.
A summarized or title-only Codex mode is not product equivalent. Upstream tracker:
<https://github.com/openai/codex/issues/16933>.

## Why this work

lore today ships a Claude Code plugin (`integrations/claude-code/`). Users on OpenAI's Codex CLI
have no equivalent: lore's MCP server can be configured standalone, but the deterministic-injection
layer that makes lore load-bearing on Claude Code — SessionStart pinning of universal conventions,
PreToolUse convention injection on file edits and shell calls, PostToolUse error-driven pattern
lookup — is unavailable on Codex.

The engine module (`src/engine/`) is already agent-agnostic per the Track 1 engine/adapter split (PR
#39), and Codex's hook wire format is close enough to Claude Code's that the same
`hookSpecificOutput.additionalContext` envelope works on the events lore needs. A second integration
is a thin adapter and a manifest directory, not a new subsystem.

The shipping bar is **the same lore experience on Codex as on Claude Code, on the events Codex
exposes** — not strict behavioural parity. Where Codex's hook surface lets lore do something it
cannot do on Claude (the most relevant case today: `UserPromptSubmit`), the right default is to use
the surface, not withhold it for symmetry.

## What we are building

Three things:

1. A new adapter module `src/codex_hook.rs`, parallel to the existing Claude adapter `src/hook.rs`.
   It translates Codex's hook wire-format to and from lore's agent-agnostic engine.
2. A new CLI subcommand `lore codex-hook`, parallel to `lore hook`. Two subcommands rather than one
   auto-detecting subcommand, so that when a hook misbehaves the failing adapter is unambiguous.
3. A new integration directory `integrations/codex/`, parallel to `integrations/claude-code/`,
   containing the manifest files Codex needs to discover lore as a plugin
   (`.codex-plugin/plugin.json`, `hooks/hooks.json`), an MCP server config wired to `lore serve`,
   and the two existing skills (`search`, `coverage-check`) re-emitted with whatever frontmatter
   Codex's skills loader expects.

The engine module `src/engine/` is reused unchanged. Predicate evaluation, query extraction,
language inference, RRF retrieval — all the load-bearing logic — already operates on the
agent-neutral `CallContext` shape that was designed for exactly this kind of second adapter.

## Success criteria

The plugin delivers the following observable behaviour:

- A fresh Codex session in a lore-indexed repository emits the `## Pinned conventions` block at
  SessionStart.
- An `apply_patch` tool call injects the relevant conventions for the touched file's language via
  `additionalContext`.
- A `Bash` tool call injects relevant conventions for the recognised binary/language.
- A failed `Bash` command triggers PostToolUse error-driven pattern lookup against the error output.
- A `UserPromptSubmit` event injects conventions whose keywords match the submitted prompt text
  (Codex-only surface; no Claude analog).
- A `/compact` re-emits `## Pinned conventions` via SessionStart with `source: "compact"`.
- The `search_patterns` and `add_pattern` MCP tools are callable from Codex via the bundled
  `lore serve` stdio server.
- The `search` and `coverage-check` skills are explicitly invokable inside Codex through the
  supported skill/plugin surfaces (`/skills`, `$skill`, or `@plugin`). Direct `/lore:*` slash
  commands are a nice-to-have if Codex exposes installed plugin skills that way.
- The Codex adapter records hook activity through the same `trace::record` surface as the Claude
  adapter, so `lore trace why <session>` queries work across both agents.
- New Codex adapter tests pin the wire contract; existing Claude integration tests continue to pass.

## Approach

### Adapter module — `src/codex_hook.rs`

Parallel to `src/hook.rs`. Reads Codex's JSON envelope from stdin, builds a `CallContext`, calls the
same `engine::*` functions, writes Codex's JSON envelope on stdout. The translation table is small
because Codex's wire format is ~95% identical to Claude's; the only meaningful differences are:

| Wire concern                      | Claude                                                                                       | Codex                                                |
| --------------------------------- | -------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| Tool name for file edits          | `Edit`, `Write`, `MultiEdit`                                                                 | `apply_patch`                                        |
| Matcher syntax                    | pipe-list (`"Edit\|Write\|Bash"`)                                                            | regex (`"^(apply_patch\|Bash)$"`)                    |
| Post-compaction re-prime          | dedicated `PostCompact` event (currently degraded — additionalContext rejected by validator) | `SessionStart` with `source: "compact"`              |
| `tool_input` shape for file edits | `{ file_path, new_string, old_string, … }`                                                   | unified-diff `patch` field on `apply_patch`          |
| Extra stdin fields                | n/a                                                                                          | `permission_mode`, `model`, `turn_id`, `tool_use_id` |
| `additionalContext` envelope      | `hookSpecificOutput.additionalContext`                                                       | `hookSpecificOutput.additionalContext` (identical)   |

The `apply_patch` tool_input is the single nontrivial translation — `engine::query::extract_query`
reads `file_path` and the diff body for Claude's Edit/Write, and the Codex adapter must derive the
equivalent signals from a unified-diff `patch` string. Implementation question for the planner:
parse the diff header to extract the touched file path, and treat the `+` lines as the
convention-relevant content. This is the single piece of work today that genuinely requires writing
new logic rather than translating.

### CLI subcommand — `lore codex-hook`

Hooks invoke a command line; Codex's `hooks/hooks.json` will name `lore codex-hook` exactly as
Claude's calls `lore hook`. The subcommand is a thin shell over `codex_hook::handle()`. No
flag-switched router, no auto-detect from stdin shape.

### Integration directory — `integrations/codex/`

```
integrations/codex/
├── .codex-plugin/
│   └── plugin.json
├── hooks/
│   └── hooks.json
├── .mcp.json           # confirmed during implementation and referenced by plugin.json
└── skills/
    ├── search/
    │   └── SKILL.md
    └── coverage-check/
        └── SKILL.md
```

The skill bodies port verbatim from `integrations/claude-code/skills/`; only the frontmatter is
reviewed against Codex's skills loader expectations. The MCP server config invokes `lore serve` over
stdio, identical to the Claude side.

### Hook wiring — `integrations/codex/hooks/hooks.json`

Four event subscriptions:

| Event              | Matcher                                                                   | Purpose                                                                                                                                                                            |
| ------------------ | ------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SessionStart`     | none (fires on all four sources: `startup`, `resume`, `clear`, `compact`) | Emit `## Pinned conventions`. The `compact` source covers the post-compaction re-prime case.                                                                                       |
| `PreToolUse`       | `"^(apply_patch\|Bash)$"`                                                 | Predicate-evaluated convention injection on file edits and shell calls.                                                                                                            |
| `PostToolUse`      | `"^Bash$"`                                                                | Error-driven pattern lookup on non-zero exit.                                                                                                                                      |
| `UserPromptSubmit` | none                                                                      | Convention injection driven by query extraction over the submitted prompt text. Codex-native surface — fires once per user turn, complementing the per-tool PreToolUse injections. |

`PreCompact`, `PermissionRequest`, `SubagentStart`, `SubagentStop`, and `Stop` are not wired — see
"Out of scope" below.

## Out of scope (today)

- **`PreCompact` hook.** Not load-bearing in Claude lore; no reason to add it for Codex either.
- **`PermissionRequest`, `Subagent*`, `Stop` hooks.** Not lore's role.
- **`updatedInput` rewrite power.** Lore injects conventions, it does not rewrite tool inputs.
  PreToolUse output never sets `updatedInput`.
- **MCP tool calls in the PreToolUse matcher.** Narrow `^(apply_patch|Bash)$` only. Matching
  arbitrary MCP tools would create noise without obvious value at this stage.
- **Public marketplace publishing / Homebrew tap.** Local Codex testing goes through an in-repo
  marketplace manifest; public Claude/Codex marketplace distribution follows as a separate roadmap
  item once the Codex integration is proven locally.
- **Single auto-detecting `lore hook` subcommand.** A future refactor if maintaining two parallel
  subcommands becomes painful; separate subcommands keep failure-mode diagnosis unambiguous.
- **Linux / Windows install instructions.** Initial install path documents Mac; cross-platform
  installer docs follow when distribution matters.

## Dependencies and assumptions

- The `src/engine/` module is treated as already agent-agnostic per the Track 1 engine/adapter split
  (PR #39). The engine has so far only been driven by Claude-shaped `CallContext` instances; the
  Codex adapter is its first second-driver. Any divergence the implementation surfaces (predicate
  evaluation, language inference, RRF behaviour on synthesized paths) is treated as a same-iteration
  blocker, not a deferred follow-up.
- The locally installed Codex version is assumed to match the codex source-of-truth in
  `github.com/openai/codex` `codex-rs/hooks/src/` as of 2026-06-08 (`additionalContext` accepted on
  PreToolUse / PostToolUse / SessionStart / UserPromptSubmit; `hooks.json` envelope identical to
  Claude's; snake_case stdin; regex matchers).
- A `lore` binary is on `PATH` for both Codex and Claude to invoke.
- `lore serve` over stdio is the MCP transport for both agents; HTTP transport is out of scope.
- `apply_patch`'s `tool_input` field name is `patch` carrying a unified-diff string. Confirm at
  implementation time by reading codex source `codex-rs/core/src/tools/runtimes/` or by inspecting
  one real Codex invocation's stdin JSON.
- Before the adapter is written, one real Codex stdin JSON is captured per relevant event
  (`PreToolUse` for `apply_patch`, `PreToolUse` for `Bash`, `PostToolUse` for `Bash`,
  `SessionStart`, `UserPromptSubmit`) and diffed against the schema declarations in the verified
  Codex hook surface section. The binary-on-PATH version is treated as authoritative if it diverges
  from main.

## Risks

- **`apply_patch` parsing.** Unified-diff parsing is the only piece of net-new logic. A naïve parse
  (grep for `+++ b/…` header) is sufficient initially; richer multi-file diffs can be handled
  iteratively. If the parser fails on a real Codex tool call, lore degrades to no-injection on that
  call rather than crashing — exit zero, no stdout, hook continues.
- **Skill loader frontmatter divergence.** Codex's skills loader may require fields Claude does not,
  or reject Claude-specific fields. Mitigation: read codex source `codex-rs/skills/src/` before
  writing the SKILL.md files; iterate if the loader rejects.
- **Codex wire-format churn.** If a Codex release changes a hook envelope, the plugin may need a
  patch. Acceptable risk; the engine/adapter split keeps the blast radius small.
- **Engine surprises from a non-Claude `CallContext`.** The engine's predicate evaluation, language
  inference, and RRF retrieval have only been exercised against Claude-shaped inputs. A new
  fixture-driven test that drives the engine from a synthesized Codex `CallContext` (apply_patch +
  Bash) is the cheapest insurance against silent divergence.
- **Maintainer cost doubles per integration.** Every engine change must now be tested against two
  adapters, and every new lore feature multiplies across N integrations. Accepted cost for now; the
  engine/adapter contract is the load-bearing surface to harden before a third adapter (Cursor,
  opencode) lands.

## Open implementation questions (for `/ce-plan`)

1. Codex `SKILL.md` frontmatter — does Codex respect Claude Code's `disable-model-invocation` and
   `user-invocable`, or use different keys?
2. Codex plugin packaging details — confirm whether plugin-bundled MCP config is referenced from
   `.codex-plugin/plugin.json` as `.mcp.json`, and verify the exact manifest keys against the local
   Codex version before UAT.

## Verified Codex hook surface (reference)

Compiled from `github.com/openai/codex` `codex-rs/hooks/src/` at 2026-06-08.

- **Hook events:** `SessionStart`, `PreToolUse`, `PostToolUse`, `PreCompact`, `PostCompact`,
  `UserPromptSubmit`, `PermissionRequest`, `SubagentStart`, `SubagentStop`, `Stop`.
- **SessionStart sources:** `startup`, `resume`, `clear`, `compact` — matcher field on the
  SessionStart entry filters on this string.
- **Tool name vocabulary (sampled from codex integration tests):** `Bash` (shell), `apply_patch`
  (file edits), MCP tool names pass through verbatim.
- **Matcher syntax:** regex string. `^Bash$` matches Bash only; bare `Bash` matches as a substring.
  Use anchored alternation for multi-tool: `^(apply_patch|Bash)$`.
- **Stdin field naming:** snake_case. Common fields across events: `session_id`, `turn_id`,
  `transcript_path`, `cwd`, `hook_event_name`, `model`, `permission_mode`. PreToolUse and
  PostToolUse add `tool_name`, `tool_input`, `tool_use_id`; PostToolUse also adds `tool_response`.
  UserPromptSubmit adds `prompt`. SessionStart adds `source`.
- **Stdout envelope:** camelCase. Universal fields: `continue`, `stopReason`, `suppressOutput`,
  `systemMessage`. Hook-specific output under `hookSpecificOutput` with `hookEventName` and
  event-specific fields. `additionalContext` is supported on PreToolUse, PostToolUse, SessionStart,
  UserPromptSubmit. PreToolUse additionally supports `permissionDecision: "allow" | "deny"`,
  `permissionDecisionReason`, and `updatedInput`.
- **`hooks.json` shape (verified identical to Claude's):**

  ```json
  {
    "hooks": {
      "PreToolUse": [
        {
          "matcher": "^(apply_patch|Bash)$",
          "hooks": [
            {
              "type": "command",
              "command": "lore codex-hook",
              "timeout": 10
            }
          ]
        }
      ]
    }
  }
  ```

## ROADMAP and CHANGELOG

The implementation PR updates `ROADMAP.md` and `CHANGELOG.md` per project conventions — see project
memories `feedback_roadmap_update_in_feature_pr` (move the new entry to `## Completed`; leave the
existing `Additional agent integrations (Cursor, opencode)` Future bullet untouched, since Codex was
not previously named in it) and `feedback_changelog_entries` (one user-facing assertive-voice line
ending in `(#N)`). The exact wording is the implementer's call.

## Next step

Hand off to `/ce-plan` for the implementation plan. Sequencing, test fixtures, and per-step
verification are the planner's call — this requirements doc constrains scope and success criteria,
not order of operations.
