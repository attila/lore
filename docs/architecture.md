# Architecture invariants

Design rules that shape how lore is built. Read this before introducing a new read surface, a new
write path, or any runtime disk access. Each invariant names what it protects and what its
exceptions are, so edge cases can be judged without re-litigating the principle.

## Invariants

1. [`knowledge.db` is the sole runtime read surface for indexed content](#knowledgedb-is-the-sole-runtime-read-surface-for-indexed-content)

## `knowledge.db` is the sole runtime read surface for indexed content

**Rule.** At runtime, `lore` reads indexed content — pattern bodies, chunks, titles, tags — from
`knowledge.db` only. No runtime code path opens a markdown file from the patterns directory to serve
agent context.

**Sanctioned exception.** Ingest is the one sanctioned disk→DB pipeline. `full_ingest`,
`delta_ingest`, `ingest_single_file`, and the write operations behind `add_pattern` /
`update_pattern` / `append_to_pattern` all read markdown from disk and write to `knowledge.db`.
Authoring writes to the patterns directory via those three MCP tools are sanctioned _only_ when they
end by re-ingesting the written file in the same call — a future authoring path that writes without
re-ingesting would leave disk and DB out of sync and violate this clause.

**Out of scope.** The invariant is about _indexed content_, not about runtime I/O in general. The
following are explicitly not covered and may read from disk at runtime without changing this rule:

- Session-local state: the dedup file (`/tmp/lore-session-*`), the lockfile.
- Agent-harness inputs: the Claude Code transcript tail read by `last_user_message` in `src/hook.rs`
  to enrich `PreToolUse` queries.
- Configuration: `knowledge.toml` loaded at CLI startup.
- Git metadata: `git rev-parse` subprocess invocations to detect repository state.

**Why this invariant exists.** The documented violation it corrects: PR #33 (universal patterns)
shipped a `render_pinned_conventions` implementation that re-read source markdown at `SessionStart`
to populate the `## Pinned conventions` section. Before #33, every runtime reader of indexed content
— the MCP server, both hooks, every CLI subcommand — went through `knowledge.db` alone. Sandbox test
drives surfaced the consequence immediately: the agent suddenly needed two read surfaces instead of
one. See `docs/plans/2026-04-22-001-feat-db-sole-read-surface-plan.md` for the restoration plan. The
invariant is now enforced by the `patterns` table (authorial bodies live in the DB) and by the
static-grep checks in `tests/invariants.rs`.

**How to enforce / extend.**

- The `patterns` table carries `raw_body` for each indexed file. Render callers query that directly;
  no re-read needed.
- `tests/invariants.rs` runs on every CI invocation. The first test pins "no pattern-level
  aggregation over `chunks`"; the second pins "no unsanctioned `fs::read*` / `File::open` /
  `OpenOptions` in runtime modules" against a documented allow-list.
- If you need to add a new runtime disk read, update the allow-list in `tests/invariants.rs` _and_
  explain the carve-out here. Reviewers reading this doc will see the exception and its reason;
  silent additions fail CI.

---

## Adding a new invariant

New invariants follow the same shape:

- Rule (one sentence if possible).
- Sanctioned exceptions, explicit.
- Out-of-scope carve-outs, explicit.
- Why it exists (the motivating incident).
- How to enforce or extend.

Append a bullet to the index at the top. The doc stays flat until the section count is awkward to
navigate in one file — then consider splitting per invariant.
