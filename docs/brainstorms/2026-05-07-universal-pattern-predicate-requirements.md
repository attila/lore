---
date: 2026-05-07
topic: universal-pattern-predicate
---

# Universal-Pattern Predicate (`applies_when`)

## Summary

Add an optional `applies_when` predicate to universal-tagged patterns that gates re-injection by
tool class and Bash command prefix, plus a complementary universal-specific score-floor knob
defaulted to the current threshold so behaviour is unchanged without explicit config. Universal
patterns without the predicate continue firing on every relevant tool call as today.

---

## Problem Frame

Dogfooding lore against the `lore` repo itself surfaces a context-pollution failure mode:
`workflows/git-branch-pr.md` is tagged `universal`, so it bypasses the read-side dedup at
`src/hook.rs:700` and re-injects on every `PreToolUse:Bash` call — including `ls`, `wc -l`, and
`grep`, where the pattern's git-and-PR conventions have no relevance. The pattern continues to score
above the existing `min_relevance` floor on these calls because its body's broad workflow vocabulary
(terms like `git`, `commit`, `branch`, `push`) is wide enough to match weakly-related FTS5 queries
derived from non-git Bash commands.

The dedup bypass itself is intentional and load-bearing: the prior
`docs/brainstorms/2026-04-20-universal-patterns-requirements.md` shipped it precisely so
process-level conventions (commit/push discipline, PR etiquette) re-fire next to the call that
actually fails, instead of scrolling out of attention after their first injection. Removing the
bypass would re-introduce the discoverability gap that motivated universal patterns in the first
place.

The cost shape: each over-fire injects ~600-1000 tokens of irrelevant guidance into the agent's
context. Across a 100-call session, a single mis-targeting universal pattern is the dominant noise
source visible in `LORE_DEBUG` traces. Pattern authors today face a binary choice — tag `universal`
and accept the noise, or de-universalise and lose always-on visibility.

---

## Requirements

**Predicate field**

- R1. Pattern authors MAY add an optional `applies_when` block to the frontmatter of a
  universal-tagged pattern. When present, the predicate gates whether the pattern re-injects on a
  given `PreToolUse` call. When absent, the pattern fires as today.
- R2. The predicate supports two keys in this iteration: `applies_when.tools` (a list of tool-class
  names such as `Bash`, `Edit`, `Write`) and `applies_when.bash_command_starts_with` (a list of Bash
  command tokens such as `git`, `gh`).
- R3. Predicate semantics: OR within each list (allowlist — any value matches), AND across keys
  (every set key must hold for the pattern to fire). Combining `tools: [Bash]` with
  `bash_command_starts_with: [git, gh]` requires the call to be a Bash tool AND the command to start
  with `git` or `gh`.
- R4. `applies_when.tools` matches when the current tool name is in the list; non-listed tools fail
  the condition.
- R5. `applies_when.bash_command_starts_with` matches when the call is a Bash tool AND the command
  starts with one of the listed tokens. Non-Bash calls fail the condition, so the key implicitly
  scopes to Bash. The matcher walks past `sudo` and `env KEY=VAL` wrapper tokens before comparing
  the prefix.

**Threshold knob**

- R6. Track 1 ships a `min_relevance_universal` config knob — a per-tier score floor for
  universal-tagged patterns. The default value equals the existing `min_relevance` threshold so
  adopting Track 1 introduces no behaviour change without explicit config. Track 2 instrumentation
  will inform the tuned value when it lands.

**Schema reservation and forward compatibility**

- R7. The `applies_when` namespace is reserved for future Track 2-B extensions:
  `applies_when.languages` and `applies_when.environments` for non-universal patterns. Track 1
  documents these as forward-declared keys but does not implement them.
- R8. Predicate parsing accepts the `applies_when` block syntactically on any pattern, but Track 1's
  predicate evaluator only runs it for universal-tagged chunks. Track 2-B later extends the
  evaluator to non-universal patterns.

**Failure modes and observability**

- R9. Malformed `applies_when` at ingest (typo'd top-level key, wrong value type, unknown nested
  key) emits a warning to stderr naming the file and the malformed key/value. Ingest succeeds; the
  pattern is treated as if `applies_when` were absent. This is skip-with-warning — neither silent
  fail-open nor ingest-rejection.
- R10. When the predicate suppresses a candidate fire, `LORE_DEBUG` records a single line naming the
  pattern source, the tool name, and the command head (truncated). Predicate-level suppression
  logging is distinct from Track 2's score-level fire-rate aggregation.

**Backwards compatibility**

- R11. Existing universal patterns without `applies_when` continue firing on every relevant tool
  call. No migration is forced. Pattern-side migration of `workflows/git-branch-pr.md` and
  `agents/unattended-work.md` happens as follow-up commits in the patterns repo, after Track 1's
  engine PR lands.

---

## Acceptance Examples

- AE1. **Covers R5.** Given `workflows/git-branch-pr.md` with
  `applies_when.bash_command_starts_with: [git, gh]`, when a Bash call runs `sudo git status`, the
  pattern injects (matcher walks past `sudo`).
- AE2. **Covers R5, R10.** Given the same pattern, when a Bash call runs `ls`, the pattern is
  suppressed (no `git`/`gh` prefix after wrapper-stripping); `LORE_DEBUG` records a single
  predicate-suppression line naming the pattern, tool, and command head.
- AE3. **Covers R3, R4.** Given a pattern with `applies_when.tools: [Bash]` only (no
  command-prefix), when an `Edit foo.rs` call fires the pattern is suppressed; on any `Bash` call
  (including `Bash ls`) the pattern fires.
- AE4. **Covers R3.** Given a pattern with `applies_when.tools: [Bash]` AND
  `applies_when.bash_command_starts_with: [git, gh]`, on `Bash ls` the pattern is suppressed
  (command-prefix fails); on `Bash git push` it fires (both keys hold); on `Edit foo.rs` it is
  suppressed (`tools` fails).
- AE5. **Covers R9.** Given a pattern frontmatter with `appliess_when:` (typo'd top-level key) and a
  nested key, ingest succeeds, emits a warning naming the file and unknown key, and the pattern
  fires as if no predicate were set.
- AE6. **Covers R6.** Given the default `min_relevance_universal` value (equal to existing
  `min_relevance`), Track 1 adoption alone introduces no firing-behaviour change for any pattern;
  setting `min_relevance_universal` higher reduces fires of universal patterns whose FTS scores fall
  in the (default, new-floor) range without affecting non-universal patterns.

---

## Success Criteria

- In a fresh dogfooding session that runs `Bash ls`, `Bash wc -l`, `Bash grep` after migrating
  `workflows/git-branch-pr.md` to use `applies_when.bash_command_starts_with: [git, gh]`: the
  pattern is NOT in `additionalContext` for any of those calls (verifiable via `LORE_DEBUG`); the
  next `Bash git push` call DOES include the pattern.
- A typo'd `applies_when` block emits a clear warning at ingest, ingest completes, and the pattern
  fires as if no predicate were set. Ingest never rejects on malformed predicate.
- `min_relevance_universal` defaults match the current `min_relevance`, so an upgrade to Track 1
  without config edits produces zero observable behaviour change for any session that has not
  migrated patterns to use `applies_when`.
- Track 2's instrumentation, when it lands, can read predicate-level suppression entries from
  `LORE_DEBUG` to inform threshold tuning for unmigrated universals.
- Implementer-handoff signal: `ce-plan` does not need to invent the predicate field structure,
  evaluation site, malformed-predicate policy, or threshold-knob default behaviour from this doc.

---

## Scope Boundaries

- Regex predicates (e.g. `bash_command_match`) — deferred until a real pattern needs more than
  smart-prefix matching.
- Path-glob, language, and environment predicates for non-universal patterns — Track 2-B.
- Score-level instrumentation, fire-rate aggregation, suppressed-fire score histograms — Track 2.
- The actual tuned value of `min_relevance_universal` — Track 2 data informs it; Track 1 ships only
  the knob with the no-behaviour-change default.
- Keyword/embedding bleed for non-universal first-fires — separate development track parked on the
  macOS dogfooding box.
- Hard cap on number of universal patterns — remains a soft warning per the prior
  `docs/brainstorms/2026-04-20-universal-patterns-requirements.md`.
- Per-session or per-project overrides of universal status — out of scope per the prior brainstorm.
- Cycle-based dedup TTL — separate roadmap item under `ROADMAP.md`'s `Future` section.
- Pattern-side migration of existing universals to use `applies_when` — follow-up commits in the
  patterns repo, not part of Track 1's engine PR. Track 1's smoke test uses fixture patterns rather
  than depending on the pattern repo.

---

## Key Decisions

- **Predicate as primary primitive, threshold knob as complement.** Both ship together in Track 1.
  The predicate is primary because pattern body size confounds FTS scoring (a pattern's score grows
  with its vocabulary breadth, so threshold tuning can be defeated by pattern growth). The threshold
  knob captures the lexical-relevance axis; the predicate captures the categorical author-intent
  axis. They are not interchangeable, and shipping only one would leave a structural gap.
- **Smart prefix matching over literal prefix or regex.** Smart prefix walks past `sudo` and
  `env KEY=VAL` wrappers. Literal prefix breaks on common wrapper invocations; regex invites
  footguns in pattern frontmatter. Smart prefix matches realistic invocation patterns without
  surfacing regex syntax to authors.
- **`applies_when` evaluator scoped to universal patterns in Track 1.** The dogfooding case is
  universal-pattern noise; non-universal patterns rely on session dedup as today. Track 2-B later
  extends the evaluator to non-universal patterns with `path_glob`, `languages`, `environments`.
  Schema reservation in Track 1 prevents migration churn when Track 2-B lands.
- **Track 1 sequences before Track 2.** Predicate correctness comes from author intent, not score
  observation, so Track 1 ships and validates without Track 2's instrumentation in place. Track 2
  needs Track 1's signal-cleaning to interpret data correctly. Predicate-level suppression logging
  belongs in Track 1; score-level aggregation belongs in Track 2.
- **Skip-with-warning on malformed predicate, not fail-open or fail-closed.** Silent fail-open is
  dangerous: a typo turns a gated pattern into an ungated one that re-fires on every call.
  Fail-closed (reject ingest) is too strict: a typo blocks the entire pattern from being indexed.
  Skip-with-warning surfaces the typo at ingest while keeping the pattern available — it fires as if
  `applies_when` were absent (current pre-Track-1 behaviour) until the author fixes the typo.

---

## Dependencies / Assumptions

- The `tags: [..., universal]` opt-in surface from the prior
  `docs/brainstorms/2026-04-20-universal-patterns-requirements.md` is the existing parsing path;
  this doc assumes `applies_when` is added as a sibling frontmatter field, not nested under `tags`.
- The existing `PreToolUse` pipeline (search → sibling-expand → dedup → inject) is the stable
  integration shape; the predicate evaluator slots between sibling-expand and dedup.
- The dedup bypass for universal patterns and the `min_relevance` gate are load-bearing pre-existing
  behaviours from the prior universal-patterns work; the predicate complements them rather than
  replacing either.
- Track 2 (instrumentation, threshold tuning, non-universal predicate extensions) is planned but
  unscheduled. Track 1 must ship usefully even if Track 2 slips.

---

## Outstanding Questions

### Resolve Before Planning

(none)

### Deferred to Planning

- [Affects R2][Technical] Where does the parsed `applies_when` predicate live on the chunk row —
  dedicated columns, JSON field alongside tags, or a separate sidecar table? Persistence shape
  affects DB migration and search-result deserialisation but is invisible at the brainstorm layer.
- [Affects R5][Technical] Exact tokenisation rule for `sudo` and `env KEY=VAL` wrapper-stripping —
  edge cases like `env -u VAR`, `env -i`, multiple env-prefix tokens, or quoted values need a small
  spec during planning. The common cases (single `sudo`, single `env KEY=VAL`) are invariant.
- [Affects R6][Needs research] Whether `min_relevance_universal` lives in `lore.toml` config
  alongside the existing `min_relevance`, or as a per-pattern threshold in frontmatter. Config-level
  is simpler and matches the existing knob; per-pattern would let authors of large universal
  patterns tune individually. Track 2 instrumentation may inform this.

---

## Next Steps

→ `/ce-plan` for structured implementation planning.
