---
title: "feat: Accept language on MCP pattern-authoring tools"
type: feat
status: complete
created: 2026-05-19
completed: 2026-05-19
origin: tmp/mcp-language-arg-ce-plan-prompt.md
pr: https://github.com/attila/lore/pull/63
---

# feat: Accept `language` on `add_pattern` / `update_pattern` / `append_to_pattern`

## Summary

The MCP pattern-authoring tools silently drop a `language` argument today because their
`inputSchema`s do not declare the field. Agents that create or modify patterns via MCP cannot opt
patterns into the structural retrieval gate (PR #50); only hand-authored markdown frontmatter does.
This plan closes the gap on the MCP surface only: extend the two relevant tools' input schemas,
validate tokens through the same `is_known_token` predicate the ingest path uses, write canonical
`language:` frontmatter into the pattern file, and surface unknown-token advisories on both stderr
and the tool response's metadata fence.

The implementation rides on existing infrastructure — `parse_frontmatter_language_list` already
validates tokens and emits `MalformedLanguageEntry` advisories during chunking, and
`index_single_file` already aggregates them. The new work is at the MCP layer: schema, coercion,
frontmatter rendering, and advisory propagation into `WriteResult`.

---

## Problem Frame

PR #50 introduced the `language:` frontmatter field and the `language_json` column that drives the
U5 structural retrieval gate. PR #61 expanded the canonical `LANGUAGES` table to 27 entries. The
agent-authoring surface, however, was left behind:

- `add_pattern`, `update_pattern`, and `append_to_pattern` have no `language` field on their
  `inputSchema`.
- The handlers in `src/server.rs` do not read a `language` argument, and `ingest::add_pattern` /
  `update_pattern`'s `build_file_content` only renders `tags:` frontmatter, never `language:`.
- An agent that writes a Rust pattern via `add_pattern` produces a file with no `language:`
  frontmatter, which means the resulting chunk lands on the FTS-fallback path (R10), not the
  structural retrieval gate (R12) — the exact gap PR #50 was built to close.

The user-visible failure mode: MCP-authored patterns silently underperform on language-scoped
queries compared to hand-authored ones, with no agent-visible signal that the argument was dropped.

---

## Scope

### In scope

- Add a `language` field to the `inputSchema` of `add_pattern` and `update_pattern`. Accept both
  string and array-of-strings input.
- Plumb the value through `ingest::add_pattern` / `update_pattern` so `build_file_content` renders a
  canonical `language:` line into the file's frontmatter.
- Validate every passed token against `crate::engine::is_known_token`. Unknown tokens warn but
  proceed (R12).
- Surface unknown-token advisories on stderr (matches the existing ingest log line) and on the tool
  response's `lore-metadata` fence as a `language_warnings` array.
- Update tool descriptions to document the new field and the unknown-token policy.
- CHANGELOG entry; move the ROADMAP item from Up Next to Completed in the same diff.

### Deferred to Follow-Up Work

- Surfacing `language:` on the `list_patterns` MCP output (the `Pattern` row would need to carry the
  parsed token list). Tracked by the table-expansion follow-up.
- A `language_json` column read on the search-side metadata fence (today the column is only exposed
  via `lore_status`).

### Outside this product's identity

- **No new MCP tool.** The work is additive on three existing schemas.
- **No engine code change.** `is_known_token`, `LANGUAGES`, and `parse_frontmatter_language_list`
  stay as-is.
- **No language inference from body content.** The field is purely declarative; we do not
  heuristically guess a language from prose, code fences, or filenames passed via the title.
- **No DB schema migration.** `language_json` was populated by the chunking path on every write
  since PR #50; this work only changes what the chunking path sees in the on-disk frontmatter.
- **No change to `append_to_pattern`'s contract.** Append is body-only by definition; see Key
  Technical Decisions §3.

---

## Key Technical Decisions

### 1. Input shape: `oneOf` scalar or array, handler coerces

The frontmatter parser (`parse_frontmatter_language_list`) already accepts three YAML shapes —
scalar, flow list, block list. The MCP `inputSchema` will mirror this with JSON Schema `oneOf`:

```json
"language": {
  "oneOf": [
    { "type": "string" },
    { "type": "array", "items": { "type": "string" } }
  ],
  "description": "Optional language declaration..."
}
```

The handler coerces a scalar input to a one-element `Vec<String>` before further processing.
Rationale:

- Mirrors the YAML surface the manual authoring path supports — agents that round-trip a file
  through search → update will not need to reshape.
- The single-language case (the dominant case for new patterns) stays ergonomic — agents pass
  `"language": "rust"`, not `"language": ["rust"]`.
- Coercion happens once at the boundary; the rest of the pipeline (validation, frontmatter
  rendering) sees a uniform `&[String]`.

Rejected: array-only schema with no scalar. The cost of the slightly more permissive shape is a
five-line coercion in the handler; the upside is parity with the YAML surface and friendlier
ergonomics for the dominant case.

### 2. `update_pattern` language semantics: three-way matching `tags`

`update_pattern`'s `tags` argument already establishes a three-way semantics for frontmatter list
fields (`src/server.rs:875-885`, `src/ingest.rs:1253-1294`):

- **absent** → preserve the existing frontmatter list
- **`[]`** → clear
- **`[...]`** → replace wholesale

The `language` argument adopts the same three-way semantics for the same reason: an agent that calls
`update_pattern` to rewrite the body without re-supplying `language` should not silently de-language
the pattern (the analogue of the de-universalisation footgun that motivated `tags`'s
preserve-on-`None` semantics).

Rejected:

- **Merge (union with existing)**: cute but surprising — an agent that intentionally narrows
  `[swift, objectivec]` to `[swift]` cannot. Replace is the more honest primitive; agents that want
  merge can list-patterns first.
- **Set-if-absent**: silently ignoring a passed argument when the field is already set creates the
  same "dropped argument" failure mode this plan exists to fix.

### 3. `append_to_pattern` does not accept `language`

`append_to_pattern` is body-only by definition: it appends a heading and body to the existing file
without touching frontmatter. The right place to change frontmatter is `update_pattern`. Adding a
`language` argument to append would either:

- Be a no-op the schema lies about (the schema says "accepted", the handler ignores it), or
- Quietly mutate frontmatter on what an agent thinks is a body-only operation (worse — same class of
  footgun as the `tags`-on-update default-clear that already cost us once).

Schema honesty beats surface symmetry. Agents that want to add a language declaration to an existing
pattern call `update_pattern` (which now accepts `language`) or edit the markdown directly. The
append tool's description gains a single line pointing at this.

### 4. Unknown-token surfacing: stderr line + `language_warnings` on the metadata fence

When `parse_frontmatter_language_list` is invoked on the newly-written file during
`index_single_file`, it already emits `MalformedLanguageEntry` advisories for unknown tokens; the
entry-bearing path already logs each entry to stderr inside `index_single_file`. This plan does not
duplicate that stderr line — it just makes sure the new advisories from MCP-authored writes flow
through the same code path.

The new surface is on the tool response's `lore-metadata` fence. `WriteResult` gains a
`language_warnings: Vec<String>` field (collected from the `IndexedFile`'s `malformed_language`
advisories, mapped to the bare lowercased tokens). The handler renders it into the metadata JSON as:

```json
"language_warnings": ["objectiv-c"]
```

— an array of strings naming the offending tokens, lowercased, deduplicated, first-seen order. Empty
array (not `null`) when every token validated; field always present so agents can pattern-match on
the key. Field name matches the existing `warnings: []` convention used by the trace fence
(`src/server.rs:1227`).

Rejected:

- **stderr only**: invisible to agents using MCP, which is the surface this plan is fixing. The
  whole point is that the authoring path needs an agent-observable signal.
- **Metadata fence only**: breaks consistency with the ingest path's stderr log, which operators
  rely on when debugging language coverage from the CLI.
- **Richer objects (`{token, file_path}`)**: not actionable on the MCP side — the agent already
  knows which tokens it just passed. Strings are enough.

---

## High-Level Technical Design

```
Agent JSON-RPC call (language: "rust" or ["java","kotlin"])
        │
        ▼
src/server.rs::handle_add / handle_update
  • read args.language (Value)
  • coerce scalar → Vec<String> via parse_language_arg(arg)
  • check_limit on serialised length (reuse check_tags_limit pattern)
        │
        ▼
ingest::add_pattern(..., language: Option<&[&str]>)
ingest::update_pattern(..., language: Option<&[&str]>)  // three-way per Decision 2
  • build_file_content(title, body, tags, language) → "---\ntags: [...]\nlanguage: [rust]\n---\n..."
  • std::fs::write(file_path, content)
        │
        ▼
index_single_file (unchanged)
  • parse_frontmatter_language_list validates against is_known_token
  • returns ChunkingAdvisories { malformed_language, ... }
        │
        ▼
WriteResult { ..., language_warnings: Vec<String> }
        │
        ▼
Tool response prose + lore-metadata fence:
  { "language_warnings": ["objectiv-c"], ... }
```

_Directional guidance for review, not implementation specification._

---

## Implementation Units

### U1. Plumb `language` arg through `add_pattern` and `update_pattern`

**Goal:** Accept the `language` argument on the two MCP tools, coerce input shape, and pass it
through to `ingest::add_pattern` / `update_pattern`. No frontmatter rendering yet — that's U2.

**Dependencies:** None.

**Files:**

- `src/server.rs` — tool definitions (`add_pattern`, `update_pattern`) `inputSchema`s; `handle_add`,
  `handle_update` arg extraction
- `src/ingest.rs` — `add_pattern`, `update_pattern` signatures (add `language: Option<&[&str]>`
  parameter)

**Approach:**

- Add a `parse_language_arg(args: &Value) -> Result<Option<Vec<String>>, String>` helper in
  `src/server.rs` that handles the three cases: absent (`None`), string (coerce to one-element vec),
  array (collect strings). Reject non-string/non-array shapes with a structured error. For
  `update_pattern`, the absent vs `[]` vs `[...]` distinction has to be preserved end-to-end —
  return `Option<Vec<String>>` so absent maps to `None`, `[]` maps to `Some(vec![])`, `[x]` maps to
  `Some(vec!["x"])`. For `add_pattern`, an empty list is equivalent to absent (no semantic
  difference on create) — fold both to `None` for that call site.
- Add a `MAX_LANGUAGE_BYTES` limit (mirror `MAX_TAGS_BYTES` shape; serialised JSON size cap) and a
  `check_language_limit` helper that runs the same serialised-size check as `check_tags_limit`.
- Pass the coerced value into the ingest helpers as `Option<&[&str]>`. For now, the ingest helpers
  accept the parameter but ignore it (U2 wires it into `build_file_content`).

**Patterns to follow:**

- `tags` extraction in `handle_add` (`src/server.rs:808-816`) for the array-of-string shape.
- `tags` three-way handling in `handle_update` (`src/server.rs:875-889`) for the `Option<Vec<&str>>`
  lifetime dance.
- `check_tags_limit` (`src/server.rs:607-618`) for serialised-size validation.

**Test scenarios:**

- `add_pattern` with `"language": "rust"` → handler coerces to `vec!["rust"]` and passes through to
  `ingest::add_pattern`.
- `add_pattern` with `"language": ["java", "kotlin"]` → array preserved.
- `add_pattern` with `"language": []` → folded to `None` (or `Some(vec![])`; either is fine as long
  as the file ends up without a `language:` line in U2).
- `add_pattern` with no `language` key → `None`.
- `add_pattern` with `"language": 42` → handler returns the structured
  `"language must be a string or array of strings"` error.
- `update_pattern` with no `language` key → `Option<Vec<String>>` is `None` (preserves on update per
  Decision 2).
- `update_pattern` with `"language": []` → `Some(vec![])` (clears on update per Decision 2).
- `update_pattern` with `"language": ["go"]` → `Some(vec!["go"])` (replaces on update per Decision
  2).
- Serialised `language` larger than `MAX_LANGUAGE_BYTES` → returns the limit-exceeded error before
  reaching the ingest layer.
- The two existing snapshot/golden tests for `add_pattern` / `update_pattern` calls that omit
  `language` still pass unchanged (parameter is `None`-defaulted, no behaviour change).

Schema-shape assertions on `tool_definitions()` (the field is added here; U4 re-asserts these
alongside description-prose drift checks):

- `add_pattern.inputSchema.properties.language.oneOf` exists with the two-branch `string` /
  `array<string>` shape.
- `add_pattern.inputSchema.required` does not contain `language`.
- `update_pattern.inputSchema.properties.language` matches the same `oneOf` shape.
- `update_pattern.inputSchema.required` does not contain `language`.
- `append_to_pattern.inputSchema.properties` does not contain `language` (negative regression guard
  — Decision 3).

**Verification:** Unit tests for `parse_language_arg` cover the shape matrix; existing MCP tool
integration tests still pass.

---

### U2. Render `language:` frontmatter on write

**Goal:** Extend `build_file_content` so it emits a canonical `language:` line into the frontmatter.
Wire the parameter from U1 through `add_pattern` / `update_pattern` so the file on disk reflects
what the MCP call requested. Implement the three-way `update_pattern` semantics from Decision 2.

**Dependencies:** U1.

**Files:**

- `src/ingest.rs` — `build_file_content`; `update_pattern`'s preserve-on-`None` branch (mirror the
  `tags` logic for `language`); `add_pattern`'s frontmatter rendering

**Approach:**

- `build_file_content(title, body, tags, language)` builds the frontmatter block in canonical order:
  `tags:` first, then `language:`, both rendered as flow lists. A `language: [rust]` block emits
  even for a single-element list — flow list matches what the parser already accepts and avoids the
  scalar/list serialisation choice. Empty list does not render a line.
- `update_pattern`: when `language: None` is passed, read the existing file via
  `parse_frontmatter_language_list` and pass the preserved list to `build_file_content`. When
  `Some(vec![])`, render no `language:` line. When `Some([...])`, render the new list. Mirror the
  existing `tags` preserve-or-replace branch precisely.
- `add_pattern`: pass the (possibly empty) list straight to `build_file_content`; no preserve case.

**Patterns to follow:**

- `update_pattern`'s `tags` three-way handling (`src/ingest.rs:1282-1294`) — the `preserved` /
  `preserved_refs` lifetime dance is the precedent.
- `parse_frontmatter_tag_list` (`src/chunking.rs`) — the analogue parser for the language preserve
  path.
- `parse_frontmatter_language_list` for the read path on preserve.

**Test scenarios:**

- `add_pattern` with `language: ["rust"]` writes a file whose first non-empty content is
  `---\nlanguage: [rust]\n---\n\n# Title\n\nBody\n`.
- `add_pattern` with `language: ["java", "kotlin", "groovy"]` writes
  `language: [java, kotlin, groovy]`.
- `add_pattern` with `language: None` (or empty) writes no `language:` line — file matches today's
  output byte-for-byte (regression guard for the pre-`language` shape).
- `add_pattern` with both `tags: ["universal"]` and `language: ["rust"]` writes
  `tags: [universal]\nlanguage: [rust]` in that order.
- `update_pattern` with `language: None` on a file declaring `language: [swift, objectivec]` writes
  back `language: [swift, objectivec]` — the preserve case, the de-universalisation-class footgun
  analogue. **This is the critical test for Decision 2.**
- `update_pattern` with `language: Some(vec![])` on a file declaring `language: [rust]` writes back
  no `language:` line — the explicit-clear case.
- `update_pattern` with `language: Some(vec!["go"])` on a file declaring `language: [rust]` writes
  back `language: [go]` — the replace case.
- After every successful `add_pattern` / `update_pattern` with `language: Some(["rust"])`, the
  database row for that source has `language_json = '["rust"]'` (verified via direct DB read — this
  is the end-to-end assertion mandated by the slice-shape-vs-pipeline-tests learning).

**Verification:** File contents match the expected frontmatter shape; DB row's `language_json`
matches what the agent passed.

---

### U3. Surface unknown-token advisories on `WriteResult` and metadata fence

**Goal:** When any passed token fails `is_known_token`, surface the offending tokens on the tool
response's `lore-metadata` fence as `language_warnings: [...]`. Stderr advisories already flow
through `index_single_file` and need no new code.

**Dependencies:** U2.

**Files:**

- `src/ingest.rs` — add `language_warnings: Vec<String>` to `WriteResult`; populate from the
  `IndexedFile`'s `malformed_language` field
- `src/server.rs` — render the field into the `lore-metadata` fence JSON in `handle_add` and
  `handle_update`

**Approach:**

- After `index_single_file` returns, map `indexed.malformed_language` to a `Vec<String>` of the bare
  `token` field, dedup while preserving first-seen order, and assign to
  `WriteResult::language_warnings`. Empty vec (not `None`) when every token validated.
- In `handle_add` and `handle_update`, add `"language_warnings": result.language_warnings` to the
  metadata JSON. Always present, defaults to `[]`.
- **Inbox-branch path parity.** The inbox-branch short-circuit (`src/ingest.rs:1167-1184` for add,
  `src/ingest.rs:1298-1316` for update) writes the file to a new branch and pushes without invoking
  `index_single_file`, so it never sees the chunking parser's advisories. Closing this gap inside
  this plan: before the short-circuit returns, call
  `parse_frontmatter_language_list(&content, &filename)` directly on the about-to-be-written
  content, dedup the malformed entries' tokens, populate `WriteResult.language_warnings`, and emit
  the same `[lore]` stderr line per offending token that `index_single_file` does. The parser is
  pure (`&str` in, advisories out — no DB, no embedder, no I/O) so the cost is roughly ten lines and
  one new helper invocation per short-circuit site. Closing this here matters because inbox-branch
  is the agent-submission contract; leaving it advisory-blind would silently re-introduce the
  dropped-argument failure mode the plan exists to fix, just on a different code path.

**Patterns to follow:**

- `embedding_failures: Vec` flow on `IndexedFile` → `WriteResult` → metadata JSON (already wired
  end-to-end and is the closest precedent).
- `warnings: []` field on the trace metadata fence (`src/server.rs:1227`) for the field-name
  convention.

**Test scenarios:**

- `add_pattern` with `language: ["rust", "objectiv-c"]` → file gets `language: [rust, objectiv-c]`;
  metadata fence includes `"language_warnings": ["objectiv-c"]`; stderr carries the `[lore]`
  advisory line for `objectiv-c` (one line per offending token, already produced by
  `index_single_file`).
- `add_pattern` with all-valid tokens → metadata fence includes `"language_warnings": []` (present,
  empty).
- `add_pattern` with the same unknown token passed twice → `language_warnings` contains the token
  once (dedup).
- `update_pattern` with `language: ["objectiv-c"]` replacing `language: [rust]` → file's frontmatter
  updates; metadata fence includes the warning for `objectiv-c`.
- `update_pattern` with `language: None` on a file whose existing frontmatter contains an unknown
  token from an earlier write → `language_warnings` contains the preserved unknown token
  (re-validates on every write, matching ingest semantics).
- Inbox-branch path: `add_pattern` with `inbox_branch_prefix: Some(_)` and
  `language: ["objectiv-c"]` → `WriteResult.language_warnings` contains `["objectiv-c"]` (advisory
  fires via the direct `parse_frontmatter_language_list` call on the short-circuit path); stderr
  carries the `[lore]` advisory line.
- Inbox-branch path: `update_pattern` with `inbox_branch_prefix: Some(_)` and
  `language: ["rust", "objectiv-c"]` → `WriteResult.language_warnings` contains `["objectiv-c"]`;
  the valid `rust` token does not appear.
- Inbox-branch path with all-valid tokens (`language: ["rust"]`) → `WriteResult.language_warnings`
  is empty `[]` (the field is present, not omitted).

**Verification:** Metadata fence carries the expected `language_warnings` array; stderr line is
unchanged from today's ingest advisory shape.

---

### U4. UAT, tool descriptions, ROADMAP move, CHANGELOG

**Goal:** Documentation, real-binary UAT, and the housekeeping that closes the ROADMAP entry.

**Dependencies:** U1, U2, U3.

**Files:**

- `src/server.rs` — tool descriptions for `add_pattern`, `update_pattern` (document the new
  `language` field and its semantics); `append_to_pattern`'s description gains one sentence pointing
  at `update_pattern` for frontmatter changes
- `ROADMAP.md` — move the entry from `## Up Next` to `## Completed`
- `CHANGELOG.md` — single bullet per the two CHANGELOG conventions (user-facing only, one sentence
  ending in `(#N)`)
- `docs/pattern-authoring-guide.md` (if it has a section on `language:`) — note that the MCP
  authoring tools now accept the same field
- `tmp/mcp-language-arg-uat.md` — disposable per-PR UAT runbook (not committed; lives in `tmp/`)

**Approach:**

- Run the UAT through the real binary per the UAT-through-real-binary learning: build the release
  binary, start `lore serve` against an isolated XDG environment, drive a JSON-RPC `add_pattern`
  call with `language: "rust"`, confirm the on-disk file carries `language: [rust]`, run
  `lore ingest`, then `lore status` and assert the `Languages:` line includes `rust: 1`. Repeat with
  an unknown token to confirm both surfaces (stderr + metadata fence) fire.
- Cross-surface grep per multi-surface-consistency: `git grep -nF 'add_pattern'`,
  `git grep -nF 'language'` (filtered to the surfaces this PR touches),
  `git grep -nF 'update_pattern'` — confirm every doc reference quoting these tools matches the new
  signature.
- ROADMAP move and CHANGELOG bullet land in the same commit/PR (per the project's
  ROADMAP-update-in-feature-PR convention).

**Patterns to follow:**

- `docs/plans/2026-05-15-001-feat-language-in-status-plan.md` — the prior PR that extended an MCP
  tool's metadata fence. Its CHANGELOG entry shape is the template.
- `docs/solutions/best-practices/uat-through-real-binary-catches-inference-path-bugs-2026-05-19.md`
  — UAT discipline.
- `docs/solutions/best-practices/slice-shape-tests-are-not-pipeline-tests-2026-05-19.md` — why U2
  and U3 include end-to-end DB-state assertions, not just file-shape assertions.

**Test scenarios:**

Schema shape — `add_pattern`:

- `tool_definitions()["add_pattern"].inputSchema.properties.language` exists.
- `language.oneOf` is a two-element array; first branch is `{type: "string"}`, second is
  `{type: "array", items: {type: "string"}}`.
- `language` is NOT present in `inputSchema.required`.
- `inputSchema.required` still contains `title` and `body` (regression guard).
- The existing properties `title`, `body`, `tags`, and `include_metadata` are still present
  (regression guard against accidental dropping).
- `language.description` is non-empty.

Schema shape — `update_pattern`:

- Same six assertions as above, with `source_file` substituted for `title` in the required-fields
  regression guard.
- `language.description` substring-matches the three-way semantics vocabulary (e.g., `preserve`,
  `clear`, `replace`) so the contract documentation cannot silently drift back to the de-language
  footgun.

Schema shape — `append_to_pattern` (the load-bearing Decision 3 regression pin):

- `tool_definitions()["append_to_pattern"].inputSchema.properties` does NOT contain a `language`
  key.
- The existing properties `source_file`, `heading`, `body`, and `include_metadata` are still
  present.
- `tool_definitions()["append_to_pattern"].description` substring-matches `update_pattern` (the
  pointer that tells agents where frontmatter changes belong).

Tool description prose:

- `add_pattern.description` substring-matches `language` and mentions the unknown-token advisory
  behaviour (substring match on a stable noun such as `unknown` or `warn`).
- `update_pattern.description` substring-matches `language` and mentions the same advisory
  behaviour.

Top-level catalogue integrity:

- The `tools/list` JSON-RPC response still contains all three tool names (`add_pattern`,
  `update_pattern`, `append_to_pattern`) — regression guard against careless edits.

Out of automated scope (reviewer + UAT runbook):

- ROADMAP entry moved from `## Up Next` to `## Completed`, referencing this plan path.
- CHANGELOG bullet present and follows the project's two CHANGELOG rules (user-facing only, one
  assertive-voice sentence ending in `(#N)`).
- `docs/pattern-authoring-guide.md` updated if it has a section quoting the MCP tool surface.

**Verification:** all schema-shape and description-content assertions pass as unit tests against
`tool_definitions()`; UAT runbook passes end-to-end against the freshly-built binary; `cargo test`
green; `cargo clippy -- -D warnings` clean; ROADMAP entry sits under Completed referencing this plan
path.

---

## Test Strategy

Three layers, matching the slice-shape-vs-pipeline-tests learning:

1. **Unit (per implementation unit, in `#[cfg(test)] mod tests`).** Argument coercion (U1),
   frontmatter rendering (U2), advisory propagation (U3). Driven against
   `KnowledgeDB::open(":memory:")` and `FakeEmbedder`. These are the fast feedback loop; they don't
   prove the pipeline.

2. **End-to-end via the MCP server (U2 and U3).** Drive the server through its JSON-RPC interface,
   assert the file contents on disk **and** the resulting `language_json` value in the DB.
   Slice-shape tests pass even when ingest is broken; the pipeline test catches the regression.

3. **UAT through the real binary (U4).** Build, run, drive `lore serve` over the production stdio
   transport from an isolated XDG environment, exercise both the happy path and the unknown-token
   path. The PR is not ready for merge until this passes.

The four feature-bearing units above (U1, U2, U3) each have explicit DB-state assertions wherever
the unit produces a database-observable effect. The unit-level scenarios are the contract; the UAT
runbook is the smoke.

---

## Risk Analysis

### R1. Existing `language:` frontmatter is silently overwritten on update

If `update_pattern` is called without `language` against a file that declares
`language: [swift, objectivec]`, and the preserve path has a bug, the language line silently
disappears — same class of footgun as the `tags` de-universalisation incident that motivated
`tags`'s preserve-on-`None` behaviour. **Mitigation:** the critical preserve-case test in U2 pins
this exact scenario; the implementation mirrors `tags`'s precedent verbatim.

### R2. Coercion path admits malformed input that crashes the chunking parser

The frontmatter parser strips `,` and control characters as a defence; the MCP layer should not
duplicate that filter, but it should also not let in shapes the parser cannot handle.
**Mitigation:** the `parse_language_arg` helper accepts only `string` and `array<string>` and
rejects every other JSON shape; the unit test matrix in U1 covers the reject cases.

### R3. Existing snapshot tests for `add_pattern` file output break

`build_file_content` is touched. Any existing snapshot test of "an `add_pattern` call with no
`language` produces this exact bytes" would fire as a failure if the frontmatter rendering changes
shape even when `language` is absent. **Mitigation:** the absent-language case in U2 explicitly pins
byte-for-byte equality with today's output (no `language:` line, no extra blank lines). The change
is purely additive on the `language: Some(...)` path.

### R4. The inbox-branch path silently swallows the advisory

The inbox-branch short-circuit (`src/ingest.rs:1167-1184` and `1298-1316`) writes a file and pushes
to a remote without running `index_single_file` locally, so the chunking-driven advisory never fires
for those writes. **Mitigation:** closed inside U3 — both short-circuits invoke
`parse_frontmatter_language_list(&content, &filename)` directly on the about-to-be-written content
and populate `WriteResult.language_warnings` from its `malformed_language` output before returning.
The same stderr line that `index_single_file` emits for unknown tokens fires on the short-circuit
path too, so CLI and inbox-branch observability stay aligned. The dedicated inbox-branch test
scenarios in U3 pin this.

---

## Workflow Notes

- All work happens inside the sibling worktree at `/srv/misc/Projects/lore/lore-mcp-language-arg/`
  on branch `feat/mcp-language-arg`.
- The main checkout at `/srv/misc/Projects/lore/lore/` is not touched.
- Plan written to `docs/plans/2026-05-19-001-feat-mcp-language-arg-plan.md` inside the worktree.

## References

- ROADMAP entry under `## Up Next` (the `add_pattern`/`update_pattern`/`append_to_pattern` line)
- `docs/plans/2026-05-14-001-feat-language-detection-architecture-plan.md` — the architecture that
  introduced `language:` frontmatter and `language_json`
- `docs/plans/2026-05-15-001-feat-language-in-status-plan.md` — the precedent MCP metadata-fence
  extension this plan mirrors
- `docs/plans/2026-05-18-001-feat-language-table-expansion-plan.md` — the data-only PR that expanded
  the canonical token table to 27 entries
- `src/server.rs` — tool definitions, handlers, `WriteResult` metadata rendering
- `src/ingest.rs` — `add_pattern`, `update_pattern`, `append_to_pattern`, `build_file_content`,
  `WriteResult`, `IndexedFile`
- `src/chunking.rs` — `parse_frontmatter_language_list`, `MalformedLanguageEntry`
- `src/engine/languages.rs` — `is_known_token`, `LANGUAGES`
- `docs/solutions/best-practices/slice-shape-tests-are-not-pipeline-tests-2026-05-19.md`
- `docs/solutions/best-practices/uat-through-real-binary-catches-inference-path-bugs-2026-05-19.md`
