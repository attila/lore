# Search Mechanics Reference

This document describes the full search pipeline that determines which patterns surface during agent
sessions. It is a reference for power users who need to understand why a specific pattern does or
does not appear in search results, and how to diagnose discoverability issues.

For practical guidance on writing patterns that surface reliably, see the
[Pattern Authoring Guide](pattern-authoring-guide.md).

## Pipeline Overview

Every search follows this sequence:

```
Tool input → Query extraction → Term cleaning → FTS5 query assembly
    → FTS5 search (lexical) ──────────────────────────────┐
    → Vector search (semantic, if hybrid enabled) ────────┤
                                                          ▼
                                              RRF score merge
                                                          │
                                              min_relevance filter
                                                          │
                                              Sibling chunk expansion
                                                          │
                                              Session deduplication filter
                                                          │
                                              Formatted output → Agent
```

## Query Extraction

The hook reads signals from the agent's tool input to construct a search query. Three signal sources
are checked in order, and their contributions are merged.

### File Path Signals

When the tool provides a `file_path` field (Edit, Write, Read), the hook extracts two kinds of
information:

**Language anchor.** The file extension maps to a language keyword:

| Extension       | Language keyword |
| --------------- | ---------------- |
| `.rs`           | `rust`           |
| `.ts`, `.tsx`   | `typescript`     |
| `.js`, `.jsx`   | `javascript`     |
| `.yml`, `.yaml` | `yaml`           |
| `.py`           | `python`         |
| `.go`           | `golang`         |

**Filename terms.** The basename (without extension) is split on non-alphabetic boundaries and
camelCase transitions, then lowercased. `ValidateEmail.rs` produces "validate" and "email."

### Bash Signals

When the tool is Bash, the hook reads the `description` field (falling back to `command`). Two
extractions occur:

**Language inference.** The command text is scanned for tool names that imply a language:

| Command contains            | Inferred language |
| --------------------------- | ----------------- |
| `npm`, `npx`, `yarn`, `bun` | `typescript`      |
| `cargo`                     | `rust`            |
| `pip`, `python`             | `python`          |

**Term extraction.** The full text is split on whitespace and non-alphabetic boundaries, then
lowercased and added to the term pool.

### Transcript Tail

If the hook input includes a `transcript_path`, the hook reads the last 32 KB of the file, finds the
most recent user message in the JSONL stream, and extracts up to 200 bytes of content. These words
are added to the term pool as additional context.

The transcript path must resolve under `$HOME`; paths outside the home directory are silently
skipped.

## Term Cleaning

All extracted terms pass through a cleaning pipeline before query assembly:

1. **Strip non-alphabetic characters.** Digits, punctuation, and symbols are removed. `gh` stays;
   `v2.1` becomes `v`.
2. **Discard short terms.** Any term shorter than three characters is dropped. "ci" (two characters)
   and "pr" (two characters) are invisible to search.
3. **Filter hex-like strings.** Terms of six or more characters composed entirely of hexadecimal
   digits (`0-9`, `a-f`) are discarded. This prevents commit SHAs and UUIDs from polluting queries.
4. **Remove stop words.** Sixty common English words are removed:

   > the, and, for, with, from, into, that, this, then, when, will, has, have, was, are, not, but,
   > can, all, its, our, use, new, let, set, get, add, run, see, how, may, per, via, yet, also,
   > just, some, been, were, what, they, each, which, their, there, about, would, could, should,
   > these, those, other, than, them, your, does, here

5. **Deduplicate.** Duplicate terms are removed, preserving the order of first appearance.

## FTS5 Query Assembly

The cleaned terms and the inferred-language set (which may be empty, singular, or multi-valued) are
assembled into one of these query shapes:

| Language set    | Terms                | Query shape                                            | Example                                 |
| --------------- | -------------------- | ------------------------------------------------------ | --------------------------------------- |
| Single inferred | Present              | `{lang} AND ({term1} OR {term2} OR ...)`               | `rust AND (validate OR email)`          |
| Multi inferred  | Present              | `({lang1} OR {lang2}) AND ({term1} OR {term2} OR ...)` | `(javascript OR typescript) AND (test)` |
| Inferred        | Empty (all filtered) | `{lang}` or `({lang1} OR {lang2})`                     | `rust`                                  |
| Not inferred    | Present              | `{term1} OR {term2} OR ...`                            | `create OR body OR file`                |
| Not inferred    | Empty                | No query (search skipped)                              | —                                       |

The single-inferred case is the most common during normal agent sessions. The multi-inferred case
arises when a signal legitimately fires for several languages — `npm test` accumulates
`{javascript,
typescript}` because both entries register `npm` as a command keyword. The OR-grouped
language anchor preserves the AND-with-terms structure so retrieval ranking remains predictable.

## Structural Language Gate

In addition to the FTS string assembly above, retrieval composes as three independently-ranked
candidate lists fed to RRF. Patterns may declare their applicable languages via the optional
`language:` frontmatter field (see the
[pattern authoring guide](pattern-authoring-guide.md#pattern-language-declaration)). The retrieval
pipeline applies a structural gate using SQLite's `json_each()` over the persisted `language_json`
column:

1. **FTS-fallback** — `MATCH "{lang} AND ({terms})"` with `WHERE c.language_json IS NULL`. Patterns
   without a `language:` declaration reach retrieval through this branch; the language anchor
   appears in the FTS predicate so the body must contain the canonical token to match. When the
   inferred-language set is empty, the MATCH collapses to terms-only and the `IS NULL` filter still
   applies.
2. **FTS-structural** — `MATCH "({terms})"` with
   `WHERE EXISTS (SELECT 1 FROM json_each(c.language_json) WHERE value IN (?inferred_langs...))`.
   Patterns with a declared `language:` that intersects the inferred set reach retrieval through
   this branch; the body anchor is waived because the declaration itself is the structural
   eligibility signal. Skipped entirely when the inferred set is empty (nothing to gate against).
3. **Vector (oversample-and-filter)** — `vec0 MATCH ?embedding AND k = ?(top_k * N)`
   `ORDER BY v.distance` fetches `N`-times-`top_k` nearest neighbours (initial multiplier `N = 3`),
   then filters in code by the same `language_json IS NULL OR EXISTS json_each ...` predicate the
   FTS branches use. Take the top `top_k` after filtering. This preserves the structural gate's
   guarantee that wrong-language-labelled patterns cannot sneak in via semantic similarity. When the
   inferred set is empty the filter degenerates to no filter — terms-only retrieval per the "no
   declaration, match on body keywords" fallback rule.

The two FTS branches are **disjoint by predicate**: a chunk has `language_json IS NULL` xor
`language_json` containing the inferred lang. No pattern double-counts across the two FTS branches.
RRF sees at most one FTS rank and one vector rank per pattern; the three-list count is a
code-organisation choice, not an arithmetic inflation. Each FTS branch carries its own internally
consistent BM25 weighting against its own MATCH terms; RRF uses positional rank from `enumerate`,
not raw BM25 score, so cross-branch score commensurability is not a concern.

**Declaration is a gate, not a ranking signal.** The structural branch's MATCH expression contains
only the enrichment terms — the inferred language tokens never enter the FTS predicate. A declared
pattern is admitted regardless of body vocabulary, but its rank inside the structural branch is
decided by how strongly the enrichment terms match the chunk's `title` (BM25 weight 10), `tags` (5),
and `body` (1). A pattern declaring `language: rust` with prose like "Use anyhow for errors" can
rank below an undeclared pattern whose heading reads `## Rust error handling` for the same query —
the declaration ensures eligibility, not dominance. This is intentional: the gate's job is to
prevent declared patterns from being filtered out by absence of a body keyword, not to override BM25
ranking inside their branch.

> **Why declare `language:`?** When a pattern's body uses prose that does not happen to repeat the
> canonical language token, the FTS-fallback branch will miss it. Declaring `language: rust` lets
> the pattern surface on every Rust tool call regardless of body vocabulary; without the declaration
> the pattern relies on its body containing the canonical keyword — which works for patterns that
> already mention the language in prose, and fails silently for the rest.

## FTS5 Search

Lore stores indexed content in an FTS5 virtual table with four searchable columns:

| Column        | BM25 weight | Purpose                                         |
| ------------- | ----------- | ----------------------------------------------- |
| `title`       | 10.0        | Pattern title (from the first markdown heading) |
| `tags`        | 5.0         | YAML frontmatter tags                           |
| `body`        | 1.0         | Section body text                               |
| `source_file` | 0.0         | File path (not weighted, used for grouping)     |

Title matches rank ten times higher than body matches. Tag matches rank five times higher. This
means a query for "error handling" strongly prefers a pattern titled "Error Handling" over one that
merely mentions the phrase in passing.

### Porter Stemming

The FTS5 table uses the tokeniser `porter unicode61`, which applies porter stemming to both indexed
content and search queries. Stemming reduces words to their root form:

- "testing" and "test" both stem to "test" — they match each other
- "fakes" and "fake" both stem to "fake" — they match each other
- "creating" and "create" both stem to "creat" — they match each other

Stemming handles morphological variants but not synonyms. "Edit" and "create" have different stems
and do not match.

### Query Sanitisation

Before the query reaches FTS5, special characters are replaced with spaces to prevent syntax errors.
The following characters are stripped:

> `. / \ : { } [ ] " ' * ^ -`

Leading minus signs on terms are also stripped (FTS5 interprets them as the NOT operator).

Parentheses and the keywords `AND`, `OR`, and `NOT` are preserved, allowing the hook to construct
structured queries such as `rust AND (validate OR email)`.

## Vector Search

When hybrid mode is enabled and Ollama is reachable, the query is also embedded as a vector and
compared against stored pattern embeddings using cosine distance via sqlite-vec.

The embedding text for each chunk is constructed as: `{title}\n{tags}\n{body}`. This means tags and
titles contribute to semantic similarity, not just lexical matching.

The default embedding model is `nomic-embed-text` (768 dimensions). Other models are supported by
changing `ollama.model` in `lore.toml`.

If Ollama is unreachable or embedding fails, the search falls back to FTS5 only.

## Hybrid Merge: Reciprocal Rank Fusion

Retrieval composes as up to three independently-ranked candidate lists (FTS-fallback,
FTS-structural, vector) merged using Reciprocal Rank Fusion (RRF) with a constant `k = 60`:

```
score(item) = Σ 1 / (k + rank_in_list_i)
```

Each candidate list retrieves `top_k * 2` results independently, and the merged scores are
normalised to a 0–1 range by dividing by the theoretical maximum across `N` input lists:

```
max_rrf = N / (k + 1.0)
normalised_score = score / max_rrf
```

A normalised score of 1.0 means the item ranked first in every list it appeared in. The three-list
count is the typical hybrid path (FTS-fallback + FTS-structural + vector); when the embedding step
is skipped or fails, RRF reduces to two lists; when the inferred-language set is empty, the
structural branch is skipped and RRF reduces accordingly.

## Relevance Threshold

The `min_relevance` setting (default: 0.6) filters results below a minimum normalised score. This
threshold is applied only when:

- Hybrid search is enabled (`search.hybrid = true`)
- Embedding succeeded (Ollama returned a valid vector)
- `min_relevance` is greater than zero

When FTS5 is the sole search method (hybrid disabled or Ollama unreachable), no threshold is applied
because FTS5 BM25 scores use a different scale and are not directly comparable.

## Ingest-Time Filtering with `.loreignore`

Search only sees what ingest indexed. A `.loreignore` file at the repository root excludes matching
markdown files from the index entirely — they never reach the FTS5 or vector tables, so they cannot
appear in results regardless of how the query is constructed.

Filtering applies during both full ingest and delta ingest. When `.loreignore` changes, delta ingest
detects the change via a content hash stored in `ingest_metadata` and runs a cumulative
reconciliation pass: previously indexed files that now match an exclusion are removed, and files
that are no longer excluded are re-indexed from disk automatically.

For the full syntax and behaviour, see the [Configuration Reference](configuration.md#loreignore).

## Sibling Chunk Expansion

After the top results are selected, lore fetches all chunks from each matched source file. If a
query matches the "Error Handling" section of a pattern file, every other section in that file is
also included in the injection.

This ensures agents receive the complete context of a pattern, not just the section that matched the
query. A pattern about error handling may have related sections on testing strategy and library
choice that are valuable in the same context.

## Session Deduplication

To prevent the same pattern from being injected repeatedly within a session, lore maintains a
per-session deduplication file at `/tmp/lore-session-{hash}`, where `{hash}` is a 16-character
FNV-1a hash of the session ID.

The deduplication lifecycle:

| Event        | Action                                                                 |
| ------------ | ---------------------------------------------------------------------- |
| SessionStart | Create or truncate the deduplication file                              |
| PreToolUse   | Read existing IDs, filter results, append new IDs                      |
| PostCompact  | Truncate the deduplication file (context was compressed, so re-inject) |

The read-filter-write sequence is protected by an exclusive advisory file lock (`fd-lock`) to
prevent concurrent hook invocations from losing writes.

Deduplication is gated on file existence: if the deduplication file does not exist (no SessionStart
has run), deduplication is skipped entirely. This prevents stale files from interfering with manual
CLI invocations of `lore hook`.

## Worked Examples

### Example 1: Editing a Rust File

**Tool input:**

```json
{
  "tool_name": "Edit",
  "tool_input": { "file_path": "src/validate_email.rs" }
}
```

**Query extraction:**

1. Extension `.rs` → language anchor `rust`
2. Basename `validate_email` → split → "validate", "email"
3. No Bash signals, no transcript tail in this example

**Term cleaning:** Both "validate" and "email" are longer than two characters, not stop words, and
not hex-like. They survive cleaning.

**Query assembly:** `rust AND (validate OR email)`

**FTS5 search:** Matches patterns with "rust" in the title or tags AND either "validate" or "email"
in the body. A pattern titled "Error Handling" tagged with `[rust, error-handling, anyhow]`
containing "validate" in its body would match.

### Example 2: Running a GitHub CLI Command

**Tool input:**

```json
{
  "tool_name": "Bash",
  "tool_input": {
    "description": "Create PR with body file for the feature branch",
    "command": "gh pr create --title 'Add validation' --body-file /tmp/pr-body.md"
  }
}
```

**Query extraction:**

1. No `file_path` → no language anchor from extension
2. Bash tool → reads `description` field: "Create PR with body file for the feature branch"
3. No language inferred (no `cargo`, `npm`, `pip` in the text)
4. Terms from description: "create", "pr", "with", "body", "file", "for", "the", "feature", "branch"

**Term cleaning:**

- "pr" → discarded (two characters)
- "with", "for", "the" → discarded (stop words)
- Surviving terms: "create", "body", "file", "feature", "branch"

**Query assembly:** `create OR body OR file OR feature OR branch`

**FTS5 search:** Matches the "GitHub Pull Requests" section of `agents/unattended-work.md`, which
contains "creating PRs," "body," and "file" in its body text. Porter stemming maps "create" to the
same root as "creating."

### Example 3: Bash Error Triggers PostToolUse Search

**Tool response (non-zero exit code):**

```json
{
  "tool_name": "Bash",
  "tool_response": {
    "exitCode": 1,
    "stderr": "error: permission denied for --body with heredoc in don't-ask mode"
  }
}
```

**Query extraction (PostToolUse path):**

1. Non-zero exit code triggers error-driven search
2. Stderr text is split into words: "error", "permission", "denied", "for", "body", "with",
   "heredoc", "in", "don't", "ask", "mode"

**Term cleaning:**

- "for", "with" → discarded (stop words)
- "in" → discarded (two characters)
- "don't-ask" splits on non-alphabetic boundaries into "don", "t", and "ask" — "t" is discarded (one
  character), "don" survives (three characters), "ask" survives
- Surviving terms: "error", "permission", "denied", "body", "heredoc", "don", "ask", "mode"

**Query assembly:** `error OR permission OR denied OR body OR heredoc OR don OR ask OR mode`

**FTS5 search:** "permission" and "heredoc" match the `agents/unattended-work.md` pattern, which
states "Inline and heredoc approaches get blocked by permission settings."
