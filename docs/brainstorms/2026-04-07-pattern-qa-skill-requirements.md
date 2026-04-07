---
date: 2026-04-07
topic: coverage-check-skill
---

# Coverage Check Skill

## Problem Frame

Pattern authors writing for lore need to verify that the patterns they author will actually surface
for the queries an agent would type in real work. The authoring guide already describes a manual
Vocabulary Coverage Technique (brainstorm candidate queries, ingest the file, run the queries, check
whether the pattern surfaces, edit and repeat) but the loop is tedious enough that authors skip it.
Patterns ship discoverable-in-theory and undiscoverable-in- practice.

The work is now mechanically possible thanks to single-file ingest (`lore ingest --file`, shipped in
PR #31), which removed the commit-before- test friction. What is missing is a streamlined way to
actually run the loop without manually chaining ingest, multiple search calls, gap analysis, and
edit suggestions.

This brainstorm scopes the smallest useful version of that streamlining: a Claude Code skill that
automates the manual checklist, runs entirely in the pattern author's session, and ships as a
markdown skill paired with one minimal Rust change (structured metadata on the existing search MCP
handler) and no schema commitments. Richer versions (deterministic CLI subcommand, persisted query
metadata, CI checks, bias mitigations) are deliberately deferred to followups so the v1 can land
quickly and start producing value.

The skill is named `coverage-check` rather than `pattern-qa` to reserve the QA slot in the product
surface for the deterministic-helper-backed version that earns it. v1 is a coverage check; the
eventual quality gate is not.

## User Flow

```
┌────────────────────────────────────────────────────────────────────┐
│ Author session (target file resolves inside lore knowledge_dir)    │
└────────────────────────────────────────────────────────────────────┘
        │
        ▼
  ┌──────────────────────────────────────────┐
  │ /lore:coverage-check rust/cargo-deny.md  │
  └──────────────────────────────────────────┘
        │
        ▼
  Pre-flight: command -v lore (R1b)
              lore_status → verify file is inside knowledge_dir (R1a)
  (exit early with actionable error if either fails)
        │
        ▼
  Read pattern file
        │
        ▼
  Brainstorm 5-12 candidate queries from pattern content
        │
        ▼
  lore ingest --file rust/cargo-deny.md     (Bash)
        │
        ▼
  For each query: lore:search_patterns       (MCP, parallel, structured)
        │
        ▼
  Coverage report (rendered to chat AND appended as JSONL line):
    surfaced (rank) + not present + errored + degraded (fts_fallback or fts_only)
        │
        ▼
  Suggest concrete edits to close gaps
        │
        ▼
  ┌─────────────────────┐
  │ Apply edits?        │
  ├─────────────────────┤
  │ yes → edit file ────┼──→ re-ingest, re-search ──┐
  │ skip → exit         │                            │
  └─────────────────────┘                            │
        ▲                                            │
        └────────────────────────────────────────────┘
        (loop terminates when surfaced-query set is unchanged
         from previous iteration, OR after 3 iterations
         per invocation; ceiling resets on re-invocation)
```

## Requirements

**Invocation and scope**

- R1. The skill is invoked as `/lore:coverage-check <path>` (or `/coverage-check <path>` when no
  collision exists). Its precondition is that the target file resolves to a path inside the
  configured lore `knowledge_dir`, not merely that the session's working directory is a pattern
  repository — the two are not equivalent in monorepos or when the configured `knowledge_dir` is a
  subdirectory of the session's cwd.
- R1a. Before brainstorming queries, the skill calls the `lore_status` MCP tool to read the
  configured `knowledge_dir`, canonicalises the target path, and verifies the target lives inside
  the configured directory. On failure, the skill exits with an actionable message naming the
  precondition (the configured `knowledge_dir`, the target path, and what would have to change for
  the invocation to succeed). This check runs in one MCP call before any embedder work, so
  out-of-scope invocations fail in seconds rather than after the brainstorm phase. This check is a
  **fast-path heuristic, not a guarantee**. The authoritative containment check is
  `validate_within_dir` inside `lore ingest --file`'s Rust code (see R4). If the pre-flight passes
  but the ingest fails because of a canonicalisation difference (symlinks, case sensitivity,
  trailing slashes), R4's halt clause catches it. Pre-flight exists to fail fast on the obvious
  mismatches, not to guarantee parity with Rust-side canonicalisation.
- R1b. Before any Bash invocation of `lore`, the skill verifies the binary is on `PATH` (e.g. via
  `command -v lore`) and exits with an actionable message if it is not. Plugin installation does not
  guarantee the CLI is on `PATH`; this check converts a confusing "command not found" into a clear
  precondition failure.
- R2. The skill operates on a single markdown file per invocation. Batch mode and directory mode are
  out of scope for v1.

**Coverage loop**

- R3. The skill reads the target pattern file in full and brainstorms 5-12 candidate queries an
  agent would plausibly type when this pattern would be useful. The query list is shown to the
  author for transparency before any searches run.
- R4. The skill calls `lore ingest --file <path>` to refresh the local index with the current
  working-tree contents of the file. Single-file ingest is used because it does not require the file
  to be committed and is orthogonal to walk-based delta state. If `lore ingest --file` exits
  non-zero, the skill halts, reports the ingest error message verbatim, and does not proceed to
  search — a stale index would silently produce incorrect coverage reports.
- R5. The skill runs each candidate query through the `search_patterns` MCP tool with `top_k = 5`.
  Queries are issued in parallel where Claude Code's tool-use loop allows (multiple tool calls per
  assistant turn), so the wall-clock cost approaches the latency of one query rather than N
  sequential queries. The skill **waits for all parallel calls to settle** before rendering the
  report; v1 commits no per-query timeout, accepting that one slow Ollama query can hold up the
  report. There is no Bash fallback in v1: if `search_patterns` is unavailable, the skill fails
  loudly with a clear message rather than silently degrading to a path the author has no way to
  verify.

  `search_patterns` returns its result rows as a structured metadata block (added in this same pull
  request — see Dependencies and Key Decisions) containing per-row `rank`, `source_file`, `score`,
  and a top-level three-value `mode` field: `hybrid` (Ollama embedder plus FTS via reciprocal-rank
  fusion), `fts_fallback` (Ollama unreachable in a hybrid-configured deployment, silently fell back
  to FTS-only for this query), or `fts_only` (deployment is configured for FTS-only via
  `config.search.hybrid = false`, embedder never attempted). The skill consumes the metadata block
  directly rather than parsing the human-readable prose body of the response. Both `fts_fallback`
  and `fts_only` use BM25 ranks that are not comparable to hybrid RRF scores, so the skill must
  refuse to compute aggregate coverage metrics in either state.
- R6. The skill produces a coverage report listing each query and one of **four** states for the
  target pattern:
  - **surfaced (rank N):** the target's `source_file` appears in a result row at rank N within the
    top 5. When chunked results return multiple rows for the same file, the rank is the minimum
    across matching rows.
  - **not present:** no result row's `source_file` matches the target.
  - **errored: <reason>:** the MCP tool returned a JSON-RPC error for this query (transport failure,
    malformed input, internal panic). Distinct from "timed out", which is not a v1 state because v1
    commits no per-query timeout.
  - **degraded:** the MCP tool succeeded but returned `mode: fts_fallback` (embedder unreachable in
    a hybrid-configured deployment) or `mode: fts_only` (deployment configured for FTS-only).
    Hybrid-mode rank is not comparable to FTS-only rank, so the skill refuses to compute coverage
    for queries in this state and surfaces them separately. For `fts_fallback` the author should
    retry once Ollama is back; for `fts_only` the author should reconfigure lore with
    `hybrid = true`. In practice the skill catches both cases at step 6's cascade-detection
    pre-flight before the parallel batch ever runs, so this state in step 8 is reserved for the edge
    case where the embedder fails partway through the parallel batch.

  Per-query errors and degradation do not abort the report. The overall coverage ratio is computed
  against successfully-executed hybrid-mode queries only; the counts of errored and degraded queries
  are surfaced separately so the author can distinguish a real gap from a tool failure or an
  unverifiable result.
- R7. For each gap, the skill proposes a concrete edit to close it: add a tag, add a phrase to the
  body, rephrase a heading, or extend the frontmatter. Suggestions reference the specific missing
  term and where the edit should land.
- R8. The author may accept, partially accept, or skip the suggestions. If any edits are applied,
  the skill re-runs steps R4 through R7 automatically. The loop terminates when (a) the set of
  surfaced queries is identical to the previous iteration (no new query newly surfaces and no
  previously-surfaced query newly drops), OR (b) the iteration count reaches three, whichever comes
  first. The final report distinguishes the two exit paths so the author knows whether coverage
  converged or the ceiling was reached with gaps remaining. The ceiling is per-invocation: an author
  who wants to keep iterating can re-invoke `/lore:coverage-check` and start a fresh counter. The
  ceiling exists to prevent pathological oscillation within one invocation, not to cap total effort
  on a difficult pattern.

  **Implementation note (planning):** the previous iteration's surfaced-query set must be rendered
  as a sorted code-fenced list inside the chat report each cycle, so the skill is comparing strings
  mechanically rather than making a judgment call about set equality. This is the load-bearing
  detail that turns R8's stability predicate from "looks the same" into "the rendered list literally
  equals the prior rendered list".

**Output and persistence**

- R9. The skill writes (a) author-approved edits to the target pattern file, and (b) chunks for the
  working-tree version of the file to the local SQLite index — this is `lore ingest --file`'s normal
  behaviour and happens regardless of whether the author accepts any edit suggestions. Running the
  skill against a draft that the author later discards (e.g. via `git checkout`) temporarily
  pollutes search results until the next walk-based `lore ingest` reconciles. On exit, the skill
  prints a one-line reminder: _if you discarded any of the iterated edits via `git checkout` or
  similar, run `lore ingest` to reconcile the index against the current working tree._ This converts
  the index-pollution caveat from a passive warning into a recovery hint. The skill writes no other
  files into the pattern repository: no metadata, no sidecar files, no temporary artefacts in the
  repo tree.
- R10. The coverage report is rendered in the chat session and is ephemeral as far as the **pattern
  repository** is concerned — diffs after a session are limited to author-approved edits to the body
  and frontmatter of the target file. The skill does, however, append a single JSONL line per
  invocation to `~/.cache/lore/qa-sessions/<timestamp>.jsonl` (or the platform equivalent under the
  lore state directory). The line records the brainstormed query set, the per-query outcomes, the
  accepted edits, the exit reason, and the iteration count. This log lives outside any pattern
  repository, is gitignored by virtue of its location, can be opted out via an environment variable
  (`LORE_NO_QA_LOG=1`), and exists solely so that followups 1, 2, and 4 — when they come to be
  designed — have real session data to ground their design choices in rather than synthetic
  intuition. The log is never read by v1 itself; v1 is purely a producer.

**Honesty about limits**

- R11. The skill description in `SKILL.md` states plainly that the skill catches paraphrase gaps in
  the author's own pattern and does not prove production discoverability. The acknowledged
  limitation is that candidate queries are generated by the same agent that reads the pattern body,
  which introduces a paraphrase bias. Reaching every brainstormed query in the top 5 is **necessary
  but not sufficient** for production discoverability — treat it as a baseline check, not a quality
  gate. Removing the bias is a followup, not a v1 goal.

## Success Criteria

- A pattern author can run `/lore:coverage-check rust/cargo-deny.md` against a small new pattern and
  receive a coverage report plus actionable edit suggestions in **a few minutes** on a warm Ollama
  with parallel query dispatch. Cold-start runs and CPU-only hosts may take longer; the skill
  prioritises correctness over latency.
- Across one or two iterations, the skill **closes gaps the author would otherwise have missed** —
  concretely, the author exits the loop with at least one accepted edit suggestion that surfaces a
  previously-absent query. Reaching every query in the top 5 is a useful baseline but is not the
  success metric, because hitting it with paraphrased queries is the symptom of paraphrase bias, not
  proof the skill works.
- The skill leaves no metadata footprint on the pattern file or its repository. Diffs after a
  session are limited to author-approved edits to the body and frontmatter of the target file. The
  local JSONL log under `~/.cache/lore/qa-sessions/` is outside the pattern repository and does not
  count against this criterion.
- The skill is discoverable in the slash command list under the lore plugin namespace and runs
  without additional configuration in any session whose configured `knowledge_dir` contains the
  target file.
- The edit-suggestion feature (R7) is justified by the goal of reducing the manual loop's friction
  enough that authors actually run it. A skill that only reports gaps without proposing edits would
  still require the author to manually figure out _which_ term to add and _where_ — preserving most
  of the friction the brainstorm exists to remove. R7 is what turns the skill from a linter into an
  assistant.

## Scope Boundaries

- **Minimal Rust change scoped to one MCP handler.** v1's only Rust addition is attaching a
  `text_response_with_metadata` block to `src/server.rs::handle_search` so the skill can consume
  structured per-row data (`rank`, `source_file`, `score`, `mode`) instead of parsing the prose body
  of the response. This mirrors the established pattern already used by `lore_status`,
  `add_pattern`, `update_pattern`, and `append_to_pattern` in the same file. No new CLI subcommand,
  no new MCP tool, no other library code changes. The skill uses `lore ingest --file` (Bash) and
  `lore_status` (MCP) exactly as they ship today.
- **No persisted query metadata in the pattern repository.** No `qa_queries` frontmatter field, no
  sidecar files in the pattern repo, no schema commitments to the pattern format. Every invocation
  regenerates the query set from scratch. (The local JSONL log under `~/.cache/lore/qa-sessions/`
  lives outside any pattern repository and is a separate category — see R10.)
- **No CI integration.** No `--check` mode, no GitHub Action, no exit-1 on coverage failure. The
  skill is interactive only.
- **No bias mitigations in v1.** No hidden-body query generation, no required author seed queries,
  no anti-queries (`must_not_surface`). The paraphrase bias is acknowledged in the skill description
  and addressed in followups.
- **No cross-project invocation.** The skill must run in a session whose configured `knowledge_dir`
  contains the target file. Running it from a consumer project against a pattern in another
  repository is out of scope and depends on MCP-level support for multiple knowledge directories.
  R1a's pre-flight check exists to detect and refuse this case with a clear message rather than
  failing opaquely.
- **No production trace logging.** The skill does not capture which queries fired in production
  sessions, which patterns surfaced for agent-typed queries, or any session telemetry beyond the
  local JSONL log of its own invocations.
- **No Bash fallback for search.** R5 commits to the MCP transport. Adding a Bash fallback would
  require parsing prose output from `lore search` and would create a code path the author has no way
  to verify is taken. Failing loudly when MCP is unavailable is the preferred failure mode for v1.
- **Single Claude Code skill deliverable.** The skill itself is one file:
  `integrations/claude-code/skills/coverage-check/SKILL.md`. No supporting scripts, no schemas, no
  companion files in the skill directory. The Rust change to `handle_search` is in `src/server.rs`
  and is not part of the skill directory.
- **No per-query timeout in v1.** R5 waits for all parallel queries to settle. Adding a per-query
  timeout is a planning-time optimisation if real Ollama latency demands it.

## Key Decisions

- **Skill packaging: pure markdown Claude Code skill plus one minimal Rust change.** Lowest cost
  path that produces a non-fragile result. Ships in one pull request, commits to no schema for the
  pattern repository, allows immediate productivity value while richer mechanisms are designed in
  followups.
- **One Rust change: structured metadata on `handle_search`.** The original "no Rust changes"
  boundary held until the second-pass review verified the actual `search_patterns` MCP response
  shape, which returns prose only — no per-row rank, no `source_file`, no `mode` indicator, and no
  machine-readable signal that the embedder fell back to FTS-only when Ollama was unreachable.
  Parsing prose in the skill prompt would be fragile by design: the next change to `handle_search`'s
  output format would silently break the skill, and the FTS-fallback case is undetectable without
  string-matching a sentinel prefix. The cleaner answer is to attach a structured metadata block to
  the search response, mirroring the pattern that `lore_status`, `add_pattern`, `update_pattern`,
  and `append_to_pattern` already use in the same file. The change is small (an estimated 50-100 LOC
  of Rust, follows an established pattern, no new dependencies, no new tests of consequence), and it
  makes the skill testable and the eventual `lore qa --json` (followup 1) trivial because the data
  shape is already correct. v1 ships the Rust change and the markdown skill in the same pull
  request.
- **Skill name: `coverage-check`, not `pattern-qa`.** Plugin-qualified as `lore:coverage-check`. v1
  is a coverage check that catches paraphrase gaps; it does not prove production discoverability.
  Naming it `pattern-qa` would squat the most strategically important slot in the product surface
  and force a future rename when the deterministic-helper-backed version (followup 1) lands.
  `pattern-qa` is reserved for the version that earns it.
- **Plugin-wide naming convention: bare names plus plugin namespace.** Skills inside the lore plugin
  are named by their function alone, without a `lore-` in-name prefix. Claude Code's automatic
  plugin namespace (`lore:<skill-name>`) handles disambiguation when needed. The convention is
  documented in the plugin README. The existing `search-lore` skill is renamed to `search` in the
  same pull request as this skill.
- **Transport: Bash for ingest, MCP-only for search.** `lore ingest
  --file` has no MCP equivalent
  today (the `reindex_file` tool is captured as a P2 followup), so the skill shells out via Bash for
  ingestion. Search uses the existing `search_patterns` MCP tool (with the structured-metadata
  addition above) and no fallback — see Scope Boundaries for the rationale.
- **Pre-flight knowledge_dir validation as a fast-path heuristic.** R1a's `lore_status` check is the
  lightest-weight way to convert an opaque `validate_within_dir` failure into an actionable error
  message before the skill commits to any embedder work. The check costs one MCP call and is a
  heuristic, not a guarantee — the authoritative containment check remains R4's `lore ingest --file`
  invocation, which will halt the skill on any pre-flight/ingest canonicalisation mismatch
  (symlinks, case sensitivity, trailing slashes). Pre-flight exists to make the common case fast and
  clear; R4's halt catches the edge cases with a less friendly error message.
- **Loop termination: stability OR ceiling, mechanical comparison.** R8 commits to a concrete
  predicate (surfaced-query set unchanged from previous iteration) plus a hard ceiling of three
  iterations per invocation, with the exit path surfaced in the final report. The previous
  iteration's surfaced-query set is rendered as a sorted code-fenced list in the chat report each
  cycle, so the skill diffs strings rather than making a judgment call about set equality. The
  ceiling resets on re-invocation; an author iterating on a difficult pattern can run
  `/lore:coverage-check` again to start a fresh counter. Ceiling exists to prevent oscillation, not
  to cap effort.
- **Pattern-repo session only.** The skill assumes the configured `knowledge_dir` contains the
  target file. Cross-project invocation requires MCP changes that are out of scope for v1.
- **Acknowledged paraphrase bias, with sharpened framing.** The skill is positioned as a coverage
  check that catches paraphrase gaps, not a quality gate. The `SKILL.md` description states the
  limitation explicitly and reframes 100 percent coverage as necessary but not sufficient for
  production discoverability.
- **Design constraint: prompts and report format must be replaceable by `lore qa --json` output.**
  When the deterministic Rust subcommand (followup 1) lands, the architectural question "does QA
  live in the skill with the CLI as a backend, or does QA live in the CLI with the skill as a thin
  invocation wrapper?" should be answerable in favour of the latter without rewriting the skill from
  scratch. v1's query-brainstorming prompt, search dispatch, and report rendering should therefore
  be structured so each piece can be replaced by a single `lore qa --json` invocation that returns
  the same shape. This is not a v1 feature; it is a v1 design constraint that costs nothing to
  honour now and prevents an architectural lock-in later.
- **Canonical JSON shape (sketch) for the v1 report and the future `lore qa --json`.** The
  replaceability design constraint is operationalised by sketching the canonical shape inline so v1
  and followup 1 share a contract:

  ```
  {
    queries: [
      {
        query: string,
        status: 'surfaced' | 'not_present' | 'errored' | 'degraded',
        rank: int | null,
        error: string | null,
        mode: 'hybrid' | 'fts_fallback' | 'fts_only' | null
      }
    ],
    coverage_ratio: float,    // computed against hybrid-mode only
    errored_count: int,
    degraded_count: int,
    exit_reason: 'converged' | 'ceiling',
    iterations: int
  }
  ```

  v1's skill renders this shape prosaically in chat and appends it verbatim to the local JSONL log
  (R10). Followup 1's `lore qa --json` emits the same shape as JSON to stdout. Both reference the
  same fields by the same names. This is a stub, not a wire protocol — planning may adjust field
  names — but locking the shape now prevents the divergence that adversarial review flagged as a
  real risk.
- **Local JSONL log as a compounding-direction concession.** R10's
  `~/.cache/lore/qa-sessions/<timestamp>.jsonl` log is the smallest possible concession to the
  otherwise legitimate concern that v1 generates valuable signal and discards all of it. The log
  lives outside any pattern repository (so it does not violate the "no persisted metadata in the
  pattern repo" boundary), is opt-out via `LORE_NO_QA_LOG=1`, and is never read by v1 itself. Its
  sole purpose is to give followups 1, 2, and 4 real session data to ground their design choices in
  when they come to be designed.

## Dependencies / Assumptions

- Single-file ingest (`lore ingest --file`) is available. Shipped in PR #31.
- The `search_patterns` MCP tool is available in the session. Shipped with the lore plugin's bundled
  MCP server.
- The structured metadata addition to `src/server.rs::handle_search` ships in the same pull request
  as this skill. The change attaches a `text_response_with_metadata` block to the search response
  containing per-row `rank`, `source_file`, `score`, and a top-level `mode` field. It does not
  change the prose body of the response, so existing consumers of `search_patterns` (none today
  besides this skill) are unaffected. Implementation cost is small and follows the established
  pattern in the same file (mirror the way `lore_status` and the write tools already attach
  metadata).
- The `lore_status` MCP tool returns the configured `knowledge_dir` path (verified in
  `src/server.rs`). This is the dependency R1a's pre-flight check rests on.
- The lore plugin is configured for a knowledge directory that contains the target pattern file.
  R1a's check makes this dependency enforceable for the common case rather than merely assumed; R4's
  halt-on-error catches the edge cases.
- The rename of `search-lore` to `search` (plus the seven prose reference updates and the plugin
  README convention note) ships in the same pull request as this skill, not as a precursor PR. There
  are no users of the lore plugin yet, so the breaking-change cost of renaming the existing skill is
  zero.
- The `lore` CLI binary is on `PATH` in the shell environment Claude Code uses for Bash tool calls.
  Plugin installation does not guarantee this; users may need to install the CLI separately. R1b
  enforces this as a pre-flight check rather than letting it surface as an opaque "command not
  found" mid-loop.
- `lore ingest --file` is a full chunk replacement for the target file, not an append. Verified by
  `tests/single_file_ingest.rs::re_ingesting_same_file_replaces_chunks_without_duplication`. The
  coverage loop depends on this contract — any change that broke it would silently corrupt iteration
  N+1's coverage report by mixing chunks from prior versions of the file with chunks from the
  current working tree.

## Followup Work (Out of Scope for v1)

The brainstorm conversation surfaced several richer mechanisms that are deliberately deferred. Each
one should land as its own document under `docs/todos/` after this brainstorm completes, so the
institutional context is not lost.

1. **`lore qa` deterministic Rust subcommand.** A subcommand that produces the canonical JSON
   coverage report shape (see Key Decisions) for a single pattern file, so the skill (and any future
   automation) can replace its in-prompt orchestration with a single Bash invocation. The structured
   metadata change to `handle_search` (shipped in this pull request) is the foundation: `lore qa`
   will reuse the same per-row rank/source/mode data that the skill consumes via MCP. The
   `pattern-qa` skill name is reserved for the deterministic-helper-backed version that pairs with
   this subcommand. Unlocks several of the items below.
2. **Persisted `qa_queries` metadata schema in the pattern repository.** Promotes the QA query set
   to first-class metadata so it survives across sessions and becomes testable in CI. Storage choice
   (frontmatter field versus sidecar file) deferred until there is a consumer to validate the
   trade-offs against. The local JSONL log from R10 is the precursor data set this followup will
   ground its design in.
3. **Pattern repository CI mode (`lore qa --check`).** Reads persisted query metadata and verifies
   every pattern still surfaces for the queries it claims to satisfy. Requires the lore release
   process to ship first so pattern repository maintainers can install lore in GitHub Actions in
   seconds rather than building from source. Tied to the existing roadmap item for prebuilt binaries
   via `cargo-zigbuild`.
4. **Bias mitigations.** Hidden-body query generation, required author seed queries, and
   anti-queries (`must_not_surface`). Promotes the skill from coverage check to discoverability
   gate. Worth doing once a deterministic helper exists to enforce the contract, with the JSONL log
   from R10 providing real session data to calibrate against.
5. **Production hook trace as ground truth.** Opt-in logging of real PreToolUse-generated queries
   and which patterns surfaced. The empirical version of QA, anchored in observed agent behaviour
   rather than synthetic queries. Larger initiative; deserves its own brainstorm.
6. **MCP `reindex_file` tool.** Already captured as `docs/todos/mcp-reindex-file-tool.md` from PR
   #31 ce-review. Lets the skill avoid shelling out via Bash for ingestion and stay inside MCP. The
   Bash-for-ingest path in v1 is interim, not a settled architectural choice.
7. **Cross-project invocation.** Allow the skill to run from a consumer project against a pattern in
   another repository. Requires MCP tools to accept a `knowledge_dir` parameter or to support
   multiple open knowledge databases per server. Tied to followup 6 since both require MCP plumbing
   changes.

## Outstanding Questions

### Resolve Before Planning

_(none)_

### Deferred to Planning

- [Affects R3][Technical] Exact wording of the skill prompt that elicits high-quality candidate
  queries. Planning will iterate on this against real patterns from `lore-patterns/`.
- [Affects R4][Technical] Whether the skill should pass `--force` by default to override
  `.loreignore` exclusions for in-progress drafts that live in an ignored staging directory, or
  whether it should refuse and ask the author to remove the ignore first. Both are defensible; pick
  one in planning so behaviour is predictable. Note that R4's halt-on-non-zero clause does **not**
  fire on a `.loreignore` skip — `lore ingest --file` exits 0 with no chunks indexed in that case
  (verified at `src/main.rs`), so planning must close this hole explicitly.

## Next Steps

→ `/ce:plan` for structured implementation planning. The naming- convention rename, the structured
metadata addition to `handle_search`, and the new skill all ship in the same pull request, so there
is no precursor work blocking planning.

## Implementation note (2026-04-07)

The structured metadata channel described in this brainstorm as "the skill reads `result.metadata`
directly instead of parsing the prose body" turned out to be unreachable from inside Claude Code —
the MCP client strips the `metadata` sibling from `result` before forwarding tool responses to the
agent. Real-run testing during PR #32 surfaced this, and the implementation pivoted mid-PR to a
different transport channel: a fenced `lore-metadata` code block embedded in `content[0].text`,
gated behind an opt-in `include_metadata: bool` tool parameter.

The pivot preserves every other design decision in this brainstorm — the three-value `mode` enum,
the canonical JSON shape, the opt-in principle, the four-state per-query classification, the
fail-fast cascade detection, the degraded-mode refusal rule, the iteration-loop stability check via
Bash diff, and the ephemeral JSONL session log. Only the transport channel for the structured
metadata changed. The agent still reads the same field names from the same JSON shape; it just
extracts them from a fenced block in the prose body instead of a sibling field on `result`.

See the plan's "Design pivot: layer 2 finding" section
(`docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md`) for the detailed diagnostic
walk-through and the new learning at
`docs/solutions/best-practices/mcp-metadata-via-fenced-content-block-2026-04-07.md` for the
production pattern that future MCP tool designs should follow.
