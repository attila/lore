---
date: 2026-04-04
topic: lore-product-documentation
---

# Lore Product Documentation

## Problem Frame

Lore's README covers installation, CLI commands, and basic usage. But there is no guidance beyond
that for users who want to use lore effectively. Dogfooding revealed that **patterns can exist, be
discoverable via search, and still fail to change agent behavior** — a problem that better
documentation directly addresses. Users also have no reference for the hook injection pipeline, the
configuration options, or how to debug discoverability issues.

The ROADMAP explicitly defers the pattern authoring guide until "based on dogfooding evidence, not
speculation." We now have that evidence from PR #19 dogfooding, the `gh pr edit --body` incident,
and search pipeline investigation. This is the right time to ship the full documentation surface.

## Evidence Base

Investigation of all 15 patterns in a real knowledge base plus search pipeline analysis produced
these findings:

**Injection gaps (pattern not surfaced):**

| Scenario                                   | Why it failed                                                                  |
| ------------------------------------------ | ------------------------------------------------------------------------------ |
| `gh pr edit --body` blocked by permissions | Pattern says "creating PRs" but not "editing" — one missing verb = zero recall |
| `just ci` not run before commit            | "just" is a stop word, "ci" is 2 chars — produces zero query terms             |
| "merge pull request owner"                 | No pattern mentions merge ownership at all                                     |

**Compliance gaps (pattern surfaced but not followed):**

| Scenario                          | Why it failed                                                                      |
| --------------------------------- | ---------------------------------------------------------------------------------- |
| `gh pr edit --body` (if injected) | Pattern uses suggestive "use X instead of Y" rather than prohibitive "NEVER use Y" |

**Effective pattern traits (from patterns that DO work):**

- Imperative voice: "NEVER use `--body` with heredocs" vs "use `--body-file` instead"
- Incident grounding: "Why: discovered on 2026-03-12 when all links were backwards"
- Vocabulary breadth: terms agents actually search for appear in body/tags
- Concrete examples: good/bad pairs, code snippets, exact commands

**Ineffective pattern traits (from patterns that fail):**

- Passive voice describing what exists ("dprint is the single formatter")
- Missing behavioral mandates ("says what `just ci` does but not 'always run it'")
- Narrow vocabulary (only mentions "creating" not "editing/updating")
- No incident context (no "why this rule exists")

## Documentation Surface

Four documents, ordered by priority. The README remains the entry point; these docs are linked from
it and from each other where relevant.

| Doc                        | Path                              | Audience    | Purpose                                                |
| -------------------------- | --------------------------------- | ----------- | ------------------------------------------------------ |
| Pattern Authoring Guide    | `docs/pattern-authoring-guide.md` | All users   | How to write patterns that agents actually follow      |
| Search Mechanics Reference | `docs/search-mechanics.md`        | Power users | Full pipeline internals for debugging discoverability  |
| Agent Integration Guide    | `docs/agent-integration.md`       | All users   | How patterns reach the agent: hooks, lifecycle, tuning |
| Configuration Reference    | `docs/configuration.md`           | All users   | `lore.toml` options, env vars, XDG paths               |

## Requirements

**Pattern Authoring Guide (`docs/pattern-authoring-guide.md`)**

- R1. Product documentation for all lore users, grounded in real dogfooding evidence
- R2. Brief search mechanics inline — enough for authors to understand why vocabulary matters. Link
  to `docs/search-mechanics.md` for the full pipeline
- R3. Use real before/after pattern examples from the evidence base, not hypothetical ones
- R4. Cover three traits of effective patterns: imperative voice, incident grounding, and
  discoverable vocabulary. Present these as tools to reach for when they apply, not a mandatory
  checklist — not every pattern originates from a failure, and forcing incident context where none
  exists produces artificial content
- R5. Include an anti-patterns section showing common failure modes with concrete examples
- R6. Provide a manual review checklist (markdown) authors can run against a draft pattern before
  committing
- R7. Explain the stop word list and minimum term length (3 chars) so authors know which terms are
  invisible to search
- R8. Explain the hook query extraction pipeline at a high level: file extension -> language anchor,
  bash command -> term extraction, transcript tail -> context enrichment
- R9. Include a "vocabulary coverage" technique: for each pattern, list the verbs and nouns an agent
  might use when the pattern should surface, then verify those terms appear in the body or tags
- R10. Address the tag strategy: when tags help (discoverability for terms not in body), when they
  don't (duplicating body terms), and the tag-as-indexed-column mechanic
- R11. Document `LORE_DEBUG=1` as the primary debugging technique for authors to trace whether their
  pattern surfaces during real hook invocations

**Search Mechanics Reference (`docs/search-mechanics.md`)**

- R12. Cover the full pipeline: query construction (language anchor + OR enrichment), FTS5 column
  weights (title > tags > body), porter stemming, RRF score normalization, min_relevance threshold,
  dedup, sibling chunk expansion
- R13. Include worked examples showing how a real tool call becomes a query becomes search results
  becomes injected context
- R14. Document the stop word list, term cleaning rules, and hex-like filtering with exact
  thresholds

**Agent Integration Guide (`docs/agent-integration.md`)**

- R15. Explain the hook lifecycle: SessionStart (priming), PreToolUse (pattern injection),
  PostToolUse (error-driven injection), PostCompact (re-priming after context compression)
- R16. Document the Claude Code plugin setup beyond the README's 3-line snippet — what each hook
  event does, what `additionalContext` vs `systemMessage` output means, and how dedup prevents
  re-injection
- R17. Cover the query extraction pipeline from the agent's perspective: what signals the hook reads
  (file path, bash command, tool name, transcript tail) and how they map to search queries
- R18. Explain how to tune injection behavior: `min_relevance` threshold, `top_k`, `hybrid` mode,
  and when to use `--force` re-ingest after pattern changes

**Configuration Reference (`docs/configuration.md`)**

- R19. Document all `lore.toml` fields with defaults, types, and examples: `knowledge_dir`,
  `database`, `bind`, `ollama.host`, `ollama.model`, `search.hybrid`, `search.top_k`,
  `search.min_relevance`, `chunking.strategy`, `chunking.max_tokens`, `git.inbox_branch_prefix`
- R20. Document environment variables: `LORE_DEBUG` (verbose logging), `XDG_CONFIG_HOME`,
  `XDG_DATA_HOME`, and their fallback behavior
- R21. Document XDG path resolution: where config and data live by default, how to override, and the
  relationship between `lore init` output and the actual paths

## Success Criteria

- A new lore user can read the pattern authoring guide and write a pattern that surfaces via hook
  injection for its intended use cases
- The guide explains why existing weak patterns fail, with concrete evidence
- An experienced user can use the search mechanics reference to debug discoverability issues
- The checklist catches the specific failure modes found during dogfooding (missing verbs, stop word
  terms, passive voice)
- A user setting up lore with an agent can follow the integration guide without referring to source
  code
- All `lore.toml` options are documented with their defaults and purpose

## Scope Boundaries

- These docs do NOT replace the README — they complement it. The README remains the landing page
  with install/quickstart
- The docs do NOT modify existing patterns (that's separate work in the knowledge repo)
- The search mechanics doc is additive reference material for power users — the authoring guide is
  self-contained without it
- No changes to lore's codebase — this is pure documentation
- Search mechanics inline in the authoring guide is authoring context, not CLI documentation

## Key Decisions

- **Real examples over hypothetical:** Every example in the authoring guide comes from actual
  dogfooding evidence
- **Progressive disclosure:** Brief mechanics inline in the authoring guide, full reference
  separate. Integration guide explains hooks from the user's perspective, search mechanics explains
  the engine from the power user's perspective
- **Evidence-first investigation:** We ran LORE_DEBUG-equivalent analysis before writing
  requirements, so recommendations are grounded in pipeline behavior, not guesses
- **Four docs, not one:** Each doc serves a distinct audience and purpose. A user writing patterns
  doesn't need to read about `lore.toml` options; a user configuring lore doesn't need the
  anti-patterns section

## Dependencies / Assumptions

- Porter stemming (PR #22) is merged and affects vocabulary recommendations
- Security hardening (PR #23) is merged and affects transcript/dedup documentation
- The stop word list and query extraction logic are stable (no planned changes)
- The search mechanics doc describes current behavior — it will need updating if the pipeline
  changes
- The README will need minor updates to link to the new docs

## Outstanding Questions

### Deferred to Planning

- [Affects R2][Technical] Exact heading structure for the authoring guide — tutorial flow or
  reference structure?
- [Affects R13][Needs research] Which worked examples are most illustrative? Candidates: Edit .rs
  file, Bash `gh pr create`, Bash `cargo clippy`, PostToolUse error query
- [Affects R6][Technical] Should the checklist be inline in the guide or a separate section at the
  end?
- [Affects all][Technical] Should docs cross-link with `[text](path)` relative links or just mention
  the path?
- [Affects R15-R18][Needs research] Review the actual Claude Code plugin config at
  `integrations/claude-code/` to ensure the integration guide matches the shipped plugin

## Next Steps

-> `/ce:plan` for structured implementation planning
