---
title: "doc: Lore product documentation — authoring guide, search mechanics, agent integration, configuration"
type: feat
status: completed
date: 2026-04-05
origin: docs/brainstorms/2026-04-04-pattern-authoring-guide-requirements.md
---

# doc: Lore Product Documentation

## Overview

Ship four product documentation files that fill the gap between "I installed lore" and "I'm using it
effectively." The pattern authoring guide is the centerpiece — grounded in real dogfooding evidence
about what makes patterns work for agent consumption. The remaining three docs cover the search
pipeline internals, the hook injection lifecycle, and the configuration surface.

All four docs live under `docs/` as plain markdown. The README remains the landing page and will
link to these docs. Rendering pipelines (mdbook, man pages) are explicitly deferred.

## Problem Frame

Lore's README covers installation and basic usage. But dogfooding revealed that patterns can exist,
be discoverable, and still fail to change agent behavior. Users have no guidance on writing
effective patterns, no reference for the hook injection pipeline, no documentation of configuration
options, and no way to debug discoverability issues beyond reading source code.

(see origin: `docs/brainstorms/2026-04-04-pattern-authoring-guide-requirements.md`)

## Requirements Trace

- R1. Pattern authoring guide for all users, grounded in dogfooding evidence
- R2. Brief search mechanics inline in authoring guide, link to full reference
- R3. Real before/after pattern examples, not hypothetical
- R4. Three traits of effective patterns: imperative voice, incident grounding (when applicable),
  discoverable vocabulary
- R5. Anti-patterns section with concrete failure examples
- R6. Manual review checklist for draft patterns
- R7. Stop word list and minimum term length documented
- R8. Hook query extraction pipeline at a high level
- R9. Vocabulary coverage technique
- R10. Tag strategy guidance
- R11. LORE_DEBUG as primary debugging technique
- R12. Full search pipeline reference (FTS5, weights, RRF, stemming, dedup, sibling expansion)
- R13. Worked examples: tool call → query → search results → injected context
- R14. Stop word list, term cleaning rules, hex-like filtering with exact thresholds
- R15. Hook lifecycle: SessionStart, PreToolUse, PostToolUse, PostCompact
- R16. Plugin setup beyond README's 3-line snippet
- R17. Query extraction from agent's perspective
- R18. Tuning injection behavior (min_relevance, top_k, hybrid, --force)
- R19. All lore.toml fields with defaults, types, examples
- R20. Environment variables (LORE_DEBUG, XDG_CONFIG_HOME, XDG_DATA_HOME)
- R21. XDG path resolution and override behavior

## Scope Boundaries

- Does NOT replace the README — complements it with links
- Does NOT modify existing patterns in the knowledge repo
- Does NOT change lore's codebase — pure documentation
- Rendering pipelines (mdbook, man pages) are deferred
- Search mechanics inline in the authoring guide is authoring context, not CLI documentation

## Context & Research

### Relevant Code and Patterns

- `src/hook.rs:558-612` — query extraction pipeline (language anchor, bash inference, transcript
  tail, term cleaning, FTS5 assembly)
- `src/hook.rs:27-33` — stop word list (60 words)
- `src/hook.rs:799-832` — `clean_terms()` (strip non-alpha, filter <3 chars, filter hex-like, filter
  stop words, dedup)
- `src/database.rs:249` — FTS5 column weights: title=10.0, body=1.0, tags=5.0
- `src/database.rs:311` — RRF hybrid merge (k=60, normalized to 0-1)
- `src/database.rs:451` — `sanitize_fts_query()` — character replacement rules
- `src/config.rs` — full Config struct with all fields and defaults
- `src/config.rs:88-108` — XDG path resolution (`resolve_xdg_base()`)
- `src/debug.rs` — `LORE_DEBUG` env var, LazyLock caching, `lore_debug!()` macro
- `integrations/claude-code/hooks/hooks.json` — hook definitions, matchers, timeouts
- `integrations/claude-code/.claude-plugin/plugin.json` — plugin manifest
- `integrations/claude-code/mcp.json` — MCP server config

### Institutional Learnings

- `docs/solutions/database-issues/fts5-query-construction-for-hook-based-search-2026-04-02.md` —
  language anchor + OR enrichment pattern, sibling chunk injection
- `docs/solutions/database-issues/fts5-query-sanitization-crashes-on-special-chars-2026-04-02.md` —
  which chars crash FTS5, sanitization rules
- `docs/solutions/integration-issues/additional-context-timing-in-pretooluse-hooks-2026-04-02.md` —
  one-tool-call delay for `additionalContext` visibility. First Edit/Write may not follow injected
  conventions; agent self-corrects from the second edit onward
- `docs/solutions/logic-errors/session-dedup-lifecycle-and-deny-first-touch-2026-04-02.md` — dedup
  gated on `path.exists()`, four-phase lifecycle
- `docs/solutions/integration-issues/claude-code-plugin-assembly-pitfalls-2026-04-02.md` — hooks
  auto-loaded, MCP json at plugin root, `disable-model-invocation` for skills
- `docs/solutions/integration-issues/reload-plugins-does-not-restart-mcp-servers-2026-04-03.md` —
  `/reload-plugins` refreshes hooks but not MCP servers; full session restart needed after binary
  update
- `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md` — `--json`
  mode suppresses stderr entirely

## Key Technical Decisions

- **Content before delivery:** Write plain markdown docs first. mdbook/man page rendering is a
  separate future task that consumes the same source files
- **Progressive disclosure across docs:** The authoring guide includes brief search mechanics
  (enough to understand vocabulary choices). The search mechanics reference goes deep. The
  integration guide explains hooks from the user's perspective. The config reference is
  lookup-oriented
- **Real examples only:** The authoring guide uses actual patterns from dogfooding (e.g.,
  `agents/unattended-work.md`, `rust/tooling.md`, `workflows/atlassian-mcp.md`) as good/bad examples
- **Cross-linking over duplication:** Each doc references the others where relevant rather than
  repeating content

## Writing Quality Bar

All four documents must meet this standard:

- **Oxford-quality grammar:** Correct punctuation, subject-verb agreement, parallel structure,
  consistent tense. No sloppy tech-talk or informal shortcuts. Grammatical precision signals
  trustworthy technical content
- **Direct and natural voice:** Grammatically correct does not mean stiff. Active voice, concrete
  verbs, second person ("you"), short sentences. Match the README's tone — authoritative without
  being academic
- **Every section passes the "so what?" test:** If a reader cannot act on it, cut it or rewrite it
- **Lead with the action, not the explanation:** Show the command or the example first, then explain
  why it works
- **One idea per paragraph, three to four sentences maximum:** Dense paragraphs are a documentation
  bug
- **Code examples are complete and runnable:** No `...` ellipsis that leaves the reader guessing
- **Scannable structure:** A user skimming finds what they need in seconds. Headings, tables, and
  code blocks do the heavy lifting; prose connects them
- **Consistent terminology:** Use the same term for the same concept throughout all four docs.
  "Pattern" not "rule" or "convention" interchangeably. "Hook" not "handler" or "callback"
- **Cross-links are tested:** Every `[text](path)` points to a real file

## Open Questions

### Resolved During Planning

- **Should the authoring checklist be separate?** No — inline at the end of the authoring guide as a
  summary section. It's short enough and serves as a natural conclusion
- **What worked examples for search mechanics?** Three examples covering distinct query shapes: (1)
  Edit .rs file → language anchor + filename terms, (2) Bash `gh pr create --body-file` → no
  language, command terms, (3) PostToolUse Bash error → stderr terms. These cover the three main
  extraction paths
- **Tutorial or reference structure for the authoring guide?** Tutorial flow — it tells a story from
  "why patterns fail" through "how to write effective ones" to "how to verify they work." The other
  three docs use reference structure (lookup-oriented)

### Deferred to Implementation

- Exact prose and section headings will be determined during writing
- Whether to include the full 60-word stop word list inline or as a collapsible/appendix section

## Implementation Units

- [x] **Unit 1: Pattern authoring guide**

  **Goal:** Ship `docs/pattern-authoring-guide.md` — the primary guide for writing effective
  patterns.

  **Requirements:** R1, R2, R3, R4, R5, R6, R7, R8, R9, R10, R11

  **Dependencies:** None

  **Files:**
  - Create: `docs/pattern-authoring-guide.md`

  **Approach:**
  - Tutorial flow: problem → principles → practice → verification
  - Open with the core insight: patterns that describe what exists fail; patterns that state what to
    do succeed. Use the `rust/tooling.md` vs `workflows/atlassian-mcp.md` contrast
  - Three main sections for the three traits (R4): imperative voice, incident grounding (framed as
    "when applicable" not mandatory), discoverable vocabulary
  - Anti-patterns section (R5) with concrete dogfooding failures: the `gh pr edit --body` incident
    (vocabulary gap), the `just ci` invisibility (stop words), the passive `rust/tooling.md`
    (missing mandate)
  - Brief search mechanics section (R2, R7, R8): how lore finds patterns, stop words, term length
    filter, query extraction summary. Link to `docs/search-mechanics.md` for details
  - Vocabulary coverage technique (R9): practical method for auditing a pattern's term coverage
  - Tag strategy section (R10): when tags add discoverability vs duplicate body terms
  - LORE_DEBUG section (R11): how to trace whether a pattern surfaces during real usage
  - Close with the review checklist (R6): concise list of checks derived from the failure modes

  **Patterns to follow:**
  - Tone and structure of `SECURITY.md` (clear, concise, table-driven where appropriate)
  - Before/after examples drawn from real patterns in the lore-patterns repo at
    `/srv/misc/Projects/repos/lore-patterns/`

  **Test expectation:** none — pure documentation

  **Verification:**
  - All 11 requirements (R1-R11) addressed
  - Every example is from a real pattern, not hypothetical
  - The checklist covers: imperative voice, vocabulary coverage, stop word avoidance, incident
    context (when applicable), tag relevance

- [x] **Unit 2: Search mechanics reference**

  **Goal:** Ship `docs/search-mechanics.md` — detailed pipeline reference for power users debugging
  discoverability.

  **Requirements:** R12, R13, R14

  **Dependencies:** None (can be written in parallel with Unit 1)

  **Files:**
  - Create: `docs/search-mechanics.md`

  **Approach:**
  - Reference structure with clear sections for each pipeline stage
  - **Query construction:** language anchor + OR enrichment, the four assembly cases (lang+terms,
    lang-only, terms-only, none), fallback behavior
  - **Term cleaning:** stop word list (all 60 words), 3-char minimum, hex-like filtering (6+ chars
    all `[0-9a-f]`), dedup
  - **FTS5 search:** column weights (title=10.0, tags=5.0, body=1.0, source_file=0.0), porter
    stemming via `tokenize = 'porter unicode61'`, query sanitization rules (which chars are
    stripped, which preserved)
  - **Vector search:** embedding via Ollama, cosine distance via sqlite-vec, embed text includes
    title + tags + body
  - **Hybrid merge / RRF:** k=60, score normalization to 0-1, `max_rrf = 2.0 / (k + 1.0)`
  - **min_relevance threshold:** default 0.6, only applied for hybrid with successful embedding, not
    applied for FTS-only
  - **Sibling chunk expansion:** all chunks from matched source files are included
  - **Dedup:** per-session file with FNV-1a hashed session ID, fd-lock advisory locking, reset on
    SessionStart/PostCompact
  - **Worked examples (R13):** three examples tracing a real tool call through the full pipeline:
    1. Edit `src/validate_email.rs` → `rust AND (validate OR email)` → matches `rust/error-handling`
    2. Bash `gh pr create --body-file /tmp/pr.md` → `create OR body OR file OR tmp` → matches
       `agents/unattended-work`
    3. PostToolUse Bash error with stderr → OR query from error terms → matches relevant pattern

  **Patterns to follow:**
  - `SECURITY.md` trust boundaries table style for the pipeline stages
  - Code references use `src/file.rs:function_name` format, not line numbers

  **Test expectation:** none — pure documentation

  **Verification:**
  - All three requirements (R12-R14) addressed
  - Pipeline description matches current code (verified against research output)
  - Worked examples are traceable through the actual code paths

- [x] **Unit 3: Hook pipeline and plugin reference**

  **Goal:** Ship `docs/agent-integration.md` — how patterns reach the agent through the hook
  lifecycle.

  **Requirements:** R15, R16, R17, R18

  **Dependencies:** None (can be written in parallel with Units 1-2)

  **Files:**
  - Create: `docs/agent-integration.md`

  **Approach:**
  - Reference structure organized around the hook lifecycle
  - **Hook lifecycle (R15):** table of four events (SessionStart, PreToolUse, PostToolUse,
    PostCompact) with output type, matcher, and behavior
  - **Plugin setup (R16):** full walkthrough of `integrations/claude-code/` structure — plugin.json,
    mcp.json, hooks.json, skills/. Explain auto-loading (hooks loaded by convention, not referenced
    in plugin.json), MCP json placement, `disable-model-invocation` for skills
  - **One-tool-call delay:** document that `additionalContext` enters the transcript after the tool
    runs, not while planning the next call. First Edit/Write may not follow injected conventions;
    agent self-corrects from the second edit onward. This is architectural, not a bug
  - **Query extraction from agent's perspective (R17):** what signals the hook reads from each tool
    type (file_path → language + filename terms, Bash description → command terms, transcript → last
    user message), and how they map to search queries. Cross-reference `docs/search-mechanics.md`
    for the full pipeline
  - **Agent type filtering:** Explore and Plan subagents are skipped (no injection)
  - **Dedup lifecycle:** SessionStart creates file, PreToolUse appends, PostCompact resets. Gated on
    `path.exists()` so manual CLI invocations skip dedup
  - **Tuning injection (R18):** `min_relevance` (raise to reduce noise, lower to increase recall),
    `top_k` (more results = more context), `hybrid` (disable for FTS-only, faster but no semantic
    matching), `--force` re-ingest (drops and recreates FTS table to pick up schema changes)
  - **Troubleshooting:** `/reload-plugins` refreshes hooks but not MCP servers — restart session
    after binary update. `LORE_DEBUG=1` traces hook queries and decisions
  - **Error contract:** `cmd_hook()` catches all errors, always exits 0. Hooks must never break the
    agent

  **Patterns to follow:**
  - Institutional learnings from `docs/solutions/integration-issues/` for accuracy on timing,
    assembly, and reload behavior

  **Test expectation:** none — pure documentation

  **Verification:**
  - All four requirements (R15-R18) addressed
  - Hook lifecycle matches `hooks.json` and `src/hook.rs` behavior
  - The one-tool-call delay is documented accurately per the institutional learning
  - Plugin setup matches the actual `integrations/claude-code/` structure

- [x] **Unit 4: Configuration reference**

  **Goal:** Ship `docs/configuration.md` — lookup-oriented reference for all configuration options.

  **Requirements:** R19, R20, R21

  **Dependencies:** None (can be written in parallel with Units 1-3)

  **Files:**
  - Create: `docs/configuration.md`

  **Approach:**
  - Reference structure: table-driven, one section per config area
  - **lore.toml fields (R19):** table with field, type, default, description for every field in the
    Config struct: `knowledge_dir`, `database`, `bind`, `ollama.host`, `ollama.model`,
    `search.hybrid`, `search.top_k`, `search.min_relevance`, `chunking.strategy`,
    `chunking.max_tokens`, `git.inbox_branch_prefix`
  - **Environment variables (R20):** table with variable, purpose, values for `LORE_DEBUG`
    (`1`/`true`/`yes`, cached for process lifetime), `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `HOME`
  - **XDG path resolution (R21):** explain the resolution chain — if XDG var is set and non-empty,
    use it; otherwise fall back to `$HOME/{subpath}`; if `$HOME` unset, error with `--config` hint.
    Show default paths: `~/.config/lore/lore.toml`, `~/.local/share/lore/knowledge.db`
  - **CLI flags:** table of global and per-command flags: `--config`, `--json`, `--force`,
    `--top-k`, `--repo`, `--model`, `--bind`, `--database`
  - **MCP tool input limits:** table from the security hardening work: query (1024 bytes), title
    (512), source_file (512), heading (512), body (256KB), tags (8KB), top_k (100)

  **Patterns to follow:**
  - `SECURITY.md` trust boundaries table style
  - `src/config.rs` Config struct as the source of truth for all fields

  **Test expectation:** none — pure documentation

  **Verification:**
  - All three requirements (R19-R21) addressed
  - Every `lore.toml` field from Config struct is documented
  - XDG resolution matches `resolve_xdg_base()` behavior

- [x] **Unit 5: README updates**

  **Goal:** Add a Documentation section to the README linking to the four new docs.

  **Requirements:** None (supporting work)

  **Dependencies:** Units 1-4

  **Files:**
  - Modify: `README.md`

  **Approach:**
  - Add a `## Documentation` section after the existing `## Search` section
  - Simple table linking to each doc with a one-line description
  - Do not duplicate content from the docs — just link to them

  **Test expectation:** none — pure documentation

  **Verification:**
  - All four docs are linked from the README
  - Links use relative paths (`docs/pattern-authoring-guide.md`)

## System-Wide Impact

- **Interaction graph:** None — pure documentation, no code changes
- **Error propagation:** N/A
- **State lifecycle risks:** None
- **API surface parity:** N/A
- **Unchanged invariants:** All existing code, tests, and behavior are unaffected. The four docs
  describe current behavior; they do not prescribe changes

## Risks & Dependencies

| Risk                                                   | Mitigation                                                                                |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| Docs describe behavior that changes in a future PR     | Each doc references source code locations. Update docs when those files change            |
| Real pattern examples reference the lore-patterns repo | Examples are self-contained quotes, not links. They work without access to the other repo |
| Docs become stale as the search pipeline evolves       | The search mechanics reference explicitly notes it describes current behavior             |

## Sources & References

- **Origin document:**
  [docs/brainstorms/2026-04-04-pattern-authoring-guide-requirements.md](docs/brainstorms/2026-04-04-pattern-authoring-guide-requirements.md)
- Dogfooding findings:
  [docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md](docs/plans/2026-04-03-002-fix-dogfooding-deferred-plan.md)
- Real patterns: lore-patterns repo (15 patterns across 7 directories)
- Institutional learnings: 7 relevant files in `docs/solutions/`
