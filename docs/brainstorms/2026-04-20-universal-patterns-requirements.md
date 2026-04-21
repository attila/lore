---
date: 2026-04-20
topic: universal-patterns
---

# Universal Patterns via Tag-Based SessionStart Injection

## Problem Frame

Process-level conventions (commit rules, push discipline, branch naming, PR etiquette) suffer from a
discoverability gap that file-edit hooks cannot reliably close. Even when these patterns are
correctly indexed, ranked, and injected on the first relevant tool call of a session, the existing
dedup mechanism then suppresses them on every subsequent call within that session — including the
one that actually fails.

The motivating incident is captured verbatim in `ROADMAP.md` lines 10-19: during the `.loreignore`
work an agent ran a plain `git push`, hit `main`-protection rejection, and only then realised the
`workflows/git-branch-pr.md` "Pushing" section already prescribed `git push origin HEAD`. The
pattern was discoverable (relevance 1.0), the hook injected it on the first git command of the
session, and session deduplication then correctly suppressed it on every subsequent git call —
including the failing push.

The shipped coverage-check skill (PR #32) addresses tool-call-driven discoverability via
`PreToolUse`. Universal patterns address the orthogonal half: always-on discoverability for
conventions whose value comes from continuous reinforcement, not one-shot injection.

## Requirements

- **R1.** A pattern author opts into the always-on tier by adding `universal` to the `tags:`
  frontmatter list of the pattern. No other authoring step is required.
- **R2.** At `SessionStart`, every universal-tagged pattern's full body is emitted in a dedicated
  `## Pinned conventions` section that appears at the top of the SessionStart payload, above the
  existing pattern title index. The body is rendered the same way the `PreToolUse` hook renders
  pattern bodies today.
- **R3.** At `PostCompact`, the same `## Pinned conventions` section is re-emitted, since
  `PostCompact` already re-runs the SessionStart content pipeline. No additional code path is
  required for this — it falls out of R2 if the implementation goes through
  `format_session_context`.
- **R4.** At `PreToolUse`, universal patterns that pass the normal search relevance gate (matched by
  `extract_query`, ranked above `config.search.min_relevance`) re-inject every time, bypassing the
  dedup filter. Universal patterns that do not pass relevance for the current tool call do not
  inject. The "right pattern at the right time" semantic is preserved; only the "you've seen this
  once, never again" filter is removed.
- **R5.** Universal patterns inject **additively** at `PreToolUse` time: they do not consume the
  existing `config.search.top_k` budget that determines how many non-universal results are returned.
  A tool call may inject up to `top_k + N_universal_matched` chunks.
- **R6.** Lore emits a soft warning at ingest time if more than three patterns carry the `universal`
  tag. The warning names the offending patterns and prompts the author to consider whether all of
  them genuinely need always-on visibility. Ingest still succeeds — the warning is advisory, not a
  hard cap.
- **R7.** The pattern authoring guide gains a new subsection (sibling to "Vocabulary Coverage
  Technique" and "Tag Strategy") titled "When to use the universal tag". It documents the criteria:
  the pattern is a process-level convention (workflow, commit, push, review etiquette), its value
  comes from continuous reinforcement rather than one-shot applicability, and it is small enough
  that re-injection on every relevant tool call is justifiable.

## Success Criteria

- A pattern tagged `universal` appears in full at the top of the SessionStart payload under
  `## Pinned conventions`, and re-appears at `PostCompact`.
- For the documented motivating incident: in a fresh session that runs `Bash git push` after several
  earlier `Bash git`-family calls, the `workflows/git-branch-pr.md` pattern body is present in the
  `additionalContext` block surrounding the failing call (verifiable via `LORE_DEBUG=1` traces), not
  suppressed by dedup.
- A non-relevant universal pattern (e.g. a `git`-tagged universal pattern on a `Bash cargo build`
  call where FTS extraction yields a rust-only query) does not inject — the relevance gate still
  applies.
- Ingest of a knowledge base with four or more `universal`-tagged patterns emits a clearly-worded
  warning to stderr naming the affected patterns, but completes successfully.

## Scope Boundaries

- **Out of scope: cycle-based dedup TTL** (already a separate `Future` roadmap item — re-inject any
  pattern after N tool call cycles since last injection). Universal is a distinct, simpler
  primitive: always-on for tagged patterns, never for the rest.
- **Out of scope: hard cap on the number of universal patterns.** R6 is a soft warning only. Authors
  who need many universal patterns retain that freedom; the warning exists to prompt deliberation.
- **Out of scope: a separate frontmatter field** (`universal: true`) as the opt-in mechanism. The
  `tags:` list is the opt-in surface (R1) — this keeps the authoring surface uniform with how every
  other pattern attribute is declared today. (Tag-vs-field is technically reversible later, so this
  is a default-choice decision, not a one-way door.)
- **Out of scope: per-session or per-project overrides** of which patterns count as universal.
  Universal status lives in the pattern frontmatter alone; no `lore.toml` knob, no session-level
  mute.
- **Out of scope: changes to coverage-check** to specifically test universal patterns.
  Coverage-check measures `PreToolUse` discoverability as before; universal-tagged patterns get
  measured the same way as any other pattern.

## Key Decisions

- **Cadence: SessionStart inject + bypass `PreToolUse` dedup, with the relevance gate intact.**
  Rationale: a SessionStart-only injection does not fix the motivating incident (the agent saw the
  pattern hours earlier and forgot). An always-inject-regardless-of-relevance approach pays the
  token cost on every tool call (potentially 50K+ tokens per 100-call session for a single
  ~500-token pattern). The relevance-gated re-injection variant fixes the documented failure while
  bounding token cost to "how often the pattern is genuinely relevant".
- **SessionStart format: dedicated `## Pinned conventions` section at the top.** Rationale: full
  bodies belong above the title index where agent attention is highest; the existing index stays
  compact and unchanged; the distinct header signals "always apply" to the agent and makes injected
  output easy to debug from the human side.
- **Budget interaction: additive, not counted against `top_k`.** Rationale: counting universal
  patterns against `top_k` would mean tagging just five patterns kills all other PreToolUse
  injection — the feature would become a denial-of-service vector against itself. Additive plus a
  soft warning at >3 tagged patterns keeps the design honest without requiring a hard limit.

## Dependencies / Assumptions

- Assumes the existing `PostCompact` handler re-runs `format_session_context` (verified at
  `src/hook.rs:268`). If a future refactor splits this, R3 requires the universal-pattern emission
  to be re-attached.
- Assumes the existing `PreToolUse` dedup pipeline is the only filter between search results and
  injection (verified at `src/hook.rs:201-231`). No other gating step needs a universal carve-out.

## Outstanding Questions

### Resolve Before Planning

(none)

### Deferred to Planning

- **[Affects R1][Technical]** Is filtering "patterns where `tags` contains `universal`" cheap enough
  as a SQL `LIKE`/regex query, or should the schema gain a dedicated boolean column for fast
  filtering? Current patterns are searched via FTS5; pinned-pattern emission needs a separate query
  path.
- **[Affects R2][Technical]** Where do universal pattern bodies live — fetched from the chunk table,
  or assembled by re-reading the pattern source file at SessionStart time? Re-reading is simpler but
  adds I/O per session start; chunk-table assembly mirrors how `PreToolUse` works.
- **[Affects R6][Needs research]** What's the right place for the ingest-time warning — the per-file
  ingest log, a final summary line, or both? Existing ingest output conventions need a quick review.
- **[Affects R4][Technical]** Implementation choice: filter universal chunks out of the dedup-write
  step (so they're never recorded as seen), or filter them in at the dedup-read step (so the read
  pretends they weren't seen). The two are equivalent in behaviour but differ in which downstream
  code path needs to be touched.

## Next Steps

→ `/ce:plan` for structured implementation planning
