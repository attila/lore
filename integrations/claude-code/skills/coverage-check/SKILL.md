---
name: coverage-check
description: Audit a pattern file's vocabulary coverage by simulating the PreToolUse hook's query extraction on synthetic tool calls, ingesting the file, and reporting which queries surface it. Catches paraphrase gaps in pattern wording. Not a quality gate — see the limit disclosure below.
disable-model-invocation: true
user-invocable: true
---

# Coverage Check

Audit the vocabulary coverage of a single pattern file by automating the manual Vocabulary Coverage
Technique from `docs/pattern-authoring-guide.md`. Infer 3-6 synthetic tool calls an agent would
plausibly make when this pattern applies, pipe each one through `lore extract-queries` to
materialize the exact FTS5 query the PreToolUse hook would inject for it, ingest the working-tree
file, search for each query in parallel, score per-query coverage, suggest concrete edits to close
gaps, and iterate until convergence.

Invoke as `/lore:coverage-check <pattern-file-path>` with a path relative to the configured
`knowledge_dir` (the pattern repository root that lore is configured to index — see step 2 for how
the skill resolves this from the `lore_status` MCP tool). The path argument is `$ARGUMENTS`.

**Quoting discipline (do not skip):** every Bash invocation in this skill that interpolates
`$ARGUMENTS`, the canonical target path, or any agent-derived value MUST quote the value with double
quotes. The existing `search` skill demonstrates the convention: `lore search "$ARGUMENTS"`
(quoted). The same applies here. A target path like `foo'; rm -rf ~ #.md` must be passed to Bash as
`lore ingest --file "foo'; rm -rf ~ #.md"` so the shell treats it as a single argument, not a
command sequence. The skill steps below show the quoted form explicitly; do not strip the quotes
when expanding the templates.

## Purpose and limit (read this first)

This skill catches **vocabulary gaps** between a pattern's wording and the queries the lore
PreToolUse hook actually synthesizes from agent tool calls. It is not a production coverage proof,
but it is stronger than a paraphrase check: the candidate queries are produced by the same
`extract_query` code path the hook uses at runtime, not by the LLM paraphrasing the pattern body.
Residual bias comes from the tool-call inference step — the agent picks which synthetic tool calls
to simulate based on the pattern's tags, headings, and code fences — but the query strings
themselves are deterministic hook output. Treat the report as a strong baseline check, not a quality
gate.

## Interaction hazard (do not skip)

While iterating with this skill, **never run plain `lore ingest` between iterations**. Use
`lore ingest --file` (which this skill calls automatically) instead. The reason is the composition
cascade documented in
`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`:
walk-based delta ingest can wipe single-file-ingested chunks for a file that was `git rm`'d in a
prior commit. The skill includes an active detection step (see step 7) that will abort the iteration
if it notices the chunks it just ingested have disappeared, but the safest workflow is to not run
plain `lore ingest` at all while this skill is iterating.

## 1. Pre-flight: `lore` binary on PATH (R1b)

Run `command -v lore` via Bash. If it exits non-zero, halt with:

> Coverage check requires the `lore` CLI on PATH. Plugin installation does not guarantee this —
> install lore separately (see the project README) and retry.

Do not proceed to step 2.

## 2. Pre-flight: `knowledge_dir` containment (R1, R1a)

This is a fast-path heuristic, not a guarantee. The authoritative containment check is
`validate_within_dir` inside `lore ingest --file`'s Rust code (see step 6). Pre-flight exists to
fail fast on the obvious cases before any embedder work.

> **Why the `include_metadata: true` parameter?** Claude Code's MCP client strips the
> `result.metadata` sibling from tool responses before surfacing them to the agent — only the
> `content[]` array reaches the model. Lore's MCP tools therefore carry structured metadata in a
> fenced `lore-metadata` code block embedded in `content[0].text` rather than as a sibling field on
> `result`. The fenced block is opt-in because it bloats the response for callers that only need the
> prose body. See
> `docs/solutions/best-practices/mcp-metadata-via-fenced-content-block-2026-04-07.md` for the
> design.

Steps:

1. Detect the platform once with `uname -s`. On Linux, use `readlink -f` for canonical path
   resolution. On Darwin (macOS) or BSD, use
   `python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$1"` as a portable fallback
   because GNU `readlink -f` is not present on those systems by default. Always quote the path
   argument when invoking either form.
2. Call the `lore_status` MCP tool with `{"include_metadata": true}`. The `include_metadata`
   parameter is **required** — without it, the server returns only the prose summary, and the
   structured `knowledge_dir` field the skill needs for deterministic path resolution is
   inaccessible from inside Claude Code's MCP client (see the background note below).
3. Extract the structured metadata from the tool response. The response's `content[0].text` ends
   with a fenced `lore-metadata` code block containing JSON. The opening marker is a blank line
   followed by a triple-backtick fence with language tag `lore-metadata`; the closing marker is a
   triple-backtick fence on its own line. Find the last opening marker in the response text, advance
   past it, read forward until the next closing marker, and parse the intervening text as JSON. The
   parsed object exposes the configured `knowledge_dir` at the top level along with
   `git_repository`, `chunks_indexed`, `loreignore_active`, and other status fields. Read
   `knowledge_dir` (a string, the configured knowledge directory). This value may be either an
   absolute path or a path relative to lore's configuration file location; the canonicalisation step
   below resolves both cases.
4. Resolve `$ARGUMENTS` against the configured `knowledge_dir`, NOT against the agent session's
   current working directory. The recipe must be deterministic so the skill's pre-flight error
   messages name the same path that R4's authoritative `validate_within_dir` check will see.

   1. Canonicalise the configured `knowledge_dir` (call it `KD`).
      - If the extracted `knowledge_dir` is absolute (starts with `/`), pass it through the
        canonicalisation command directly.
      - If it is relative, **halt** with: "Coverage check halted: lore_status returned a relative
        `knowledge_dir` (`<value>`), but R1a cannot determine the correct anchor to resolve it
        against. Reconfigure lore to use an absolute `knowledge_dir` in `~/.config/lore/lore.toml`
        and retry." A relative `knowledge_dir` is rare and deterministically refusing it is safer
        than guessing an anchor that may differ from what `lore ingest --file` will use at R4 time.
   2. If `$ARGUMENTS` is absolute, canonicalise it directly (call it `T`).
   3. If `$ARGUMENTS` is relative, join it onto `KD` (the canonicalised, absolute `knowledge_dir`)
      with a path separator, then canonicalise the joined path. Concretely on Linux:
      `T="$(readlink -f -- "$KD/$ARGUMENTS")"`. On macOS/BSD:
      `T="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' -- "$KD/$ARGUMENTS")"`.
      This guarantees the target resolves against the pattern repository, not the agent's current
      working directory — the agent may be invoked from any folder.
5. Check that the canonical target path `T` starts with the canonical `knowledge_dir` `KD` followed
   by `/` (string-prefix check, with the trailing `/` so that `KD = /foo` does not match
   `T = /foobar/x.md`).
6. If the check fails, halt with:

   > Coverage check requires the target file to live inside the configured `knowledge_dir`.
   >
   > Configured `knowledge_dir`: `<canonical-knowledge_dir>` Target file (canonical):
   > `<canonical-target>`
   >
   > Move the file inside the `knowledge_dir`, change the lore configuration to point at the
   > target's parent directory, or invoke the skill from a session whose lore configuration covers
   > the file.

   Do not proceed.

## 3. Read the target pattern file

Read the full body of the target pattern file via the file-reading tool (e.g. Read in Claude Code).
Hold the body content in working memory for the brainstorm step.

## 4. Derive candidate queries from synthetic tool calls (R3)

Produce the candidate query set by simulating the lore PreToolUse hook's own query extraction on
synthetic tool calls, not by paraphrasing the pattern body. The goal is 3-6 production-realistic
queries that are byte-for-byte identical to what the hook would synthesize if a working agent
actually issued those tool calls. Two sources feed the tool-call set:

### Source A: `qa_simulations` frontmatter override (opt-in)

If the target pattern's frontmatter contains a `qa_simulations` list, use it verbatim and skip
inference. Each entry is a `{tool_name, tool_input}` object matching Claude Code's tool-call shape.
Example:

```yaml
qa_simulations:
  - tool_name: Edit
    tool_input:
      file_path: Cargo.toml
  - tool_name: Bash
    tool_input:
      command: cargo deny check
```

Use this when the automatic inference below cannot find good signals (unusual patterns, patterns
about workflows rather than code) or when you want to pin a specific simulation set for
reproducibility across runs.

### Source B: Automatic inference (default)

Inspect the pattern's `tags:` frontmatter, headings, fenced code blocks, and concrete filenames
mentioned in the body. Construct 3-6 `{tool_name, tool_input}` objects that an agent would plausibly
issue when this pattern applies. Use the following tag-driven heuristics as a starting point:

| Tag signal                  | Likely tool calls                                                      |
| --------------------------- | ---------------------------------------------------------------------- |
| `rust`, `cargo`             | `Edit Cargo.toml`, `Edit src/<file>.rs`, `Bash cargo <verb-from-body>` |
| `typescript`, `pnpm`, `npm` | `Edit package.json`, `Edit src/<file>.ts`, `Bash pnpm <verb>`          |
| `python`, `uv`, `pytest`    | `Edit pyproject.toml`, `Edit <file>.py`, `Bash pytest`                 |
| `ruby`, `rails`             | `Edit app/<path>.rb`, `Edit db/migrate/<file>.rb`, `Bash bundle exec`  |
| `ci`, `github-actions`      | `Edit .github/workflows/<file>.yml`                                    |
| `sqlite`, `sql`             | `Edit <file>.sql`, `Bash sqlite3 <args>`                               |
| `git`                       | `Bash git <verb-from-body>`                                            |
| `yaml`, `yml`               | `Edit <file>.yml`                                                      |
| `testing`                   | `Edit <test-file-matching-language>`, `Bash <test-runner>`             |

Concrete filenames named in the pattern body (e.g. `deny.toml`, `justfile`, `rust-toolchain.toml`)
take precedence over inferred ones — add an `Edit <that-file>` call verbatim. Verbs named in fenced
code blocks (e.g. `cargo deny check` in a fenced sh block) should become `Bash` calls with that
exact command.

### Confirmation

Render the constructed tool-call list to chat as a numbered list and ask the author to confirm,
edit, or replace it before running extraction:

> I'll simulate these tool calls to derive candidate queries:
>
> 1. `Edit Cargo.toml`
> 2. `Edit deny.toml`
> 3. `Bash cargo deny check`
>
> Confirm, edit, or replace before I run extraction. (y / edited list / skip)

### Extraction via `lore extract-queries`

For each confirmed tool call, pipe a thin JSON envelope through `lore extract-queries`:

```sh
echo '{"tool_name":"Edit","tool_input":{"file_path":"Cargo.toml"}}' \
  | lore extract-queries
```

The subcommand reads the envelope, runs the same `extract_query` logic as the PreToolUse hook, and
prints the resulting FTS5 query to stdout (or emits nothing if no terms survive cleaning). Collect
each non-empty stdout line as one candidate query.

**Empty stdout is diagnostic, not an error.** It means the hook would not inject any pattern context
for that tool call at all — discoverability via that route is structurally zero. Examples:
`Bash just ci` (`just` is a stop-word, `ci` is < 3 chars) or `Bash gh pr view` (similarly stripped).
If any of your inferred tool calls return empty, record the fact and move on — the remaining
non-empty queries form the candidate set.

### Degenerate case: zero candidate queries

If **every** tool call yields empty output, halt with:

> Coverage check halted: none of the inferred (or `qa_simulations`-specified) tool calls produce
> FTS5 queries after hook-style term cleaning. This pattern's production-time discoverability via
> the PreToolUse hook is structurally weak for these tool calls. Either add a `qa_simulations`
> frontmatter entry with richer tool calls, or reword the pattern's tags / headings / code fences so
> automatic inference finds stronger signals.

### Render the materialized query set

Print the final non-empty query set to chat — one query per line — before step 5 so the author sees
the exact strings that will be searched. Unlike v0's LLM-brainstormed list, these are **hook
output**: byte-for-byte what the agent session would see when it triggered the corresponding tool
calls.

## 5. Refresh the index, with .loreignore detect-then-prompt (R4)

Run `lore ingest --file <target>` via Bash, **without** `--force` initially. Capture the exit code.

- **Exit non-zero:** halt with the verbatim error message and the prefix "Coverage check halted:
  `lore ingest --file` failed.".
- **Exit zero with chunks indexed:** proceed to step 6.
- **Exit zero with zero chunks indexed:** the file was silently skipped by `.loreignore`. Detect
  this by calling `search_patterns` once with a distinctive token from the file body and checking
  whether the target's `source_file` appears in `metadata.results`. If it does not, the file is
  `.loreignore`-excluded.

  Prompt the author with explicit consequences (do not abbreviate this prompt — the author needs to
  understand what `--force` actually does before saying yes):

  > `<target>` is excluded by `.loreignore`. Forcing the ingest will write the file's chunks into
  > the local search index, where they will surface in any subsequent `search_patterns` call from
  > this skill, the `search` skill, or the PreToolUse hooks until the next walk-based `lore ingest`
  > re-applies `.loreignore` (which deletes the chunks again). If this draft contains secrets,
  > internal hostnames, customer names, or other content you parked in `.loreignore` for a reason,
  > decline now and remove the file from `.loreignore` first if you still want to coverage-check it.
  > Continue with `--force`? (y/N)

  - On `y`: re-run `lore ingest --file --force <target>`. If the second run still produces zero
    chunks (verified by the same detection check), halt with "Coverage check halted: file remains
    unindexed after `--force`. Investigate the ingest pipeline."
  - On `N` (or empty): exit cleanly with "Coverage check skipped: file is `.loreignore`-excluded and
    the author declined to bypass."

## 6. Cascade detection and search-mode pre-flight (do not skip)

Before the main parallel search batch, issue **one extra** `search_patterns` call with a query
constructed from a distinctive token in the pattern body — the agent picks a unique-looking word
from the body that satisfies the FTS5 rubric (≥3 alphabetic characters, not a stop-word). **The call
must pass `include_metadata: true`** in the arguments so the response carries the fenced
`lore-metadata` block the skill reads below.

```json
{
  "name": "search_patterns",
  "arguments": {
    "query": "<distinctive token from body>",
    "top_k": 5,
    "include_metadata": true
  }
}
```

This single call serves two purposes that both produce loud aborts on failure.

**(a) Search-mode pre-flight.** Extract the fenced `lore-metadata` JSON from the response's
`content[0].text` using the same extraction recipe described in step 2 (locate the last opening
triple-backtick fence with language tag `lore-metadata`, advance past it, read until the next
closing triple-backtick fence, parse as JSON). Read the `mode` field from the parsed object. The
lore MCP server reports one of three values:

- **`"hybrid"`** — full hybrid search (Ollama embedder + FTS combined via reciprocal-rank fusion).
  Proceed to (b) below.
- **`"fts_only"`** — the lore deployment is configured for FTS-only search via
  `config.search.hybrid = false` in the lore configuration file. The embedder was never attempted.
  Halt the iteration immediately with:

  > Coverage check halted: lore is configured for FTS-only search
  > (`config.search.hybrid =
  > false` in `~/.config/lore/lore.toml`). The coverage-check skill
  > requires hybrid mode (Ollama embedder + FTS combined via reciprocal-rank fusion) because
  > FTS-only ranks (BM25) are not comparable to hybrid ranks across queries — the per-query coverage
  > scoring would be meaningless. Set `hybrid = true` in `~/.config/lore/lore.toml` and retry.
- **`"fts_fallback"`** — the lore deployment is configured for hybrid mode but the embedder (Ollama)
  was unreachable for this query, so the search fell back to FTS-only. The parallel query batch in
  step 7 would also fall back, producing a fully-degraded report. Halt the iteration immediately
  with:

  > Coverage check halted: the lore embedder (Ollama) is unreachable. The cascade-detection query
  > fell back to FTS-only, and the parallel coverage queries that follow would do the same —
  > producing a report whose ranks are not comparable across queries. Restart Ollama and retry the
  > skill.

These two early aborts catch deployment-misconfiguration and embedder-unavailability **once** at
step 6 instead of repeating the diagnosis 5-12 times in step 7's parallel batch and then refusing
the coverage ratio at step 9. Step 9's refusal logic remains as a fallback for the edge case where
the embedder fails _partway through_ the parallel batch (after passing cascade detection).

**(b) Cascade detection.** With `mode == "hybrid"` confirmed, read the `results` array from the same
parsed metadata object and verify that at least one row's `source_file` matches the canonical target
path (relative to `knowledge_dir`). If none does, halt the iteration with:

> Coverage check halted: index state changed unexpectedly between ingest and search. Check for
> parallel `lore ingest` processes (file watcher, pre-commit hook, second Claude Code session). The
> composition cascade documented in
> `docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`
> may have fired. Stop the parallel writer and re-invoke the skill.

This single extra MCP call converts three silent failures (deployment misconfiguration, embedder
unavailability, composition cascade) into three distinct loud aborts. Do not skip it; the cost is
one query per iteration and the benefit is a clear next-action message in each case instead of a
misleading coverage report.

## 7. Parallel search (R5)

Issue all candidate queries from step 4 through the `search_patterns` MCP tool with `top_k = 5` and
**`include_metadata: true`** on every call. Issue them in parallel (multiple tool calls per
assistant turn) so the wall-clock cost approaches the latency of one query rather than N sequential
queries.

```json
{
  "name": "search_patterns",
  "arguments": {
    "query": "<candidate query from step 4>",
    "top_k": 5,
    "include_metadata": true
  }
}
```

**Wait for all parallel calls to settle** before scoring per-query state in step 8. v1 commits no
per-query timeout — one slow Ollama query holds up the report. There is no Bash fallback: if
`search_patterns` is unavailable, halt with "Coverage check halted: `search_patterns` MCP tool
unavailable. Restart the Claude Code session."

## 8. Score per-query state (R6)

For each query response, extract the fenced `lore-metadata` JSON from `content[0].text` using the
same extraction recipe described in step 2 and applied in step 6. Then classify the target pattern
into exactly one of four states:

- **errored: `<reason>`** — the JSON-RPC `error` field on the response is non-null. Read the error
  message and surface it. When the error field is set, there is no `content` array and no fenced
  block to parse; branch on `resp["error"].is_null()` first.
- **degraded: fts_fallback** — the response succeeded but the extracted metadata's `mode` field is
  `"fts_fallback"`. The embedder was unreachable for this individual query, falling back to FTS-only
  mid-batch. This state is **rare in practice** because step 6's pre-flight already aborted the
  iteration if the cascade-detection query reported `fts_fallback`. It can still happen if the
  embedder fails partway through the parallel batch (e.g. Ollama hits a request budget mid-batch).
  When it does, hybrid-mode rank is not comparable to FTS-only rank, so coverage cannot be computed
  for this query. Note: `mode == "fts_only"` is **structurally impossible** at this step because
  step 6 catches the FTS-only deployment and aborts the iteration before the parallel batch ever
  runs.
- **surfaced (rank: N)** — the target's canonical `source_file` (relative to `knowledge_dir`)
  appears in any row of the extracted metadata's `results` array. Compute N as the **minimum**
  `rank` value across all matching rows (a single source file may produce multiple chunk rows; the
  lowest rank wins).
- **not_present** — no row's `source_file` matches the target.

## 9. Coverage ratio refusal on degraded queries

**If any query in this iteration is in the `degraded` state, do not compute or display a coverage
ratio.** Render the report listing per-query states (surfaced, not_present, errored, degraded) but
replace the ratio line with:

> Embedder was partially unavailable (`<N>` of `<M>` queries degraded). Coverage ratio is not
> computed because hybrid and FTS-only ranks are not comparable. Retry once Ollama is back.

Suggest no edits in this iteration. Skip to step 14 (exit reminder).

## 10. Render the coverage report (hybrid case)

Render a markdown coverage report in chat with four sections: **Surfaced**, **Not present**,
**Errored**, **Degraded** (the last is empty in the hybrid case thanks to step 9).

The **Surfaced** section MUST be a sorted markdown code-fenced list, one query per line, sorted
alphabetically:

```
query A
query B
query C
```

The sorted code-fenced list is the input to step 12's stability comparison. Do not paraphrase, do
not group, do not annotate the queries inside the code fence.

After the four sections, print:

- Coverage ratio: `<surfaced-count>` of `<successful-query-count>` queries (computed against
  successfully-executed hybrid queries only — guaranteed all queries hybrid by the step 9
  short-circuit)
- Errored count (if any), surfaced separately

## 11. Persist surfaced list to temp file

Write the sorted surfaced-query list (one query per line, no markdown formatting, no code fences)
to:

```
${XDG_RUNTIME_DIR:-/tmp/lore-$(id -u)}/lore/coverage-check/<session>-iter-<N>.txt
```

Where `<session>` is the same session identifier used for the JSONL log path in step 12 (see below)
and `<N>` is the current iteration number (1-indexed).

The runtime directory is **deliberately ephemeral**: `XDG_RUNTIME_DIR` is wiped on logout/reboot on
systemd systems, and the `/tmp/lore-$(id -u)` fallback is wiped at next boot on most distros. The
skill writes its iteration state and the JSONL session log under the same root so neither artefact
persists across reboots — pattern body content, brainstormed queries, and accepted edits never end
up in long-lived storage like `~/.cache` where backup tools or cloud sync would pick them up.
Same-session inspection still works: the files are readable for the lifetime of the session, just
not beyond.

Set `umask 077` at the start of the skill (immediately before the first `mkdir -p` in step 12) so
every file the skill creates is owner-readable only. The `/tmp/lore-$(id -u)` fallback is in a
shared world-readable parent (`/tmp`), so the umask plus an explicit `chmod 700` on the directory
itself (in the step 12 recipe) is what actually keeps other users on the same machine out.

Create the parent directory with `mkdir -p` if it does not exist. If the `mkdir` fails (EACCES on
read-only NFS, tmpfs out of space), abort the iteration with "Coverage check halted: cannot write
iteration state to `<path>`."

## 12. Append the JSONL session log (R10)

Pin the cache path with this exact shell recipe (the agent runs it once at session start, before
step 5). The `umask 077` at the top and the `chmod 700` on the runtime root are load-bearing — they
ensure every file and directory the skill creates is owner-readable only, even when the fallback
`/tmp/lore-$(id -u)` lands inside a world-readable `/tmp`:

```sh
umask 077
runtime_root="${XDG_RUNTIME_DIR:-/tmp/lore-$(id -u)}/lore/qa-sessions"
mkdir -p "$runtime_root" || { echo "ERROR: cannot create $runtime_root" >&2; exit 1; }
chmod 700 "$(dirname "$runtime_root")" "$runtime_root"
# Linux:
session_id="$(date -u +%Y%m%dT%H%M%S%N)-$$"
# macOS/BSD fallback (no %N support in BSD date):
# session_id="$(python3 -c 'import time, os; print(f"{int(time.time_ns())}-{os.getpid()}")')"
log_file="$runtime_root/$session_id.jsonl"
```

Use `uname -s` to pick the platform-appropriate `session_id` command. The PID suffix prevents
collisions between concurrent Claude Code sessions. The `mkdir -p` is idempotent. EACCES aborts the
iteration with a clear message.

After every iteration, append one JSON line to `$log_file` recording:

- `query_set`: the brainstormed candidate queries (array of strings)
- `outcomes`: per-query state objects matching the canonical shape from the plan's Key Decisions:
  `{query, status, rank?, error?, mode}`
- `accepted_edits`: edits the author accepted in this iteration (filled in after step 14)
- `exit_reason`: `converged`, `ceiling`, `cascade_detected`, `loreignore_skipped`, `degraded`, or
  `null` if iteration is ongoing
- `iteration`: 1-indexed iteration counter
- `coverage_ratio`: float, or `null` if step 9 fired

Skip the entire log step if `LORE_NO_QA_LOG=1` is set in the environment. The log is opt-out for
authors who want zero trace even within the session, but the default behaviour is already secure by
construction — the log lives in an ephemeral runtime directory that does not survive reboot and is
not picked up by backup or cloud sync tools. There is no need to set `LORE_NO_QA_LOG=1` to protect
against persistent leakage of pattern body content; that risk is structurally eliminated by the
runtime-directory location.

The log is never read by this skill. It exists for same-session debugging (the author can `cat` the
file mid-session to inspect what the skill recorded) and as a short-lived breadcrumb. Future
improvements that need a persistent corpus of session data must collect it through their own
explicit opt-in mechanism, not by reading this log.

This skill is **POSIX-only**. Native Windows is out of scope. WSL is supported through the
`/tmp/lore-$(id -u)` fallback because `XDG_RUNTIME_DIR` is not reliably set in WSL environments.

## 13. Suggest concrete edits to close gaps (R7)

For each query in the **Not present** section, propose **one** concrete edit:

- **Add a tag:** if the missing term is a noun or abbreviation that belongs in `tags:` frontmatter
- **Add a phrase to the body:** if the missing term is a verb or domain term that should appear in
  prose
- **Rephrase a heading:** if a heading uses an obscure term and a more agent-realistic word would
  surface the pattern
- **Extend the frontmatter:** if structured metadata (e.g., aliases) would help

Each suggestion must reference the specific missing term and where the edit should land (e.g., "add
`audit` to tags", "add the phrase 'supply chain audit' to the Cargo Deny section body").

## 14. Author decision and iteration loop (R8)

Ask the author: **accept all / accept some / skip**.

- **Accept all:** apply every suggested edit via the file-editing tool, then increment the iteration
  counter.
- **Accept some:** apply only the edits the author selected, then increment the iteration counter.
- **Skip:** exit cleanly with "no edits applied".

If any edits were applied:

1. If the iteration counter exceeds 3, halt with "Coverage check ceiling reached at 3 iterations.
   Remaining gaps: `<list>`. Re-invoke the skill to start a fresh counter — the ceiling exists to
   prevent pathological oscillation within one invocation, not to cap total effort."
2. Otherwise loop back to **step 5** (re-ingest), then through steps 6 (cascade detection), 7
   (parallel search), 8 (score), 9 (degraded refusal), 10 (render), 11 (persist), 13 (suggest).

After re-rendering and persisting the new sorted list at step 11 in the second and subsequent
iterations, **immediately** run:

```sh
runtime_root="${XDG_RUNTIME_DIR:-/tmp/lore-$(id -u)}/lore/coverage-check"
diff "$runtime_root/<session>-iter-<N-1>.txt" \
     "$runtime_root/<session>-iter-<N>.txt"
```

The `diff` exit code is the loop signal:

- **Exit 0:** the surfaced-query set is identical to the prior iteration's. Halt with "Coverage
  check converged: surfaced query set is stable."
- **Non-zero exit:** the set changed. Continue the loop (back to step 13 to suggest more edits, then
  step 14 to ask the author again).

The `diff`-via-temp-file mechanism is deliberate: comparing two sorted text files via Bash is
mechanically deterministic, whereas asking the agent to re-read and visually compare its prior chat
output is unreliable across context compaction, whitespace drift, and judgment-style "looks the
same" hallucination.

## 15. Exit and cleanup

After the loop terminates (converged, ceiling, skip, or any halt condition):

1. Print the recovery hint (R9):

   > If you discarded any of the iterated edits via `git checkout` or similar, run `lore ingest` to
   > reconcile the index against the current working tree. The skill's incremental ingests left the
   > index reflecting the working-tree state at each iteration; if you've reverted the working tree,
   > those chunks are now stale.

2. Clean up the per-iteration temp files:
   `rm -f "${XDG_RUNTIME_DIR:-/tmp/lore-$(id -u)}/lore/coverage-check/<session>-iter-"*.txt`. The
   JSONL log file is **not** explicitly cleaned up by this skill — it lives in the same ephemeral
   runtime root and is wiped automatically at next reboot or logout (depending on the platform), so
   no audit-trail accumulation problem exists.
