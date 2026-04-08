---
title: "feat: Coverage Check Skill (lore:coverage-check) with structured search metadata"
type: feat
status: active
date: 2026-04-07
origin: docs/brainstorms/2026-04-07-pattern-qa-skill-requirements.md
---

# feat: Coverage Check Skill (`lore:coverage-check`) with structured search metadata

## Overview

Ship a new Claude Code skill — `lore:coverage-check` — that automates the manual Vocabulary Coverage
Technique from `docs/pattern-authoring-guide.md`. The skill reads a target pattern markdown file,
brainstorms 5–12 candidate queries, refreshes the local index via `lore ingest --file`, runs the
queries against `search_patterns` in parallel, reports coverage gaps with suggested edits, and
auto-iterates up to three cycles per invocation. v1 ships as a markdown skill paired with one
minimal Rust change to attach structured metadata to the existing `search_patterns` MCP handler so
the skill can consume rank/source/mode data without parsing prose.

The PR also adopts the lore plugin's bare-name skill naming convention by renaming `search-lore` →
`search` and updating the seven prose references plus a one-line note in the plugin README.

## Problem Frame

Pattern authors writing for lore have the manual Vocabulary Coverage Technique available in the
authoring guide today, but the loop is tedious enough that it gets skipped. Patterns ship
discoverable-in-theory and undiscoverable-in-practice. Single-file ingest (`lore ingest --file`,
shipped in PR #29) removed the commit-before-test friction; this skill removes the manual
orchestration friction by automating the brainstorm → ingest → search → report → edit loop in one
slash command.

The skill is named `coverage-check` rather than `pattern-qa` to reserve the QA slot in the product
surface for the deterministic-helper-backed version that earns it (see Followup 1 in the origin
document). v1 is a coverage check; the eventual quality gate is not.

(see origin: `docs/brainstorms/2026-04-07-pattern-qa-skill-requirements.md`)

## Requirements Trace

Each requirement in the origin brainstorm maps to one or more implementation units below. The trace
is exhaustive — no requirement is silently dropped.

- **R1, R1a, R1b** (invocation, pre-flight `knowledge_dir` check, `lore` on `PATH` check) — **Unit
  3** (skill prompt covers all three pre-flight steps).
- **R2** (single-file scope per invocation) — **Unit 3** (skill prompt enforces).
- **R3** (5–12 candidate queries with FTS5-friendly rubric) — **Unit 3** (skill prompt encodes the
  vocabulary rubric drawn from the short-hook-queries learning).
- **R4** (single-file ingest, halt on non-zero, `--force` policy with `.loreignore`
  detect-then-prompt) — **Unit 3** (skill prompt invokes `lore ingest --file <path>` first, prompts
  the author before retrying with `--force` if the file is excluded).
- **R5** (parallel `search_patterns` MCP, no Bash fallback, wait-for-all- settle, no per-query
  timeout) — **Unit 2** (Rust change makes the parallel-results path machine-readable) and **Unit
  3** (skill prompt invokes in parallel and waits).
- **R6** (four-state coverage report: surfaced/not_present/errored/ degraded; refuse coverage ratio
  if any query is degraded) — **Unit 2** (Rust change adds three-value
  `mode: "hybrid" |
  "fts_fallback" | "fts_only"` enum so MCP clients can distinguish full hybrid
  search from embedder-fallback and from configured FTS-only deployments) and **Unit 3** (skill
  prompt renders the four states and applies the refusal rule, with cascade detection at step 6
  catching `fts_only` and `fts_fallback` early).
- **R7** (concrete edit suggestions per gap) — **Unit 3** (skill prompt generates and applies
  suggestions).
- **R8** (auto-iteration with stability OR ceiling=3 termination, Bash-`diff`-driven mechanical
  comparison) — **Unit 3** (skill prompt encodes the iteration loop with temp-file diff for the
  stability check).
- **R9** (file-and-DB writes only, exit reminder for index pollution) — **Unit 3** (skill prompt
  prints the recovery hint on exit).
- **R10** (chat-ephemeral coverage report, JSONL log under `~/.cache/lore/qa-sessions/`, opt-out via
  `LORE_NO_QA_LOG`) — **Unit 3** (skill prompt instructs the agent to append the JSONL line with a
  pinned shell recipe).
- **R11** (`SKILL.md` description names the paraphrase-bias limit) — **Unit 3** (skill description
  in frontmatter and body).
- **Naming convention rename** — **Unit 1** (rename `search-lore` → `search`, update 7 prose
  references, plugin README convention note).

## Scope Boundaries

Carried forward from the origin document. The Bash-`diff`-via-temp-file stability check (R8) and the
cascade detection extra search call (Unit 3 step 8) are new Unit 3 mechanisms but do not expand
scope — they replace weaker mechanisms in the brainstorm.

- **Minimal Rust change scoped to one MCP handler.** Only Unit 2 touches Rust —
  `src/server.rs::handle_search` gains a `text_response_with_metadata` block. No new CLI subcommand,
  no new MCP tool, no other library code changes.
- **No persisted query metadata in the pattern repository.** No `qa_queries` frontmatter, no sidecar
  files in pattern repos. The local JSONL log lives outside any pattern repo and is a separate
  category.
- **No CI integration.** No `--check` mode, no GitHub Action.
- **No bias mitigations in v1.** Hidden-body generation, author seeds, and anti-queries are
  followups.
- **No cross-project invocation.** R1a's pre-flight refuses out-of-scope invocations with a clear
  message.
- **No production trace logging.** The local JSONL log captures only this skill's own invocations,
  not real PreToolUse query data.
- **No Bash fallback for search.** R5 commits to MCP-only.
- **Single Claude Code skill deliverable.** One file at
  `integrations/claude-code/skills/coverage-check/SKILL.md`. The Rust change is in `src/server.rs`
  and is not part of the skill directory.
- **No per-query timeout in v1.** R5 waits for all parallel queries to settle.
- **POSIX shell environment only.** v1 assumes a POSIX shell for the pre-flight checks, the JSONL
  log path computation, and the iteration-stability `diff`. Native Windows is out of scope; WSL
  works if `~/.cache/` resolves to a writable path inside the WSL user's home, with the caveat that
  the JSONL log lands in WSL's home rather than the Windows user's home.
- **No cross-invocation oscillation detection.** v1 is a producer for the JSONL log; reading the log
  to detect cross-invocation thrashing patterns is captured under Future Considerations and does not
  ship in v1.

## Context & Research

### Relevant Code and Patterns

- **Existing skill structure:** `integrations/claude-code/skills/search-lore/SKILL.md` — frontmatter
  has `name`, `description`, `disable-model-invocation: true`, `user-invocable: true`. Body is a
  single H1 followed by flowing paragraphs. The new `coverage-check/SKILL.md` mirrors the
  **frontmatter shape** exactly, but the body structure is intentionally richer than `search-lore`'s
  single-paragraph format because the coverage-check loop has 13 distinct steps that benefit from
  explicit numbered subsections. This divergence is deliberate and called out here so an implementer
  doesn't try to compress the skill back into search-lore's form.
- **`text_response_with_metadata` helper:** `src/server.rs:793-811`. Returns the same JSON-RPC shape
  as `text_response` plus a `metadata` sibling on `result`. Existing call sites (4):
  - `handle_add` at `src/server.rs:547` (metadata block at lines 541-546)
  - `handle_update` at `src/server.rs:614` (metadata block at lines 608-613)
  - `handle_append` at `src/server.rs:677` (metadata block at lines 671-676)
  - `handle_lore_status` at `src/server.rs:754` (metadata block at lines 713-722)
- **`handle_search` current return path:** `src/server.rs:386-491`. Builds prose `response` string
  from formatted results (lines 449-485) and returns `text_response(req, &response)` at line 487.
  The change is to build a `metadata` JSON value before line 487 and replace the `text_response`
  call with `text_response_with_metadata(req, &response, &metadata)`.
- **`embed_failed` internal flag:** Already exists at `src/server.rs:412` inside `handle_search`'s
  scope. Translating it to the `mode` enum is a one-liner in the metadata block.
- **`lore_status` exposes `knowledge_dir`:** `src/server.rs:714` puts
  `"knowledge_dir": ctx.config.knowledge_dir.display().to_string()` as the first field in the
  metadata object. The skill reads `result.metadata.knowledge_dir` for R1a's pre-flight.
- **MCP test harness:** `src/server.rs:840` opens the inline `#[cfg(test)]` module. `TestHarness` is
  at lines 853-898 — builds an in-memory `KnowledgeDB`, a `FakeEmbedder` (4-dim), and exposes
  `request_value()` to drive `handle_request` directly. The metadata-assertion pattern to copy is
  `lore_status_reports_non_git_state` at lines 1017-1055: assert `resp["error"].is_null()`, then
  read fields from `resp["result"]["metadata"]` by name, then assert prose-content invariants on
  `resp["result"]["content"][0]["text"]`.
- **`tools_list_returns_all_five_tools`** at `src/server.rs:927` (the `#[test]` line; the function
  body starts the next line) uses `insta::assert_json_snapshot!` and asserts `tools.len() == 5`.
  **This test must remain unchanged** by Unit 2 — see the file-scoped invariant in Unit 2's
  Approach.
- **`commit_status_metadata` helper** at `src/server.rs:819-825` demonstrates the project's
  exhaustive-match convention for enum-shaped metadata serialization (no wildcard arms, doc comment
  explaining why). The `mode` field follows the same exhaustive-match discipline.
- **`lore ingest --file --force` CLI surface:** Verified in `src/main.rs` (lines 74-82, 282-369) and
  `src/ingest.rs`. The `--force` flag works with `--file` to override `.loreignore` filters for the
  targeted file. Without `--force`, an `.loreignore`d file results in a silent skip with exit 0.
- **Single-file ingest contract** verified by
  `tests/single_file_ingest.rs::re_ingesting_same_file_replaces_chunks_without_duplication`. The
  coverage loop depends on full-replace semantics; this test pins the contract.
- **Plugin manifest** at `integrations/claude-code/.claude-plugin/plugin.json` declares
  `"skills": "./skills/"` — skills are auto-discovered by directory name. **The rename does not
  require plugin.json changes.**
- **No state-directory helper exists** for `~/.cache/lore/`. Existing XDG helpers are
  `default_config_path()` (`src/config.rs:114`) and `default_database_path()` (`src/config.rs:125`)
  using `resolve_xdg_base()` at `src/config.rs:93`. The skill pins a concrete shell recipe for
  resolving and creating the cache path (no Rust helper for v1) — see Unit 3 step 10 for the recipe.
- **No `integrations/claude-code/README.md` exists** today, nor
  `integrations/claude-code/.claude-plugin/README.md`. Unit 1 must create the appropriate one
  alongside the plugin manifest to host the convention note.

### Institutional Learnings

- **Composition cascade hazard** —
  `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`.
  The skill calls `lore ingest --file` in a loop on what is typically an uncommitted draft. If the
  author has previously `git rm`'d the file, a subsequent plain `lore ingest` (delta) will silently
  wipe the chunks the skill upserted. **The skill prompt includes a hazard callout warning the
  author never to run plain `lore ingest` between iterations — only `lore ingest --file`.** Beyond
  the prose warning, Unit 3 step 8 adds an active detection: between the ingest and the main search
  batch, the skill issues one extra `search_patterns` call with a token known to be in the file body
  and verifies the target's `source_file` appears in the response. If it does not, the skill aborts
  the iteration with "index state changed unexpectedly — check for parallel `lore ingest` processes"
  rather than producing a misleading coverage report.
- **MCP `text_response_with_metadata` contract pattern** —
  `docs/solutions/best-practices/mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md`. Key
  constraints: (a) the `metadata` sibling on `result` is purely additive — existing consumers
  reading `content[0].text` keep working, so this is **not** a breaking change; (b) use a
  tagged-union shape (`{"kind": "..."}`) for any conditional state; (c) if the metadata renders an
  enum, use exhaustive `match` with no wildcard arm so adding a variant fails to compile; (d) **pin
  the metadata contract with a test that asserts on `result["metadata"]` directly**, not on the text
  content.
- **Pre-flight via `lore_status` validates the design** — same file as above, "Companion: a status
  tool for pre-flight checks" section. Frames `lore_status` as the _plan_ surface and per-tool
  metadata as the _verify_ surface. The skill should read `metadata.knowledge_dir` from
  `lore_status` (not the prose text) and confirm containment client-side.
- **CLI prose parsing is fragile, prefer structured output** —
  `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md` and
  `docs/solutions/best-practices/cli-suppress-stderr-in-json-mode-2026-04-03.md`. The skill calls
  `lore ingest --file` via Bash. **Rely on exit code only — do not parse the prose stderr summary.**
  Adding `--json` to ingest is a follow-up; v1 treats any non-zero exit as `errored` and halts.
- **MCP server does not pick up new binaries on `/plugin reload`** —
  `docs/solutions/integration-issues/reload-plugins-does-not-restart-mcp-servers-2026-04-03.md`.
  After landing the `handle_search` metadata change, validating the skill's metadata-shape
  consumption requires a `/exit` + relaunch step in the test plan, not just `just install`.
- **Plugin assembly conventions** —
  `docs/solutions/integration-issues/claude-code-plugin-assembly-pitfalls-2026-04-02.md`. Skill
  directories live under `integrations/claude-code/skills/<name>/SKILL.md`, the bare directory name
  becomes the slash command. Confirms that `coverage-check` (kebab-case) is the discoverable name
  and that the rename of `search-lore/` → `search/` is mechanically correct.
- **Vocabulary brainstorming must favor FTS5 keyword overlap** —
  `docs/solutions/best-practices/short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md`
  and `docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md`.
  Candidate queries the skill brainstorms must mimic real hook queries: 3+ character alphabetic
  terms, 3–5 terms each, no stop-words, include synonym verbs (edit/update/create). **Never test
  with command names like `just ci`, `go run`, `npm run` — they produce zero queryable terms after
  FTS5 cleaning and would always return `not_present` regardless of pattern content.** The skill
  prompt encodes this rubric.
- **Filter changes need bidirectional reconciliation** —
  `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`.
  Indirect: confirms the skill should not assume re-running `lore ingest --file` necessarily
  refreshes the index in all cases. The full-replace contract from
  `re_ingesting_same_file_replaces_chunks_without_duplication` is the v1 dependency we pin.

### External References

External research skipped — local patterns are strong (the existing `text_response_with_metadata`
call sites are the canonical reference) and the institutional learnings cover every novel surface.

## Key Technical Decisions

- **Three deliverables, one PR.** The Rust change to `handle_search` (Unit 2), the new
  `coverage-check` skill (Unit 3), and the `search-lore` → `search` rename (Unit 1) ship together.
  Splitting them would force the skill to either land without the metadata it depends on or to ship
  a fragile prose-parsing variant that gets thrown away.
- **`handle_search` metadata follows the exact `handle_lore_status` pattern.** Use
  `serde_json::json!` macro inline (not a typed struct), put the `mode` field at the top level, and
  put per-result fields in a `results: [...]` array. Mirror the field-naming conventions already in
  the file.
- **Search metadata fields:**
  - Top level: `mode` (three-value enum, see below), `query`, `top_k`, `result_count`.
  - Per result row in `results`: `rank` (1-indexed integer matching the existing `[N]` prose
    prefix), `title`, `source_file`, `score`. Other fields (`tags`, `body_excerpt`) are nice-to-have
    but not required by the skill.
  - The `mode` field is the load-bearing one. It distinguishes three orthogonal cases that the prose
    response body cannot expose:
    - `"hybrid"`: full hybrid search (Ollama embedder + FTS combined via reciprocal-rank fusion).
      Scores are comparable across queries; aggregate coverage metrics are valid.
    - `"fts_fallback"`: hybrid was attempted but the embedder was unreachable; silently fell back to
      FTS-only for this query. Scores use BM25 rank, not comparable to `"hybrid"`.
    - `"fts_only"`: deployment is configured for FTS-only via `config.search.hybrid = false`. The
      embedder was never attempted. Scores use BM25 rank, not comparable to `"hybrid"`.
  - The mode value is derived from `(config.search.hybrid, embed_failed)` via a Rust enum
    `SearchMode` whose `as_str` match is intentionally exhaustive (no wildcard arm) so adding a new
    variant fails to compile until the JSON serialisation is updated.
  - **`embed_failed` is NOT exposed.** The previous design exposed `embed_failed: bool` alongside
    `mode`, but it was strictly derivable (`true` iff `mode == "fts_fallback"`) and the redundancy
    created a maintenance hazard for clients that branched on either field. Clients read `mode`
    exclusively.
- **No new metadata helper.** The metadata block is built inline in `handle_search` using
  `serde_json::json!`, matching the convention of the four existing call sites. A
  `search_results_metadata()` helper would be premature abstraction for one call site.
- **`.loreignore` policy: detect-then-prompt, not blanket `--force=true`.** Resolves the
  brainstorm's deferred R4 question. The skill first runs `lore ingest --file <path>` _without_
  `--force`. If the file is silently skipped because of `.loreignore` (the path the brainstorm
  flagged where R4's halt clause cannot fire), the skill detects this by re-querying the index for
  the target's `source_file` count and finding zero chunks. On detection, the skill prompts the
  author: _"`<path>` is excluded by `.loreignore`. Run with `--force` to bypass the exclusion for
  this coverage check? (y/N)"_. On `y`, the skill re-runs `lore ingest --file --force <path>`. On
  `N`, the skill exits cleanly. Rationale for the prompt-first approach over the brainstorm's
  blanket `--force=true`: an author who put a draft in `.loreignore` has prior intent that should be
  acknowledged, not overridden silently. The cost of the prompt is one author keystroke; the cost of
  silent override is polluting the index with content the author specifically parked.
- **Skill frontmatter:** `name: coverage-check`, `description: <one-line>`,
  `disable-model-invocation: true`, `user-invocable: true`. Mirrors the existing `search-lore`
  (post-rename: `search`) convention. The skill is human-invoked, not auto-fired by the agent.
- **Candidate-query brainstorm rubric (R3):** the skill prompt instructs the agent to produce 5–12
  queries each meeting the FTS5 rubric: ≥3 alphabetic characters per token, no stop-words (`the`,
  `a`, `is`, `to`, …), 3–5 tokens per query, mix of action verbs (with synonyms:
  edit/update/create), domain nouns, and abbreviations the pattern contains. The prompt cites the
  short-hook-queries learning by name so the rubric is auditable. Queries containing only command
  names (`just`, `npm`, `cargo`) are explicitly disallowed because they produce zero queryable terms
  after cleaning.
- **R1a canonicalisation: pinned shell recipe.** The skill computes the canonical target path with a
  single concrete shell command instead of leaving the method to the agent's judgment:
  - Linux: `readlink -f <target>`
  - macOS / BSD (no GNU `readlink`):
    `python3 -c "import os, sys;
    print(os.path.realpath(sys.argv[1]))" <target>` as a portable
    fallback that does not depend on GNU coreutils
  - The skill detects the platform via `uname` once at start and picks the right command
  - Containment is then a string-prefix check: the canonical target must start with the canonical
    `knowledge_dir` followed by `/`. R1a is framed as a fast-path heuristic; R4's
    `validate_within_dir` is the authoritative check. The pinned recipe guarantees R1a is at least
    deterministic across sessions and machines.
- **R8 stability comparison: Bash `diff` against a temp file, not model self-comparison.** The
  brainstorm initially specified that the agent would render each iteration's surfaced-query set as
  a sorted code-fence and compare its own prior chat output. This relies on model behaviour (context
  retention, deterministic rendering, accurate string comparison) that adversarial review correctly
  flagged as unreliable — chat compaction can summarise the prior list, whitespace can drift, and
  the model can hallucinate equality.

  The replacement: each iteration writes its sorted surfaced-query list to a temp file under
  `~/.cache/lore/coverage-check/<session>-iter-N.txt`. On the next iteration, the skill issues
  `diff ~/.cache/lore/coverage-check/<session>-iter-{N-1}.txt
  ~/.cache/lore/coverage-check/<session>-iter-N.txt`
  via Bash. The `diff` exit code is the loop signal: 0 = stable (converged), non-zero = changed
  (continue if iteration < 3). This moves the comparison out of the model's head and into a
  deterministic shell tool. The temp files are cleaned up on skill exit (success or error). The
  session ID is the same `<timestamp>` used for the JSONL log so the diff temp files are colocated
  with the audit trail.
- **Cascade detection: one extra search call between ingest and the main search batch.** Between
  Unit 3 step 6 (ingest) and Unit 3 step 7 (parallel search), the skill issues one additional
  `search_patterns` call with a query constructed from a distinctive token in the pattern's body
  (the agent picks a unique-looking word that the FTS5 rubric would accept). It then verifies the
  target's `source_file` appears in `metadata.results`. If not, the chunks the skill just ingested
  are absent from the index — almost certainly because a parallel `lore ingest` (file watcher,
  second Claude Code session, pre-commit hook) wiped them. The skill aborts the iteration with:
  _"Index state changed unexpectedly between ingest and search. Check for parallel `lore ingest`
  processes (file watcher, pre-commit hook, second Claude Code session). The composition cascade
  documented in
  `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  may have fired."_. This converts a silent failure into a loud one for one extra MCP call per
  iteration.
- **R6 fts_fallback handling: refuse coverage ratio if ANY query is degraded.** The brainstorm
  originally said "compute the ratio over hybrid-mode queries only". Adversarial review correctly
  flagged this as false-precision: a 67% ratio computed over the 3 of 8 queries that happened to be
  hybrid is meaningless when the other 5 went fts_fallback. The replacement: if any query in an
  iteration returns `mode: fts_fallback`, the skill DOES NOT compute or display a coverage ratio for
  that iteration. Instead the report says _"Embedder was partially unavailable (`<N>` of `<M>`
  queries degraded). Coverage ratio is not computed because hybrid and FTS-only ranks are not
  comparable. Retry once Ollama is back."_. The author can still see which individual queries were
  `surfaced`, `not_present`, `errored`, or `degraded` — only the aggregate ratio is suppressed.
- **JSONL log path: pinned shell recipe with `mkdir -p`, nanosecond precision, PID suffix, abort on
  EACCES.** The skill prompt pins one concrete recipe instead of leaving path resolution to the
  agent:

  ```sh
  cache_root="${XDG_CACHE_HOME:-$HOME/.cache}/lore/qa-sessions"
  mkdir -p "$cache_root" || { echo "ERROR: cannot create $cache_root" >&2; exit 1; }
  log_file="$cache_root/$(date -u +%Y%m%dT%H%M%S%N)-$$.jsonl"
  ```

  - `${XDG_CACHE_HOME:-$HOME/.cache}` handles the unset-XDG case with a POSIX-standard fallback.
  - `mkdir -p` is idempotent and creates the directory tree on first invocation.
  - The `|| { ...; exit 1; }` aborts the iteration on EACCES (read-only NFS, tmpfs out of space,
    permission denied) instead of silently losing the log.
  - The filename combines nanosecond precision (`%N`) with the shell PID (`$$`) so two simultaneous
    Claude Code sessions on the same machine cannot collide.
  - On non-GNU `date` (macOS), `%N` falls back to literal `%N` — the skill instead uses
    `python3 -c "import time; print(int(time.time_ns()))"` on macOS, picked via the same `uname`
    detection as the R1a canonicalisation recipe.

  This replaces the brainstorm's "agent expands `$XDG_CACHE_HOME` and `~` itself" with a concrete
  recipe that has no agent-judgment surface. Windows is out of scope (see Scope Boundaries).
- **Hazard callout in SKILL.md.** Per the composition-cascade learning, the skill description
  includes a one-line warning: _while iterating with this skill, never run plain `lore ingest` —
  only `lore ingest --file`._ Cite the cascade learning by filename so the reader can find the
  underlying explanation.

## Open Questions

### Resolved During Planning

- **R3 prompt wording.** Resolved: the skill prompt encodes the FTS5 vocabulary rubric drawn from
  `short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md` and
  `common-tool-commands-produce-zero-queryable-terms-2026-04-05.md`. See the candidate-query
  brainstorm rubric in Key Technical Decisions.
- **R4 `--force` policy.** Resolved: detect-then-prompt. See Key Technical Decisions for the
  rationale.
- **JSONL log path resolution.** Resolved: pinned shell recipe with `mkdir -p`, nanosecond
  precision, PID suffix, abort on EACCES.
- **Search metadata field shape.** Resolved: top-level `mode`, `query`, `top_k`, `result_count`,
  `embed_failed`; per-row `rank`, `title`, `source_file`, `score`. See Key Technical Decisions.
- **R8 stability mechanism.** Resolved: Bash `diff` against temp file, not model self-comparison.
  See Key Technical Decisions.
- **R6 fts_fallback ratio handling.** Resolved: refuse to compute the coverage ratio if any query in
  the iteration is degraded.
- **R1a canonicalisation method.** Resolved: pinned `readlink -f` (Linux) / `python3 realpath`
  (macOS/BSD) recipe.
- **Cascade detection in v1.** Resolved: one extra search call between ingest and the main search
  batch, verifying the target's `source_file` appears in the response.

### Deferred to Implementation

- **Exact wording of the skill's instruction prompt.** The skill's prose body needs iteration
  against real patterns from `lore-patterns/` to confirm the candidate-query brainstorm produces
  high-quality results. The plan specifies the rubric and the structural sections; the implementer
  iterates the wording during the manual smoke test loop in Unit 3's verification.
- **`lore_status` MCP response field stability.** The plan reads `metadata.knowledge_dir` from the
  `lore_status` response. If the field name or path changes during implementation (e.g. someone
  reorganises metadata), the skill prompt must be updated to match. The
  `lore_status_reports_non_git_state` test pins the field name today.
- **macOS `date %N` and `readlink -f` portability.** The plan specifies `python3` fallbacks for
  both; the implementer verifies the fallbacks actually work on a macOS box during smoke testing and
  adjusts the pinned recipe if needed.

## High-Level Technical Design

> _This illustrates the intended approach and is directional guidance for review, not implementation
> specification. The implementing agent should treat it as context, not code to reproduce._

### Search response shape (Unit 2)

The change to `handle_search` is purely additive to the existing JSON-RPC response. Today's response
shape (prose only):

```json
{
  "jsonrpc": "2.0",
  "id": "...",
  "result": {
    "content": [
      { "type": "text", "text": "[1] Title (source: rust/foo.md)\n..." }
    ]
  }
}
```

After Unit 2, the same prose body remains in `content[0].text`, plus a new `metadata` sibling on
`result`:

```json
{
  "jsonrpc": "2.0",
  "id": "...",
  "result": {
    "content": [{ "type": "text", "text": "..." }],
    "metadata": {
      "query": "<the search query string>",
      "top_k": 5,
      "result_count": 3,
      "mode": "hybrid",
      "results": [
        {
          "rank": 1,
          "title": "Cargo Deny",
          "source_file": "rust/cargo-deny.md",
          "score": 0.876
        },
        {
          "rank": 2,
          "title": "...",
          "source_file": "...",
          "score": 0.654
        }
      ]
    }
  }
}
```

The `mode` field is a three-value enum:

- `"hybrid"`: full Ollama-embedder + FTS search via reciprocal-rank fusion. Comparable across
  queries.
- `"fts_fallback"`: hybrid was attempted but the embedder was unreachable for this query; silently
  fell back to FTS-only. Not comparable to `"hybrid"`.
- `"fts_only"`: deployment is configured for FTS-only via `config.search.hybrid = false`. The
  embedder was never attempted. Not comparable to `"hybrid"`.

The skill detects `"fts_fallback"` and `"fts_only"` at step 6 (cascade detection runs one extra
search call before the parallel batch and aborts the iteration on either non-hybrid value with a
clear next-action message). Step 9's coverage-ratio refusal is the fallback for the rare case where
the embedder fails partway through the parallel batch after passing cascade detection.

### Skill iteration loop (Unit 3)

```
PRE-FLIGHT
  ├─ command -v lore             → halt with R1b error if missing
  ├─ uname → pick canonicalise() and timestamp() commands
  ├─ lore_status                  → read metadata.knowledge_dir
  ├─ canonicalise(target_path)    → string-prefix containment check
  └─ exit with R1a error if outside knowledge_dir

LOOP (iteration_count = 1; max = 3; session_id = timestamp_with_pid)
  ├─ Read target file (full content)
  ├─ Brainstorm 5-12 candidate queries (apply FTS5 rubric)
  ├─ Show queries to author
  ├─ Bash: lore ingest --file <target>           [no --force initially]
  │   ├─ exit non-zero → halt with R4 error
  │   └─ exit 0 → check chunks_indexed for target source_file
  │              ├─ > 0 → proceed
  │              └─ == 0 → prompt: ".loreignore detected, retry with --force? (y/N)"
  │                       ├─ y → re-run with --force, halt if still 0 chunks
  │                       └─ N → exit cleanly
  ├─ Cascade detection: search_patterns(distinctive_token_from_body)
  │   ├─ target source_file in results → proceed
  │   └─ not in results → abort iteration with cascade-detected message
  ├─ Parallel: search_patterns × N (one per query)
  │   └─ wait for ALL to settle (no timeout)
  ├─ Per query, read result.metadata fields:
  │   ├─ JSON-RPC error          → state: errored
  │   ├─ mode == "fts_fallback" → state: degraded
  │   ├─ target source_file in any result row → state: surfaced (rank N)
  │   └─ otherwise               → state: not_present
  ├─ If ANY query is degraded:
  │   └─ Render report WITHOUT coverage ratio, with "embedder partially
  │      unavailable, retry" message; suggest no edits, exit
  ├─ Render coverage report:
  │   ├─ Surfaced queries (sorted list, written to
  │   │   ~/.cache/lore/coverage-check/<session>-iter-N.txt)
  │   ├─ Not present queries
  │   ├─ Errored queries
  │   └─ Coverage ratio (over hybrid-mode queries only — guaranteed all
  │       queries hybrid by the degraded short-circuit above)
  ├─ Append JSONL line via pinned shell recipe (skip if LORE_NO_QA_LOG=1)
  ├─ For each gap, propose concrete edit suggestion
  ├─ Ask author: accept / partial / skip
  ├─ If any edits applied:
  │   ├─ iteration_count += 1
  │   ├─ if iteration_count > 3 → exit with "ceiling reached" message
  │   ├─ else loop back to ingest, then:
  │   │   └─ at end of next iteration, run:
  │   │       diff ~/.cache/lore/coverage-check/<session>-iter-{N-1}.txt
  │   │            ~/.cache/lore/coverage-check/<session>-iter-N.txt
  │   │   ├─ exit 0 → "converged" exit
  │   │   └─ exit non-zero → continue loop
  └─ Else exit (no changes)

EXIT
  ├─ Clean up ~/.cache/lore/coverage-check/<session>-iter-*.txt
  └─ Print: "if you discarded any iterated edits via git checkout,
            run lore ingest to reconcile the index"
```

## Implementation Units

### - [ ] Unit 1: Rename `search-lore` → `search` and document the naming convention

**Goal:** Adopt the lore plugin's bare-name plus plugin-namespace skill naming convention
plugin-wide. Rename the existing `search-lore` skill to `search`, update all in-repo prose
references, and add a one-line convention note to the plugin README.

**Requirements:** Naming convention adoption (origin Key Decision).

**Dependencies:** None — independent of Units 2 and 3, can land first.

**Files:**

- Rename: `integrations/claude-code/skills/search-lore/` → `integrations/claude-code/skills/search/`
- Modify: `integrations/claude-code/skills/search/SKILL.md` (rename `name: search-lore` →
  `name: search` in frontmatter)
- Modify: `README.md` (two `/search-lore` references)
- Modify: `src/main.rs` (one `/search-lore` reference in an `eprintln!` string at line 261)
- Modify: `docs/todos/lore-status-discovery-asymmetry.md` (frontmatter files list + one prose
  mention)
- Modify: `docs/hook-pipeline-reference.md` (directory tree + one prose mention)
- Modify: `docs/plans/2026-04-01-005-feat-agent-integration-claude-code-plan.md` (three references —
  ASCII tree, file path, and skill-name mention)
- Modify: `docs/solutions/integration-issues/claude-code-plugin-assembly-pitfalls-2026-04-02.md`
  (directory tree and YAML example name)
- Create: `integrations/claude-code/.claude-plugin/README.md` (does not exist today; this file is
  the home for the convention note)

**Approach:**

- Directory rename is a `git mv` operation. Plugin manifest at
  `integrations/claude-code/.claude-plugin/plugin.json` does **not** change — skills are
  auto-discovered from `./skills/` by directory name.
- Prose updates are precise string replacements. The string `search-lore` is distinctive enough that
  text replacement is safe across all seven files. Both the slash-command form `/search-lore` and
  the bare directory form `search-lore/` should become `search` and `search/` respectively.
- The frontmatter `name:` field changes from `search-lore` to `search`.
- The plugin README convention note is one paragraph: "Skills inside this plugin are named by their
  function alone, with no `lore-` prefix. Claude Code's plugin namespace handles disambiguation as
  `/lore:<skill-name>`."
- This rename is a breaking change for anyone with muscle memory for `/lore:search-lore`, but the
  brainstorm explicitly accepts that cost on the grounds that the lore plugin has no released users
  yet.

**Patterns to follow:**

- Existing skill structure at `integrations/claude-code/skills/search-lore/SKILL.md` (will become
  `integrations/claude-code/skills/search/SKILL.md`).
- Plugin manifest convention at `integrations/claude-code/.claude-plugin/plugin.json`.

**Test scenarios:**

_Test expectation: none — pure refactor with no behavior change._

The verification is mechanical:

- `cargo test` still passes (no Rust code references the old skill name by string outside the one
  `eprintln!` in `src/main.rs`).
- `Grep search-lore .` in the repo returns zero matches after the rename, except in this plan file
  and the brainstorm file (which preserve the rename history as historical artifacts).
- A manual smoke test in Claude Code: `/lore:search foo` returns the same behavior as
  `/lore:search-lore foo` did pre-rename.

**Verification:**

- Repo grep for `search-lore` is empty outside historical artifact docs.
- The existing skill behavior is unchanged when invoked by its new name.
- Plugin loads cleanly (no warnings about missing or duplicate skills).

---

### - [ ] Unit 2: Add `text_response_with_metadata` to `handle_search`

**Goal:** Augment the `search_patterns` MCP tool's response with a structured metadata block
exposing per-result `rank`, `source_file`, `score`, and a top-level `mode` field that distinguishes
hybrid (Ollama embedder + FTS) from FTS-only fallback. The skill (Unit 3) consumes these fields
directly instead of parsing the prose response body.

**Requirements:** R5, R6 from the origin document. The `mode` field is the only signal that
distinguishes Ollama-down silent fallback from hybrid results — without it, R6's `degraded` state is
unreachable.

**Dependencies:** None. Can land before or alongside Unit 3, but must land before Unit 3 is verified
end-to-end.

**Execution note:** Test-first for the metadata contract. Write the metadata-shape assertion test
before modifying `handle_search`, mirroring the
`add_pattern_response_metadata_pins_commit_status_for_non_git_dir` discipline from the existing
`text_response_with_metadata` learning. The metadata shape is the contract; tests pin the contract.

**Files:**

- Modify: `src/server.rs` (the `handle_search` function around lines 386-491; the metadata block is
  built inline before the existing `text_response(req, &response)` call at line 487 and the call is
  replaced with `text_response_with_metadata(req, &response, &metadata)`)
- Test: `src/server.rs` (inline `#[cfg(test)]` module starting at line 840; add new tests to the
  existing search-patterns test cluster)

**Approach:**

- Build the metadata `serde_json::Value` in a `build_search_metadata` helper near the existing
  response helpers in `src/server.rs`. The plan originally called for an inline `json!` block in
  `handle_search` mirroring the four existing call sites, but the per-row `results` array pushed
  `handle_search` over the project's `clippy::too_many_lines = 100` limit, so the metadata
  construction was extracted into the helper. The helper is testable in isolation if a future test
  wants to assert on the metadata shape without going through the full handler.
- Introduce a `SearchMode` Rust enum with three variants — `Hybrid`, `FtsFallback`, `FtsOnly` — that
  maps from `(config.search.hybrid, embed_failed)` to a stable string via an exhaustive `as_str`
  match (no wildcard arm). This mirrors the discipline in `commit_status_metadata`: adding a new
  variant fails to compile until the JSON serialisation is updated. The three values distinguish
  full hybrid search, embedder-down fallback, and configured FTS-only deployments — all three need
  different client behaviour because their rank semantics are not comparable.
- Top-level metadata fields: `mode` (the three-value enum, serialised as a string), `query`,
  `top_k`, `result_count`. `embed_failed` is **NOT** exposed — it was strictly derivable from `mode`
  and the redundancy created a maintenance hazard.
- Per-result fields in a `results: [...]` array: `rank` (1-indexed integer matching the prose `[N]`
  prefix), `title`, `source_file`, `score`. Map from the existing `SearchResult` rows the function
  already iterates over for prose rendering.
- Replace the `text_response(req, &response)` call at line 487 with
  `text_response_with_metadata(req, &response, &metadata)`. The prose `response` body is
  **unchanged** — this is purely additive, so existing consumers reading `content[0].text` continue
  to work.
- **File-scoped restriction:** Unit 2 does NOT modify the `tools_list` handler, the
  `search_patterns` tool registration, or the JSON-Schema declaration of the tool's input/output.
  Only the request-handler (`handle_search`) response-building code and the new
  `SearchMode`/`build_search_metadata` helpers change. This is a stricter invariant than "the
  snapshot test still passes" — it bounds the blast radius of Unit 2 to one function and one helper
  module.

**Patterns to follow:**

- `handle_lore_status` at `src/server.rs:704-756` — the canonical metadata-emitting handler with the
  closest analogous shape (a top-level field cluster plus several optional inner blocks).
- `text_response_with_metadata` helper at `src/server.rs:793-811`.
- `lore_status_reports_non_git_state` test at `src/server.rs:1017-1055` — the metadata-assertion
  pattern: `assert!(resp["error"].is_null())`, read fields from `resp["result"]["metadata"]` by
  name, then assert on `resp["result"]["content"][0]["text"]` to confirm prose content is unchanged.

**Test scenarios:**

- **Happy path — hybrid mode, results present.** Insert a chunk for `rust/cargo-deny.md`, send a
  `search_patterns` request for `"cargo deny check"`, assert: `error.is_null()`,
  `metadata.mode == "hybrid"`, `metadata.embed_failed.is_null()` (the field was removed),
  `metadata.query == "cargo deny check"`, `metadata.top_k == 5`, `metadata.result_count >= 1`,
  `metadata.results[0].rank == 1`, `metadata.results[0].source_file == "rust/cargo-deny.md"`,
  `metadata.results[0].score > 0`. Also assert the prose `content[0].text` still contains
  `[1] ... (source: rust/cargo-deny.md)` — confirms additive non-breaking change.
- **Happy path — empty result set.** Insert no chunks, send a `search_patterns` request, assert:
  `metadata.results` is an empty array, `metadata.result_count == 0`, `metadata.mode == "hybrid"`,
  `metadata.embed_failed.is_null()`. Prose body is "No results."
- **Edge case — multiple chunks for same source file.** Insert two chunks for `rust/cargo-deny.md`
  (different headings), assert `metadata.results` contains two rows both with
  `source_file == "rust/cargo-deny.md"`, with `rank` values 1 and 2. This is the case where the
  skill computes "rank of the pattern = minimum rank across rows whose source_file matches".
- **Error path — embedder failure triggers `fts_fallback` mode.** Use a `FailingEmbedder`, send a
  `search_patterns` request, assert: `metadata.mode == "fts_fallback"`,
  `metadata.embed_failed.is_null()`. Prose body still contains the existing
  `Note: Ollama unreachable` prefix (per `src/server.rs:418-422`). The skill (Unit 3) consumes
  `metadata.mode` rather than string-matching the prose note.
- **Configuration path — FTS-only deployment reports `fts_only` mode.** Build a test harness with
  `config.search.hybrid = false`, insert a chunk without an embedding, send a `search_patterns`
  request, assert: `metadata.mode == "fts_only"` (distinct from `"hybrid"` and `"fts_fallback"`),
  `metadata.embed_failed.is_null()`, `metadata.result_count >= 1`,
  `metadata.results[0].source_file == "rust/cargo-deny.md"`. Pins the contract that `"fts_only"` is
  its own mode value, not aliased to `"hybrid"` (which the original derivation via `embed_failed`
  alone would have silently produced).
- **Edge case — `top_k` parameter respected.** Send a `search_patterns` request with `top_k: 3`,
  assert `metadata.top_k == 3` and `metadata.results.len() <= 3`.
- **Error path — JSON-RPC error response carries no metadata block.** Send a request with
  `top_k > MAX_TOP_K`, assert `resp["error"].is_null() == false` and the response either has no
  `result` or no `metadata` block. Pins the asymmetry the skill consumer relies on (read
  `resp["error"]` first; only read `result.metadata` on the success path).
- **Integration — `tools_list_returns_all_five_tools` snapshot still passes unchanged.** Sanity
  check on the file-scoped restriction. The existing test at `src/server.rs:927` should pass without
  modification.
- **Integration — existing `search_patterns_returns_results` test still passes unchanged.** Existing
  test at `src/server.rs:951-983` only asserts on prose content; since the prose body is unchanged,
  the test must still pass without modification. This proves the change is additive.

**Verification:**

- `cargo test` passes including the new metadata tests.
- `cargo test --features test-support` passes (matches `just test` conventions).
- `just ci` passes (formatting, clippy, deny, doc).
- The two integration tests above pass unchanged, proving non-breaking change.
- Manual MCP inspection in a fresh Claude Code session (after `/exit` and relaunch — see
  institutional learning #5): invoke `search_patterns` and confirm the response includes the new
  `metadata` sibling on `result`.

---

### - [ ] Unit 3: Create the `coverage-check` skill

**Goal:** Author the markdown skill at `integrations/claude-code/skills/coverage-check/SKILL.md`
that implements the entire coverage loop — pre-flight, brainstorm, ingest, cascade detection,
parallel search, four-state report, edit suggestions, iteration with diff-driven stability/ceiling
termination, exit reminder, JSONL log.

**Requirements:** R1, R1a, R1b, R2, R3, R4, R5 (consumer side), R6 (consumer side), R7, R8, R9, R10,
R11.

**Dependencies:**

- **Unit 2 must land first** — Unit 3's skill consumes the structured search metadata. Without Unit
  2, R6's `degraded` state is unreachable and the skill would have to parse prose, contradicting Key
  Decisions.
- Unit 1 should land before Unit 3 to keep the new skill consistent with the renamed `search` skill.
  If Units 1 and 3 land in the same PR (recommended), Unit 1 simply lands first in the diff order.

**Execution note:** Iterate the skill prompt against real patterns from `lore-patterns/` during
verification. The skill is markdown, not code; its quality is measured by whether it produces useful
coverage reports on real input. The cargo test below provides a structural sanity check (the file
parses, the frontmatter is valid, the referenced MCP tool names exist) but does not exercise
behaviour.

**Files:**

- Create: `integrations/claude-code/skills/coverage-check/SKILL.md`
- Test: `tests/coverage_check_skill_parses.rs` — a small `cargo test` that opens
  `integrations/claude-code/skills/coverage-check/SKILL.md`, asserts the YAML frontmatter parses
  cleanly with the four required fields (`name`, `description`, `disable-model-invocation`,
  `user-invocable`), and asserts that every MCP tool name referenced in the body (`search_patterns`,
  `lore_status`) appears in the lore MCP server's tool registration. This catches typos at build
  time without exercising runtime behaviour.

**Approach:**

- Frontmatter follows the search skill convention (post-rename):
  ```yaml
  ---
  name: coverage-check
  description: Audit a pattern file's vocabulary coverage by brainstorming agent-realistic queries, ingesting the file, and reporting which queries surface it. Catches paraphrase gaps in pattern wording. Not a quality gate — see paraphrase-bias note below.
  disable-model-invocation: true
  user-invocable: true
  ---
  ```
- The body is a single skill-prompt document instructing the agent through the loop. Section
  structure (each is a markdown subsection):
  1. **Purpose and limit disclosure (R11).** One paragraph naming the paraphrase-bias limit and the
     "necessary but not sufficient" framing. Cite the limit explicitly so authors don't treat 100%
     coverage as a quality gate.
  2. **Interaction hazard.** One callout warning the author never to run plain `lore ingest` between
     iterations of this skill — only `lore ingest --file`. Cite the cascade learning by filename.
  3. **Pre-flight (R1, R1a, R1b).** Instruct the agent to:
     - Run `command -v lore` and exit with the R1b error if missing.
     - Run `uname` once to detect Linux vs macOS/BSD and pick the canonicalisation command
       (`readlink -f` on Linux, `python3 -c "import os, sys; print(os.path.realpath(sys.argv[1]))"`
       on macOS/BSD) and the timestamp command (`date +%Y%m%dT%H%M%S%N` on Linux,
       `python3 -c "import time; print(int(time.time_ns()))"` on macOS/BSD).
     - Call the `lore_status` MCP tool, read `result.metadata.knowledge_dir`, canonicalise it via
       the picked command, canonicalise the target file path the same way, and verify the canonical
       target starts with the canonical knowledge_dir followed by `/`. Exit with the R1a error if
       the check fails. Note explicitly that this is a fast-path heuristic and the authoritative
       check is R4's `lore ingest --file` halt.
  4. **Read target file (R3 prep).** Instruct the agent to read the full target pattern body before
     brainstorming.
  5. **Brainstorm candidate queries (R3) with FTS5 rubric.** The rubric is the load-bearing detail.
     The prompt explicitly says:
     - 5–12 queries
     - Each query is 3–5 tokens
     - Each token is ≥3 alphabetic characters
     - No stop-words (`the`, `a`, `is`, `to`, `for`, `in`, `on`, …)
     - No command names with no queryable terms (`just ci`, `npm run`, `go test`, etc.)
     - Mix verbs (with synonyms: edit/update/create), nouns, and abbreviations the pattern body or
       tags actually contain
     - Reference the short-hook-queries learning by filename so the rubric is auditable Show the
       brainstormed list to the author before searching.
  6. **Refresh index (R4) with detect-then-prompt for `.loreignore`.** Run
     `lore ingest --file <target>` via Bash _without_ `--force`. On non-zero exit, halt with the
     verbatim error message. On zero exit, query `lore_status` again and check whether
     `chunks_indexed_for_source[<target>]` (or the equivalent — pick whichever lore_status field
     surfaces this; if none does, run a `search_patterns` for a distinctive token from the file body
     and check whether the target's source_file appears) is greater than zero. If it is zero, the
     file was silently skipped by `.loreignore`. Prompt the author: _"`<target>` is excluded by
     `.loreignore`. Run with `--force` to bypass the exclusion for this coverage check? (y/N)"_. On
     `y`, re-run with `--force`. On `N`, exit cleanly. If the second `--force` run still produces
     zero chunks, halt with an error explaining the contradiction.
  7. **Cascade detection (Unit 3 step explicitly added).** Before the main parallel search batch,
     issue one extra `search_patterns` call with a query constructed from a distinctive token in the
     pattern's body (the agent picks a unique-looking word that the FTS5 rubric would accept).
     Verify the target's `source_file` appears in `metadata.results`. If not, abort the iteration
     with: _"Index state changed unexpectedly between ingest and search. Check for parallel
     `lore ingest` processes (file watcher, pre-commit hook, second Claude Code session). The
     composition cascade documented in
     `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
     may have fired."_. The author can re-invoke after stopping the parallel writer.
  8. **Parallel search (R5).** Issue all candidate queries via the `search_patterns` MCP tool in one
     assistant turn (multiple tool calls per turn). Wait for all to settle before processing — no
     per-query timeout in v1. If `search_patterns` is not available, halt with a clear "MCP tool
     unavailable" error.
  9. **Score per-query state (R6).** For each query response:
     - If JSON-RPC `error` is non-null → state: `errored: <reason>`
     - Else if `result.metadata.mode == "fts_fallback"` → state: `degraded: fts_fallback`
     - Else look for any row in `result.metadata.results` whose `source_file` (canonicalised
       relative to `knowledge_dir`) matches the target → state: `surfaced (rank: N)` where N is the
       minimum rank across matching rows
     - Else → state: `not_present`

10. **Coverage ratio refusal on degraded queries.** **If any query in this iteration is in
    `degraded` state, the skill MUST NOT compute or display a coverage ratio for the iteration.**
    Render the report listing per-query states but replace the ratio line with: _"Embedder was
    partially unavailable (`<N>` of `<M>` queries degraded). Coverage ratio is not computed because
    hybrid and FTS-only ranks are not comparable. Retry once Ollama is back."_. Suggest no edits in
    this iteration. Skip to step 14 (exit reminder).
11. **Render coverage report (hybrid case).** A markdown section in chat with four subsections
    (surfaced, not_present, errored, degraded — though the degraded list will be empty thanks to the
    refusal at step 10). The surfaced subsection lists queries one per line, sorted alphabetically.
    Include the coverage ratio (computed against successfully-executed hybrid queries) and the
    errored count surfaced separately.
12. **Persist surfaced-list to temp file for stability check.** Write the sorted surfaced-query list
    (one query per line, no formatting) to `~/.cache/lore/coverage-check/<session>-iter-<N>.txt`,
    creating the directory if needed. The session ID is the same one used for the JSONL log path.
    This file is the substrate for R8's stability comparison via Bash `diff`.
13. **Append JSONL log (R10).** Append one JSON line via the pinned shell recipe in Key Technical
    Decisions. Skip if `LORE_NO_QA_LOG=1` is set. The line records the brainstormed queries,
    per-query outcomes, accepted edits (filled in later in the loop), exit reason, and iteration
    count.
14. **Suggest concrete edits (R7).** For each gap, propose one concrete edit: add a tag, add a
    phrase to the body, rephrase a heading, or extend the frontmatter. Reference the specific
    missing term and where the edit should land.
15. **Iteration loop (R8) with stability OR ceiling=3 termination.**
    - Author may accept, partially accept, or skip suggestions.
    - If any edits applied, increment iteration counter.
    - If counter > 3, exit with "ceiling reached at 3 iterations" message naming any remaining gaps.
      Note that the ceiling is per-invocation; the author can re-invoke to start a fresh counter.
    - Otherwise loop back to step 6 (re-ingest), step 7 (cascade detection), step 8 (parallel
      search), and re-run step 9-12 (score, refusal, render, persist).
    - **At step 12 of the second and subsequent iterations, immediately after writing iter-N.txt,
      issue:**
      `diff ~/.cache/lore/coverage-check/<session>-iter-{N-1}.txt
       ~/.cache/lore/coverage-check/<session>-iter-N.txt`
    - If the `diff` exit code is 0, the surfaced-query set is identical to the prior iteration →
      exit with "converged" message.
    - If non-zero → continue to step 14 (suggest more edits) or loop again.
16. **Exit and cleanup.**
    - Print the one-line recovery hint (R9): _if you discarded any iterated edits via
      `git checkout`, run `lore ingest` to reconcile the index against the current working tree._
    - Clean up `~/.cache/lore/coverage-check/<session>-iter-*.txt` (the JSONL log file is _not_
      cleaned up — it is the audit trail and persists across sessions).

**Patterns to follow:**

- `integrations/claude-code/skills/search/SKILL.md` (post-rename) for frontmatter shape and
  imperative prompt tone. Note that the body structure intentionally diverges from search/SKILL.md's
  single- paragraph format because the coverage-check loop has 16 distinct steps that benefit from
  explicit numbered subsections — this divergence is called out in Context & Research and is
  deliberate.
- The Vocabulary Coverage Technique section in `docs/pattern-authoring-guide.md` for terminology and
  authoring guidance the skill should reference.
- The structured-metadata consumption pattern is novel — there is no existing skill that consumes
  MCP tool metadata. The implementer is designing this convention for the lore plugin's first
  metadata- consuming skill.

**Test scenarios:**

**Automated coverage (cargo test):**

- **`tests/coverage_check_skill_parses.rs`** — opens the SKILL.md file, asserts the YAML frontmatter
  parses, asserts the four required frontmatter fields are present and have the expected types,
  asserts every MCP tool name mentioned in the body (`search_patterns`, `lore_status`) appears in
  the lore MCP server's tool registration (importable as a `pub` constant or via the tool list test
  fixture). This catches typos and stale tool references at build time.

**Must-run smoke tests (PR description checklist):**

These three are the load-bearing ones; the implementer ticks each one on the PR description before
requesting review.

- [ ] **Smoke 1: small new pattern, happy path.** In a Claude Code session with cwd inside
      `lore-patterns/` and lore plugin enabled, run `/lore:coverage-check rust/cargo-deny.md`.
      Expected: pre-flight passes, 5–12 queries brainstormed satisfying the FTS5 rubric, ingest
      runs, cascade detection passes, parallel searches return, coverage report rendered with rank
      info, at least one accepted edit suggestion closes a previously-absent query, JSONL log line
      written under `~/.cache/lore/qa-sessions/`. Total elapsed time on warm Ollama: a few minutes.
- [ ] **Smoke 2: out-of-scope path (R1a precondition failure).** Run
      `/lore:coverage-check /tmp/foo.md`. Expected: exits at the R1a step with the configured
      `knowledge_dir`, the canonical target path, and an actionable recovery message. Total elapsed
      time: under 5 seconds. No ingest call, no embedder work.
- [ ] **Smoke 3: degraded mode (R6 fts_fallback refusal).** Stop the Ollama server, run
      `/lore:coverage-check rust/cargo-deny.md`. Expected: search responses come back with
      `metadata.mode == "fts_fallback"`; the report surfaces all queries as `degraded`; the coverage
      ratio is **explicitly replaced** with the "embedder partially unavailable" message; no edits
      are suggested. The author sees a clear "Ollama is down, retry when it's back" rather than
      meaningless coverage numbers.

**Optional smoke tests** (run if time permits but not required):

- Symlinked `knowledge_dir`: create a symlink to the lore-patterns directory, set the configured
  `knowledge_dir` to the symlink, and run the skill on a file inside. Pre-flight should pass cleanly
  (the canonicalisation recipe resolves both sides via `readlink -f` / `python3 realpath`).
- Missing `lore` binary (R1b): temporarily rename `lore` out of `PATH`, run the skill, expect R1b
  error. Restore `lore`.
- Ingest failure (R4 halt): run on a non-`.md` file, expect ingest to exit non-zero and the skill to
  halt with the verbatim error.
- `.loreignore` detect-then-prompt: add a draft to `.loreignore`, run the skill on it, expect the
  prompt; answer y → proceeds with `--force`; answer N → exits cleanly.
- Cascade detection: in a second terminal, run `lore ingest` between the skill's ingest and the
  cascade-detection search call. Expected: cascade detection fires with the parallel- process
  warning.
- Convergence: pattern that converges after 1 iteration → "converged" exit via `diff` exit code 0.
- Ceiling reached: pattern that requires more than 3 iterations → "ceiling reached" exit. Re-invoke
  and confirm counter resets.
- `LORE_NO_QA_LOG=1` opt-out: set the env var, run the skill, confirm no JSONL line is appended.

**Verification:**

- The cargo test passes (`cargo test
  --test coverage_check_skill_parses`).
- The three must-run smoke tests pass with the expected outcomes on warm Ollama against
  `lore-patterns/`. Each is ticked off in the PR description checklist.
- The JSONL log file at `~/.cache/lore/qa-sessions/` contains one line per skill invocation (except
  when `LORE_NO_QA_LOG=1`).
- Manual eyeball review of the JSONL line confirms the canonical shape from the brainstorm's Key
  Decisions sketch.
- Iterating the prompt wording during the smoke tests produces a prompt that consistently elicits
  FTS5-friendly queries without manual intervention.
- The PR description includes the `/exit` + relaunch step prominently (institutional learning #5).

---

## System-Wide Impact

- **Interaction graph:**
  - `handle_search` is called by the MCP server for `search_patterns` requests. Existing callers
    (only the Claude Code plugin via the `search` skill — Unit 1's renamed `search-lore` — and any
    future skills) see the additive metadata field but continue to work against the prose body. No
    callers break.
  - `lore_status` is called by the new skill twice per iteration: once for the R1a pre-flight (read
    `metadata.knowledge_dir`) and once after ingest to check `chunks_indexed_for_source` for the
    `.loreignore` detect-then-prompt step (R4). `handle_lore_status` is unchanged.
  - `lore ingest --file` (with and without `--force`) is called via Bash by the new skill. Existing
    CLI behavior is unchanged.
- **Error propagation:**
  - JSON-RPC errors from `search_patterns` propagate to the skill as "errored" per-query state —
    they do not abort the entire coverage report.
  - Bash exit codes from `lore ingest --file` propagate to the skill; non-zero halts the loop with
    the verbatim error message.
  - Pre-flight failures (R1a, R1b) exit the skill before any embedder work, with an actionable error
    message naming the precondition.
  - Cascade detection failure aborts the iteration with a parallel-process warning.
- **State lifecycle risks:**
  - The composition cascade is documented in the skill's interaction- hazard callout AND actively
    detected via the Unit 3 step 7 search call. Detection costs one extra MCP call per iteration and
    converts a silent failure into a loud one.
  - The local SQLite index is mutated by every iteration of the skill. Running on a draft and then
    discarding edits leaves pollution in the index until the next walk-based reconcile — the R9 exit
    reminder mitigates this with a recovery hint.
  - The JSONL log accumulates over time. v1 has no rotation policy; if accumulation becomes a
    problem, a follow-up todo can add a rotation-by-size or rotation-by-age helper. Per-line size is
    small (one session's metadata), so realistic accumulation is bounded.
  - The temp files under `~/.cache/lore/coverage-check/` are cleaned up on skill exit (success or
    failure). If the skill crashes between writing a temp file and cleanup, the file persists until
    the next invocation overwrites it (different session ID) — the accumulation is bounded by the
    number of crash sessions.
- **API surface parity:**
  - The metadata addition to `search_patterns` is purely additive. Existing prose-body consumers
    (none in lore today, but any external consumer of the MCP server) continue to work.
  - The skill rename from `search-lore` to `search` is a breaking change for slash-command muscle
    memory. The brainstorm explicitly accepts this cost on the grounds of zero-released- users.
- **Integration coverage:**
  - The Rust change to `handle_search` is covered by inline unit tests in `src/server.rs`.
  - The skill file is structurally validated by `tests/coverage_check_skill_parses.rs` (frontmatter
    parses, referenced MCP tools exist).
  - End-to-end coverage of the skill is by the three must-run smoke tests against `lore-patterns/`.
    The optional smoke tests cover edge cases that are valuable but not blocking.
  - The `tools_list_returns_all_five_tools` snapshot test at `src/server.rs:927` provides a sanity
    check that the tool count is unchanged. The existing `search_patterns_returns_results` test at
    `src/server.rs:951-983` provides a sanity check that the prose body is unchanged.
- **Unchanged invariants:**
  - `handle_search`'s prose `content[0].text` body is unchanged.
  - The `tools_list` handler and `search_patterns` tool registration are unchanged (Unit 2's
    file-scoped restriction).
  - `lore ingest --file`'s contract is unchanged. The chunk-replacement test
    (`re_ingesting_same_file_replaces_chunks_without_duplication`) pins the contract that the skill
    depends on.
  - `lore_status`'s metadata fields are unchanged. The skill reads `metadata.knowledge_dir`, which
    has been in place since the `lore_status` tool shipped.
  - The plugin manifest at `integrations/claude-code/.claude-plugin/plugin.json` is unchanged by the
    rename — skills are auto-discovered from `./skills/` by directory name.

## Risks & Dependencies

| Risk                                                                                                                                       | Mitigation                                                                                                                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The skill prompt produces poor candidate queries (paraphrase bias too strong, FTS5 rubric not enforced enough).                            | Iterate the prompt during Unit 3's smoke test loop. The rubric is encoded in the prompt with explicit citation of the short-hook-queries learning. If the smoke tests show consistent quality issues, refine before merging.                                                                 |
| MCP server doesn't pick up the new `handle_search` binary on `/plugin reload`.                                                             | PR description test plan explicitly includes a `/exit` and Claude Code relaunch step after `just install`, before any metadata-shape verification. Made prominent so the implementer cannot miss it.                                                                                         |
| Composition cascade: parallel `lore ingest` wipes single-file chunks between the skill's ingest and search steps.                          | Unit 3 step 7 actively detects this with one extra `search_patterns` call between ingest and main search batch. On detection, the iteration aborts with a clear parallel-process warning. The hazard callout in SKILL.md remains as the documentation layer.                                 |
| The structured metadata addition to `handle_search` accidentally changes the prose body or modifies tool registration.                     | Unit 2's file-scoped restriction explicitly forbids modifying `tools_list`, the `search_patterns` registration, or input/output schemas. The existing `search_patterns_returns_results` and `tools_list_returns_all_five_tools` tests serve as regression checks.                            |
| The R1a pre-flight check passes but the actual ingest fails due to canonicalisation differences.                                           | R1a uses a pinned shell recipe (`readlink -f` on Linux, `python3 realpath` on macOS/BSD) so the heuristic is at least deterministic. R4's halt-on-non-zero is the authoritative check. The two-layer design is documented in the skill prompt.                                               |
| `tools_list_returns_all_five_tools` snapshot test breaks because someone (not v1) adds an output-schema declaration to the search tool.    | Unit 2's file-scoped restriction makes the invariant about _which files change_, not about _which tests pass_. An implementer who tries to add tool-schema declarations as part of Unit 2 violates the restriction explicitly.                                                               |
| Manual smoke tests in Unit 3 are skipped under time pressure.                                                                              | The smoke test set is cut from 7 to 3 must-run tests, formatted as a PR-description checklist the implementer ticks. The cargo test (`coverage_check_skill_parses`) provides a structural backstop that catches typos and stale tool references at build time without requiring smoke setup. |
| The JSONL log path resolution fails on the implementer's machine (`$XDG_CACHE_HOME` not set, NFS mount, `python3` not available on macOS). | The pinned shell recipe handles `${XDG_CACHE_HOME:-$HOME/.cache}` fallback, aborts loudly on EACCES, and uses `uname` to pick the platform-appropriate timestamp/realpath command. The implementer verifies the recipe during smoke test 1.                                                  |
| R8 stability comparison via model self-comparison would be unreliable (chat compaction, whitespace drift, hallucinated equality).          | Replaced with Bash `diff` against temp files under `~/.cache/lore/coverage-check/<session>-iter-N.txt`. The exit code of `diff` is the loop signal. No model judgment involved in the stability check itself; the model only writes the input file in a known sorted format.                 |
| `.loreignore`d files cannot be coverage-checked silently (the brainstorm's blanket `--force=true` would have overridden author intent).    | Detect-then-prompt: try `lore ingest --file <path>` first; if zero chunks ingested, prompt the author before retrying with `--force`. One keystroke cost, preserves author intent.                                                                                                           |
| Author runs the skill against the same file 20 times in a row, oscillating gaps without noticing.                                          | Captured under Future Considerations (cross-invocation oscillation detection). v1 is intentionally a JSONL-log producer; consuming the log to detect thrashing is a followup that does not block this plan.                                                                                  |

## Future Considerations

These are not v1 work, but the plan acknowledges them so they are not forgotten.

- **Cross-invocation oscillation detection.** v1's R8 ceiling is per- invocation. An author who
  re-invokes the skill repeatedly can cause the gap set to oscillate (close one query, break
  another) across sessions without noticing. A follow-up could read the JSONL log on skill start,
  grep for the same target file's recent invocations (last N entries within last 24h), and warn if
  the union of closed/reopened query sets across sessions overlaps. This is intentionally deferred —
  v1 is strictly a log producer, never a reader, and adding read-side logic contradicts that
  posture. The log exists _exactly_ so a future tool can do this analysis without v1 having to.
- **`lore qa` deterministic Rust subcommand (followup 1 from origin).** Reuses the structured
  metadata from this plan's Unit 2 as its data source. The `pattern-qa` skill name is reserved for
  the version that pairs with this subcommand.
- **Automated end-to-end skill test.** v1's verification is the cargo structural test plus three
  manual smoke tests. A future improvement could spin up a headless Claude API session, feed the
  skill prompt directly, and assert structural properties of the output. Not in v1 because the
  harness is non-trivial and the structural cargo test plus 3 smoke tests catches the most likely
  regressions.
- **`lore_status` returning per-source chunk count.** Unit 3 step 6 needs to determine whether a
  target file's chunks are zero after an ingest (the `.loreignore` detect signal). v1 either gets
  this from a future `lore_status` field or falls back to a `search_patterns` for a distinctive
  token. If `lore_status` adds a per-source-file chunk count later, the skill prompt becomes
  cleaner.

## Design pivot: layer 2 finding

**Added 2026-04-07 during real-run testing on a separate machine.** The original design relied on
reading `result.metadata` directly from the `search_patterns` and `lore_status` MCP responses. This
was correct at the wire-format layer — `lore serve` emitted the sibling correctly and the unit tests
verified it. It was **wrong** for the primary consumer.

### What we found

Claude Code's MCP client strips the `metadata` sibling from `result` before forwarding tool
responses to the agent. Only the `content[]` array reaches the model. Every lore MCP tool that used
`text_response_with_metadata` (`search_patterns`, `lore_status`, `add_pattern`, `update_pattern`,
`append_to_pattern`) was silently broken for Claude Code agents. The coverage-check skill's step 2
(R1a pre-flight reading `metadata.knowledge_dir` from `lore_status`), step 6 (cascade detection +
search-mode pre-flight reading `metadata.mode`), step 7 (parallel search batch), and step 8
(per-query state classification reading `metadata.results[].rank/source_file`) all depended on a
channel that did not reach the agent.

The finding was reproduced in two ways:

1. Calling `search_patterns` and `lore_status` directly from inside my own Claude Code session and
   observing that the tool output contained only the prose body — no `metadata` sibling reached the
   agent.
2. Piping a `tools/call` request to `lore serve` over stdio with
   `printf '...' | lore serve 2>/dev/null | jq '.result | keys'` and observing
   `["content", "metadata"]` — confirming the wire format was correct and the stripping happened on
   the client side, not in the lore binary.

A follow-on exploratory test emitted two text blocks in `content[]` to check whether Claude Code
forwards multi-block arrays. It does — but it concatenates the blocks into a single flat string with
no delimiter, gluing the second block's text directly onto the end of the first block's body. This
confirmed that `content[]` reaches the agent but needs an explicit in-text delimiter to be
parseable.

### What we pivoted to

Structured metadata now travels inside `content[0].text` as a fenced code block with the distinctive
language tag `lore-metadata`. The fenced block is opt-in via a new `include_metadata: bool`
parameter on every lore MCP tool schema. Default callers (the `search` skill, hook-injected queries,
general-purpose `search_patterns` calls from agents that only want the prose) pay no token cost for
the embedded JSON. Opt-in callers (specifically the coverage-check skill) pass
`include_metadata: true` on every call.

The `result.metadata` sibling is dropped entirely from lore's MCP response shape. It was unreachable
from the primary consumer and kept only as redundant noise; removing it simplifies the wire format
and eliminates the dual-channel maintenance hazard.

The skill's four metadata-consuming steps (2, 6, 7, 8) now extract the fenced block from
`content[0].text` using a deterministic recipe: locate the last opening triple-backtick fence with
language tag `lore-metadata`, advance past it, read forward until the next closing triple-backtick
fence, and parse the intervening text as JSON. The extractor is safe because `serde_json::to_string`
escapes newlines, so the first newline-then-fence after the opening marker is unambiguously the
closing fence — never a false match inside a JSON string value.

### What carried over unchanged

- The three-value `SearchMode` enum (`Hybrid` / `FtsFallback` / `FtsOnly`). The metadata payload
  shape is identical; only the transport channel changed.
- The canonical JSON shape (queries, per-row rank/source_file/score, top-level
  mode/result_count/query/top_k). Same field names, same structure, extracted from a different place
  in the response.
- The exhaustive-match discipline for enum serialisation. `SearchMode::as_str` and
  `commit_status_metadata` still have no wildcard arms; adding a variant fails to compile until the
  serialiser is updated.
- The skill's logic for pre-flight containment, cascade detection, degraded-mode refusal, iteration
  loop with Bash `diff` stability check, and ephemeral JSONL log. Only the metadata extraction
  channel changed.

### Institutional learning

The superseded learning at
`docs/solutions/best-practices/mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md` was marked
with `status: superseded` and a prominent banner pointing at the new learning. The new learning at
`docs/solutions/best-practices/mcp-metadata-via-fenced-content-block-2026-04-07.md` documents the
production pattern (opt-in parameter, fenced block, extractor recipe, test discipline) and explains
why the sibling channel fails for Claude Code. The three principles from the superseded learning
that remain correct — state both branches of conditional behaviour in the tool description, use a
tagged-union discriminator, pin the contract with tests — are preserved and repeated in the new
learning.

### Tests added / rewritten

- **Rewrote 10 existing metadata tests** (5 `search_patterns`, 1 `add_pattern`, 4 `lore_status`) to
  pass `include_metadata: true` and assert on the fenced block via a new
  `extract_lore_metadata_fence` test helper. Tests now use the project's AAA structure with
  `// Arrange`, `// Act`, `// Assert` comment labels.
- **Added 3 new "default path omits fence" tests** pinning the opt-in contract:
  `search_patterns_omits_metadata_fence_by_default`, `lore_status_omits_metadata_fence_by_default`,
  and `add_pattern_omits_metadata_fence_by_default`. These verify that callers who do not pass the
  parameter get a clean prose-only response and that `extract_lore_metadata_fence` returns `None`.
- **Updated `search_patterns_response_metadata_absent_on_error`** to assert that error responses
  have `result` null, not just that they lack a `metadata` sibling. Passes `include_metadata: true`
  to verify the opt-in setting does not leak metadata into error responses.
- **Regenerated the `tools_list_returns_all_five_tools` insta snapshot** to reflect the new
  `include_metadata` property in every tool's `inputSchema`.

### What remains untested

End-to-end skill behaviour from inside a Claude Code session. The cargo structural test
(`tests/coverage_check_skill_parses.rs`) verifies the skill file's frontmatter and tool-name
references but does not exercise the fenced-block extraction logic or the iteration loop. A future
improvement could spin up a headless Claude API session and pipe the skill prompt through it — but
that harness is non-trivial and outside v1 scope. For now, the three must-run smoke tests in the PR
description remain the behavioural verification gate.

## Design pivot: query source — from LLM brainstorm to hook simulation

**Added 2026-04-08 after end-to-end skill testing.** The second and more consequential mid-PR pivot.
v1's original step 4 asked the LLM to brainstorm 5-12 candidate queries from the pattern body.
Real-run testing exposed the obvious flaw: the same agent that reads the pattern body also writes
the queries, so the queries paraphrase the body nearly losslessly, coverage is trivially high, and
the report tells the author nothing about production discoverability. The brainstorm rubric (3-5
tokens, no stop-words, no command-name-only queries) was an attempt to patch this by forbidding the
known failure modes, but forbidding the failure modes is exactly wrong — those failure modes are the
signal the skill should be measuring.

### What we pivoted to

A new `lore extract-queries` CLI subcommand that wraps the existing
`hook::extract_query(&HookInput) -> Option<String>` logic. It reads a thin JSON envelope
(`{tool_name, tool_input}`) from stdin, wraps it into a `PreToolUse` `HookInput`, and prints the
resulting FTS5 query to stdout (or nothing if no terms survive cleaning). Empty output is the
diagnostic signal — it means the PreToolUse hook would inject nothing for that tool call, which is a
real production-discoverability finding.

The skill's step 4 now produces queries in three layered sources:

1. **`qa_simulations` frontmatter override (opt-in).** An optional frontmatter list of
   `{tool_name, tool_input}` objects. If present, the skill uses them verbatim and skips inference.
   This is the escape hatch for patterns whose discoverability depends on unusual tool calls
   automatic inference cannot guess.
2. **Automatic inference (default).** The skill inspects the pattern's tags, headings, concrete
   filenames, and fenced code block contents to construct 3-6 synthetic tool calls, guided by
   tag-to-tool-call lookup tables embedded in the skill prompt (rust/cargo, typescript/pnpm,
   python/uv, ruby/rails, ci/github-actions, sqlite, git, yaml, testing). Concrete filenames named
   in the pattern body (`deny.toml`, `justfile`) take precedence over inferred ones.
3. **Author confirmation** before any extraction runs. The skill renders the inferred tool-call list
   to chat and asks the author to confirm, edit, or replace. This keeps the author in the loop
   without reintroducing paraphrase bias — the author is correcting tool calls, not writing queries.

For each confirmed tool call, the skill pipes the JSON envelope through `lore extract-queries` and
collects the stdout line as one candidate query. The candidate set is the union of non-empty
results.

### What this buys

- **Query strings are hook output, not author paraphrase.** They are byte-for-byte what the
  PreToolUse hook would inject when an agent issues the same tool calls at runtime. Paraphrase bias
  is reduced from "the agent reworded the pattern body" to "the agent picked which tool calls to
  simulate" — still a source of bias, but a much smaller one, and one the author can audit at
  confirmation time.
- **Empty queries become diagnostics.** A tool call that produces empty stdout (e.g. `Bash just ci`
  — `just` is a stop-word, `ci` is shorter than three characters) is a real finding: the hook would
  inject nothing for that call, so the pattern's discoverability via that route is structurally
  zero. The skill records the fact and continues; it no longer forbids the failure modes it should
  be measuring.
- **Zero-candidate degenerate case halts loudly.** If every inferred tool call produces empty
  output, the skill halts with a clear next-action message telling the author to reword tags and
  headings or add a `qa_simulations` override.

### What carried over unchanged

- The iteration loop (ingest → search → score → suggest → iterate) and its Bash `diff` stability
  comparison.
- The three pre-flight aborts at step 6 (cascade detection, `fts_fallback`, `fts_only`).
- The ephemeral JSONL log under `${XDG_RUNTIME_DIR:-/tmp/lore-$(id -u)}/lore/qa-sessions/`.
- The fenced `lore-metadata` block extraction recipe from the layer-2 pivot.
- The three-value `SearchMode` enum and exhaustive-match discipline.

### Tests added

- **`tests/extract_queries.rs`** — five integration tests covering the happy path (`Edit src/lib.rs`
  emits a `rust` anchor, `Bash cargo deny check` emits a rust anchor with enrichment,
  `Edit app/page.tsx` emits a `typescript` anchor), the degenerate case (`Bash just
  ci` emits
  empty stdout), and the malformed-input failure path (invalid JSON exits non-zero with a stderr
  message containing "invalid JSON").

### What remains manual

- The tool-call inference step itself. The tag-to-tool-call lookup tables embedded in the skill
  prompt are heuristics, not data; the LLM still chooses among them. A follow-up could replace the
  heuristics with a deterministic Rust pass that walks the pattern's frontmatter and body, but v1
  keeps inference inside the skill prompt to avoid blocking on another Rust subcommand.
- The paraphrase bias in the inference step. The author-confirmation checkpoint is the mitigation;
  the agent shows its work before any extraction runs.

## Documentation / Operational Notes

- **PR description test plan must include the `/exit` + relaunch step** between `just install` and
  any verification of the new metadata-consuming skill behavior. Without this step, the MCP server
  will continue serving the old binary and the skill will appear broken. Make this prominent.
- **PR description must include the three must-run smoke tests as a checkbox list** the implementer
  ticks before requesting review.
- **`docs/pattern-authoring-guide.md`** does not need updating for v1. The Vocabulary Coverage
  Technique section already describes the manual loop; the new skill is the automated version. A
  follow-up doc PR can add a one-line note pointing to the skill, but it's not in this plan's scope.
- **`ROADMAP.md`** should be updated to mark this work as completed (move from "Up Next" if listed
  there to "Completed") when the PR merges. Not part of the implementation units; handled at PR
  merge time.
- **No release process changes.** v1 of the skill ships with the current install flow
  (`just install`). The pattern repository CI mode is followup 3 and explicitly depends on the lore
  release process being shipped first.
- **Follow-up todos to capture under `docs/todos/` after this plan ships:**
  - Followup 1 from the brainstorm: `lore qa` deterministic Rust subcommand. Will reuse the
    structured metadata from this plan's Unit 2 as its data source.
  - Followup 2: persisted `qa_queries` metadata schema in the pattern repository. The local JSONL
    log from this plan provides design-time ground truth.
  - Cross-invocation oscillation detection (see Future Considerations above).
  - Followups 3, 4, 5, 6, 7: as captured in the brainstorm's Followup Work section.

## Sources & References

- **Origin document:**
  [`docs/brainstorms/2026-04-07-pattern-qa-skill-requirements.md`](../brainstorms/2026-04-07-pattern-qa-skill-requirements.md)
- **Authoring guide section the skill automates:** `docs/pattern-authoring-guide.md` § Vocabulary
  Coverage Technique
- **Existing skill convention:** `integrations/claude-code/skills/search-lore/SKILL.md` (renamed to
  `search/` in Unit 1)
- **Plugin manifest:** `integrations/claude-code/.claude-plugin/plugin.json`
- **Rust pattern to mirror:**
  - `src/server.rs:793-811` (`text_response_with_metadata` helper)
  - `src/server.rs:704-756` (`handle_lore_status` reference handler)
  - `src/server.rs:541-547` (`handle_add` reference handler)
- **Test pattern to copy:** `src/server.rs:1017-1055` (`lore_status_reports_non_git_state`)
- **Single-file ingest contract pin:**
  `tests/single_file_ingest.rs::re_ingesting_same_file_replaces_chunks_without_duplication`
- **Institutional learnings:**
  - `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
  - `docs/solutions/best-practices/mcp-tool-conditional-outcomes-as-metadata-2026-04-06.md`
  - `docs/solutions/best-practices/short-hook-queries-favour-fts5-over-semantic-search-2026-04-05.md`
  - `docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md`
  - `docs/solutions/best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md`
  - `docs/solutions/best-practices/cli-suppress-stderr-in-json-mode-2026-04-03.md`
  - `docs/solutions/integration-issues/reload-plugins-does-not-restart-mcp-servers-2026-04-03.md`
  - `docs/solutions/integration-issues/claude-code-plugin-assembly-pitfalls-2026-04-02.md`
  - `docs/solutions/best-practices/filter-changes-in-delta-pipelines-need-bidirectional-reconciliation-2026-04-06.md`
- **Related PRs:** PR #29 (single-file ingest, the prerequisite this skill builds on)
