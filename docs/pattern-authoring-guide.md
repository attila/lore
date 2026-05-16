# Writing Effective Patterns

Lore indexes your patterns and injects them into agent sessions at precisely the moments they are
needed. But indexing alone does not guarantee effectiveness. A pattern must be both **discoverable**
(the search engine finds it) and **actionable** (the agent follows it). This guide explains how to
write patterns that achieve both, based on dogfooding evidence.

## Why Patterns Fail

Dogfooding revealed a consistent failure mode: patterns that describe what exists do not change
agent behaviour. Patterns that state what to do — and why — succeed.

Consider two real patterns from a production knowledge base:

**This pattern failed to prevent a known mistake:**

```markdown
## Task runner: just

`just` is the task runner for all local and CI operations. Every quality gate has a recipe.
`just ci` runs them all in sequence — if local CI passes, remote CI passes.

Key recipes: `fmt`, `clippy`, `test`, `deny`, `doc`, `ci`, `setup` (git hooks).
```

The pattern accurately describes the tooling, but it never says _always run `just ci` before
committing_. An agent read this, understood the tool existed, and then ran individual `cargo clippy`
and `cargo test` commands instead — missing the formatting check and breaking CI.

**This pattern consistently prevents its target mistake:**

```markdown
# Atlassian MCP createIssueLink has swapped inward/outward semantics

When creating "Blocks" links via MCP, inwardIssue is the BLOCKER and outwardIssue is the BLOCKED
issue — opposite of intuition.

**Why:** Discovered on 2026-03-12 when all dependency links from a plan were created backwards.

**How to apply:** Every time you use `createIssueLink` with type "Blocks", remember to swap: the
blocker goes in `inwardIssue`, the blocked goes in `outwardIssue`. Verify by reading one ticket
after creating links.
```

The difference is not length or formatting. The effective pattern states what to do, explains why
the rule exists, and uses the exact terms an agent would encounter when performing the task.

## Three Traits of Effective Patterns

### Imperative Voice

Tell the agent what to do and what never to do. Passive descriptions ("X is the formatter") leave
room for interpretation. Direct imperatives ("Always run X before committing") do not.

**Weak (descriptive):**

> `just` is the task runner for all local and CI operations.

**Strong (imperative):**

> Always run `just ci` before committing. Never substitute individual commands such as `cargo test`
> or `cargo clippy` — they skip formatting checks and other quality gates that `just ci` runs in
> sequence.

Imperative voice serves a dual purpose: it is both more actionable for agents and more searchable by
the engine. "Always run `just ci` before committing" contains the verb "run," the tool name, and the
action context "committing" — all terms an agent might use when the pattern should surface.

### Incident Grounding

When a rule exists because something went wrong, say so. A prohibition grounded in a real failure
carries more weight than an abstract best practice.

**Without incident context:**

> Use `--body-file` instead of `--body` with heredocs for `gh pr create`.

**With incident context:**

> Use `--body-file /tmp/pr-body.md` instead of `--body` with inline strings or heredocs. Inline and
> heredoc approaches get blocked by don't-ask mode permission settings.

The second version explains _why_ — the agent now understands the consequence of violating the rule,
not just the rule itself.

Not every pattern originates from a failure, and you should not fabricate incident context where
none exists. A configuration reference such as "enable clippy pedantic at warn level" does not need
a failure story. Use incident grounding when you have it; omit it when you do not.

### Discoverable Vocabulary

A pattern that the search engine cannot find is a pattern that does not exist. Lore uses FTS5
full-text search with porter stemming, which means the words in your pattern's title, tags, and body
determine whether it surfaces for a given query.

**The vocabulary gap in practice:**

A pattern about GitHub pull requests contained the phrase "when creating PRs with `gh pr create`."
When an agent ran `gh pr edit --body`, the hook extracted terms like "edit" and "body" — neither of
which appeared in the pattern. The pattern scored zero despite being directly relevant.

The fix is straightforward: mention the verbs and nouns an agent might use. If a pattern applies to
creating, editing, and updating pull requests, use all three verbs in the body.

> **Why not semantic search?** You might expect semantic search to bridge this gap — embeddings
> should understand that "edit" and "create" are related concepts. In practice, semantic search
> helps with broader conceptual matching, but hook-injected queries are typically short (three to
> five terms extracted from a tool call), and short queries produce weak embedding signals. FTS5
> keyword matching carries most of the weight for these precise, tool-driven lookups. Relying on
> semantic search alone to compensate for missing vocabulary is a gamble; including the terms
> directly is a certainty.

## How Lore Finds Your Pattern

Understanding the search pipeline helps you write patterns that surface reliably. This section
covers the essentials; see [Search Mechanics Reference](search-mechanics.md) for the full pipeline.

### Query Construction

When an agent uses a tool, the hook extracts search terms from its input:

| Signal          | Source                              | Example                                                  |
| --------------- | ----------------------------------- | -------------------------------------------------------- |
| File extension  | `file_path` in Edit/Write           | `.rs` → language anchor "rust"                           |
| Filename terms  | `file_path` basename                | `validate_email.rs` → "validate", "email"                |
| Bash command    | `description` or `command` field    | `cargo clippy` → language "rust", terms from description |
| Transcript tail | Last user message (up to 200 bytes) | Additional context terms                                 |

These terms are assembled into an FTS5 query. When a language is detected, the query takes the form
`rust AND (validate OR email)`. When multiple languages are inferred from shared signals (e.g.
`npm
test` infers both JavaScript and TypeScript), they OR-join inside parentheses:
`(javascript OR typescript) AND (terms)`. When no language is detected, terms are joined with `OR`.

Patterns can also declare their applicable languages explicitly via the `language:` frontmatter
field — see [Pattern language declaration](#pattern-language-declaration). Declared patterns reach
retrieval through a structural gate that matches against the inferred-language set without requiring
the canonical token to appear in the pattern body; undeclared patterns continue to rely on the
keyword-matching path described here.

### What the Engine Cannot See

Two filtering rules remove terms before they reach the search engine:

**Short terms are discarded.** Any term shorter than three characters is dropped. This means "ci"
(two characters) never reaches the search engine, and a pattern about `just ci` must contain longer
synonyms such as "continuous integration" or "quality gate."

**Stop words are filtered.** Sixty common English words are removed from queries, including `just`,
`use`, `new`, `run`, `get`, `set`, `add`, and `all`. If your pattern's key concept is also a stop
word, ensure the body contains alternative vocabulary.

The full stop word list:

> the, and, for, with, from, into, that, this, then, when, will, has, have, was, are, not, but, can,
> all, its, our, use, new, let, set, get, add, run, see, how, may, per, via, yet, also, just, some,
> been, were, what, they, each, which, their, there, about, would, could, should, these, those,
> other, than, them, your, does, here

### Porter Stemming

Lore uses porter stemming, which reduces words to their root form during both indexing and search.
"Testing" and "test" share the same stem, so a query for "test" matches a pattern containing
"testing." Similarly, "fakes" matches "fake," and "creating" matches "create."

Stemming helps with morphological variants, but it does not help with synonyms. "Edit" does not stem
to "create," so a pattern that says "creating" will not match a query containing "edit."

## Vocabulary Coverage Technique

Before finalising a pattern, audit its term coverage. Use `lore ingest --file` to index the draft
without committing it — the full edit → ingest → search loop runs against your working tree, no WIP
commit required:

1. List the verbs and nouns an agent would use when the pattern should surface. For a pattern about
   pull request workflows, this might include: create, edit, update, merge, push, branch, PR, pull
   request, draft, review.

2. Index the draft file directly:

   ```sh
   lore ingest --file patterns/my-new-pattern.md
   ```

3. Search for your pattern using the terms from step 1:

   ```sh
   lore search "edit pull request" --top-k 3
   lore search "merge branch workflow" --top-k 3
   ```

4. If your pattern does not appear in the results, add the missing terms to the body or tags and
   re-run `lore ingest --file patterns/my-new-pattern.md`. Repeat until every query from step 1
   surfaces the pattern.

5. Remember that stop words and short terms are invisible. If a key concept such as "CI" is too
   short, include a longer form such as "continuous integration" or "quality gate pipeline."

Single-file ingest does not touch delta-ingest state, so the next `lore ingest` (against the whole
repository) still sees real git changes. It also respects `.loreignore` by default; pass `--force`
alongside `--file` to index a file that is otherwise excluded.

**Interaction hazard — `lore ingest` can wipe chunks you just upserted.** Single-file ingest is
orthogonal to git state, but walk-based delta ingest is not. If you single-file-ingest a file that
was deleted in git history between the last walk-based ingest and `HEAD`, the next `lore ingest`
will observe the deletion in `git diff` and remove the chunks you just added. Concrete example: you
`git rm draft.md` and commit, then recreate `draft.md` in the working tree and run
`lore ingest --file draft.md`. The single-file ingest lands. Running `lore ingest` afterwards will
silently undo it. The safe workflow is to finish iterating with `lore ingest --file`, commit the
file to git, and only then run `lore ingest`.

**Automating this loop with the `/coverage-check` skill.** If you are authoring patterns inside a
Claude Code session with the lore plugin installed, the `/coverage-check <pattern-file-path>` skill
automates the entire loop above: it derives the candidate query set by simulating the PreToolUse
hook's own query extraction on synthetic tool calls (via the `lore extract-queries` subcommand),
ingests the draft via `lore ingest --file`, searches in parallel, scores per-query coverage,
suggests concrete edits to close gaps, and iterates until the surfaced-query set stabilises. Because
the queries are hook output rather than author paraphrase, the report approximates production
discoverability more closely than the manual loop. See
`integrations/claude-code/skills/coverage-check/SKILL.md` for the full contract.

## Tag Strategy

Tags appear in YAML frontmatter and are indexed as a separate FTS5 column with five times the weight
of body text. Use them to boost discoverability for terms that do not appear naturally in the body.

**Tags are useful when** they add vocabulary the body lacks. If your pattern about error handling
also applies to "anyhow" and "result types," adding those as tags ensures the pattern surfaces for
queries containing those terms — even if the body text uses different phrasing.

**Tags are redundant when** they duplicate terms already prominent in the title or body. Adding
`tags: [error, handling]` to a pattern titled "Error Handling" and containing the phrase "error
handling" in every paragraph adds no search value.

**Practical guidance:**

- Use three to seven tags per pattern
- Include abbreviations, tool names, and domain terms that agents might search for
- Avoid generic tags such as "best-practice" or "convention" — they match too broadly
- Tags are comma-separated in YAML: `tags: [rust, clippy, linting, code-quality]`

## When to use the `universal` tag

A pattern tagged `universal` opts into lore's always-on injection tier. Its full body is emitted at
every `SessionStart` (and re-emitted at every `PostCompact`) under a dedicated
`## Pinned
conventions` section, AND it bypasses the `PreToolUse` dedup filter so it re-injects on
every relevant tool call instead of being suppressed after the first appearance. The relevance gate
stays intact — the pattern still has to match the agent's tool call to inject.

When a universal pattern carries an `applies_when` predicate (see the next section), the
SessionStart and PostCompact pinning step is skipped — a predicate-bearing pattern has implicitly
declared itself conditionally relevant, and pinning it at session start would contradict its own
scope. Such patterns still re-inject on every matching `PreToolUse` call via the predicate path;
they are deferred until needed rather than pinned upfront.

The header in the SessionStart payload reads `## Pinned conventions` rather than
`## Universal
patterns` because that phrasing reads better as agent-facing copy. Both names refer to
the same mechanism: the `tags:` value is `universal`; the section header is `## Pinned conventions`.

**Use `universal` for patterns where:**

- The pattern is a process-level convention (commit messages, push discipline, branch naming, PR
  etiquette, code review process). These rules need continuous reinforcement throughout a session,
  not one-shot relevance.
- One reminder is not enough. The motivating example was a `git push` failure mid-session because
  dedup had correctly suppressed the workflow pattern after its first appearance hours earlier.
- The body is small enough to justify per-call re-injection. A 2 KB universal pattern matched 50
  times in a session adds 100 KB of repeated context. Lore emits a per-pattern advisory at ingest
  time when any single universal body exceeds 1 KB.

**Do not use `universal` for:**

- Code-style conventions whose authority is naturally scoped to specific file types or tool calls
  (Rust naming, TypeScript imports). The PreToolUse hook surfaces these correctly without the
  always-on cost.
- Reference material the agent reads once at the top of a session and remembers (terminology,
  architecture overviews). The standard pattern index handles these.
- Long-form documentation. Universal bodies should be short and directive. If your pattern needs
  more than ~1 KB to express, split it.

**Operational notes:**

- Keep the count low. Lore emits a stderr advisory when more than three patterns are tagged
  `universal` so authors notice the count growing past intent.
- Tag changes take effect at the next `lore ingest`. Both adding and removing the tag work the same
  way — re-ingest the file (delta or single-file) and the next session reflects the new state.
- The tag is case-sensitive: `universal` is the only form that opts in. Lore emits a near-miss
  advisory at ingest if it sees `Universal`, `UNIVERSAL`, or any other case-shifted variant in a
  pattern's tag list.
- Universal patterns are exempt from the coverage-check skill's discoverability scoring (they bypass
  the channel coverage-check measures), but the report marks them so authors do not pointlessly
  chase low coverage scores on a pattern that always re-injects.

## Tool/command predicate (`applies_when`)

A `universal` tag opts a pattern into always-on injection, but always-on can mean too-on. A workflow
pattern about `git push` re-injects on every `Bash` call — including `ls`, `wc -l`, and `grep` where
its content has no relevance. The `applies_when` predicate gates re-injection on tool class and Bash
command prefix so a universal pattern fires only when the call is actually relevant to it.

The predicate is whole-file: it lives in the pattern's frontmatter, and every chunk of the pattern
shares it. There are no per-section predicates.

### Authoring shape

```yaml
---
title: Git Branch and PR Workflow
tags:
  - workflow
  - universal
applies_when:
  tools: [Bash]
  bash_command_starts_with: [git, gh]
---
```

Both keys are optional, and `applies_when` itself is optional. A universal pattern without
`applies_when` continues to fire on every relevant call, exactly as before — the predicate is
opt-in.

### Available keys

- `applies_when.tools` — list of tool-class names (e.g. `Bash`, `Edit`, `Write`). Matches when the
  current tool name is in the list. Case-sensitive.
- `applies_when.bash_command_starts_with` — list of Bash command tokens (e.g. `git`, `gh`, `cargo`).
  Matches when the call is `Bash` AND the command starts with one of the listed tokens (after the
  smart-prefix matcher walks past common wrappers — see below).

### Semantics

- **OR within each list.** Any one entry matching is enough.
- **AND across keys.** If both `tools` and `bash_command_starts_with` are set, both must match.
- A missing key is unconstrained (does not narrow the match). A pattern with only
  `bash_command_starts_with: [git]` and no `tools` key implicitly requires `Bash` because the
  command-prefix check only meaningfully runs on `Bash` calls.

### SessionStart pinning is deferred when `applies_when` is set

The mere presence of an `applies_when` block — regardless of which keys it carries — opts a
universal pattern out of the SessionStart pinned-conventions block (and out of the PostCompact
re-emit, which shares the same code path). The reasoning is symmetrical to the predicate's own
declaration: a pattern with a predicate is conditionally relevant, and pinning it unconditionally at
session start would contradict that scope. Predicated universals are therefore deferred to their
PreToolUse path and re-inject on every matching tool call, just as the predicate specifies — they do
not also pin at session start.

This carries a small first-tool-call delay: the predicated pattern is visible to the agent on the
first tool call where the predicate matches AND the search pipeline returns at least one result, not
earlier. With the default `min_relevance_universal` (which inherits from `min_relevance` and is
`0.0` out of the box) and the search-overfetch / universal-no-truncate behaviour in
`search_with_threshold`, this is the realistic case. If you raise `min_relevance_universal` high
enough to filter weak-keyword universals out, predicated patterns can be deferred across multiple
tool calls — that is the cost of a strict universal floor and applies to every universal, not just
predicated ones.

### Smart-prefix matcher behaviour

`bash_command_starts_with` does not require a literal first-token match. The matcher walks past
common wrappers before checking the prefix, so a pattern declaring `bash_command_starts_with: [git]`
fires on all of these:

- `git status`
- `git status` — leading whitespace is trimmed
- `sudo git status` — sudo with no flags
- `sudo -E git push` — sudo with short flags (`-E`, `-H`)
- `sudo -u user git push` — sudo with `-u USER` (two-token form)
- `env GIT_PAGER=cat git log` — single `KEY=VAL` assignment
- `env A=1 B=2 git status` — multiple `KEY=VAL` assignments
- `env -u VAR git status` — env with `-u VAR` (two-token unset-var form)
- `env -i git status` — env with `-i` (hermetic-environment flag)
- `env A=1 env B=2 git status` — nested env wrappers; each `env` scope is unwrapped in turn
- `sudo env A=1 git pull` — one `sudo` wrapper followed by one `env` wrapper
- `bash -c "git status"` — `bash -c` quoted-command extraction; the first token inside the quoted
  body is the effective command head
- `sh -c 'gh pr create'` — same as above with single quotes; `sh` is treated identically to `bash`

The matcher operates on the raw command string — it never passes through the FTS-cleaning that
strips short tokens, so two-character commands like `gh` survive intact.

**Documented limitations.** The matcher unwraps at most one outer `sudo` scope, any number of nested
`env` scopes, and one outer `bash -c` / `sh -c` scope. The following do NOT fire on
`bash_command_starts_with: [git]`:

- `bash -c "echo \"git status\""` — nested-quote / escaped-quote handling inside `bash -c` is not
  implemented; the matcher splits at the first matching outer quote and does not unescape inner
  ones.
- `bash -c "sudo git status"` — wrapper-stripping inside the quoted `bash -c` body is not recursive;
  the matcher takes the body's first whitespace-delimited token verbatim (`sudo`), so an inner
  wrapper is not unwrapped.
- `env "A=value with spaces" git status` — quoted `KEY=VAL` with internal spaces is not recognised;
  the tokeniser splits the value at the first space.

In practice these forms are unusual; a single sudo, nested env wrappers, and a single `bash -c`
covers nearly all realistic invocations.

### Empty-list semantics

`tools: []` and `bash_command_starts_with: []` are valid syntax but match nothing — they are
zero-element allowlists. If you want a pattern to fire on every Bash call (and no other tool), set
`tools: [Bash]`, not `tools: []`.

### Indentation contract

The frontmatter parser is hand-rolled and enforces a strict indentation contract:

- Top-level keys (`tags:`, `applies_when:`) at column 0.
- Nested keys under `applies_when:` (`tools:`, `bash_command_starts_with:`) at 2-space indent.
- Block-list items under nested keys at 4-space indent (`- git`).
- **Tabs are not accepted.** A tab-indented child triggers an ingest-time advisory and the predicate
  is treated as absent.

Inline-list values (`tools: [Bash]`) and block-list values

```yaml
applies_when:
  tools:
    - Bash
  bash_command_starts_with:
    - git
    - gh
```

are both supported and parse identically.

### Behaviour on a non-universal pattern

You can attach `applies_when` to a non-universal pattern, but it is dormant in this release: the
ingest layer parses and persists it, but the PreToolUse evaluator only consults it for chunks that
also carry the `universal` tag. Ingest emits an info-level advisory naming the file so the
discrepancy is visible. A future track will extend evaluation to non-universal patterns and
introduce additional keys (`environments`). The `language:` key documented in
[Pattern language declaration](#pattern-language-declaration) below is the first such addition; it
is orthogonal to `applies_when` and gates retrieval rather than evaluation.

### Worked examples

**A workflow pattern restricted to `git` and `gh`** — fires on every Bash call that uses git or gh,
suppressed on `Bash ls`, `Bash wc -l`, `Bash grep`, and on any non-Bash tool:

```yaml
---
title: Git Branch and PR Workflow
tags: [workflow, universal]
applies_when:
  bash_command_starts_with: [git, gh]
---
```

**A shell-safety pattern that should fire on every Bash call** — and only Bash:

```yaml
---
title: Always quote variable expansions in shell scripts
tags: [shell, safety, universal]
applies_when:
  tools: [Bash]
---
```

**A multi-tool universal** — fires on Edit, Write, and Bash but not Read:

```yaml
---
title: Never write secrets to source files
tags: [security, universal]
applies_when:
  tools: [Edit, Write, Bash]
---
```

For tuning the relevance floor on universal patterns independently of `min_relevance`, see
[`min_relevance_universal`](configuration.md#search-section) in the configuration reference. The
predicate (`applies_when`) is the categorical complement to that numerical floor.

## Pattern language declaration

The optional `language:` frontmatter field declares which programming language(s) a pattern is
about. Declaring it lets lore's retrieval surface the pattern on a relevant tool call even when the
pattern body never mentions the canonical token in prose. Without a declaration, the pattern's
discoverability depends on body keywords — it surfaces only when its body happens to contain the
inferred-language word that lore extracts from the tool call.

### Purpose

Language detection runs over every tool call and produces an inferred-language set: a Rust file edit
infers `{rust}`, an `npm test` Bash command infers `{javascript, typescript}`, a `pyproject.toml`
edit infers `{python}`. The retrieval pipeline then admits patterns that either declare a language
matching the inferred set (structural gate) or have no declaration and whose body coincidentally
matches the canonical token (the fallback path).

Declaring `language:` is the durable way to participate in the structural gate: a pattern about Rust
error handling can use prose like "Use anyhow for errors" without the word "rust" anywhere in the
body, and still surface on a Rust file edit. Authors with patterns that already mention the
canonical token in prose can skip the declaration; lore will continue to find them through the
fallback path.

### Declaration is for eligibility, not ranking

Declaring `language: rust` does one thing: it tells lore the pattern is about Rust. That makes lore
_consider_ the pattern on every Rust tool call, even when the body never mentions "rust" in prose.
It does not, on its own, push the pattern up the ranking.

Once a pattern is eligible, lore decides which patterns are most relevant by where the query words
land:

- Words in a **heading** carry the most weight (e.g. `## Rust error handling`)
- Words in **tags** carry the next-most weight (e.g. `tags: [rust, errors]`)
- Words in the **body** carry the least

So a pattern with `language: rust` and a generic heading like "Use anyhow for application errors"
will surface on every Rust tool call, but might still rank below an undeclared pattern whose heading
reads `## Rust error handling` for the same query.

**Practical advice.** Treat the declaration and the heading/tags as separate levers:

1. Declare `language:` so lore always considers the pattern for that language.
2. Name the language in your heading too — the strongest ranking lever.
3. Use the language as a tag — second-strongest.
4. Body mentions are the weakest signal; do not rely on them alone.

### Syntax

Three forms are accepted, mirroring the `tags:` field:

```yaml
---
language: rust
---
```

```yaml
---
language: [javascript, typescript]
---
```

```yaml
---
language:
  - rust
  - golang
---
```

Tokens are case-insensitive in the source; lore normalises every token to lowercase at ingest. An
empty list (`language: []`) and a missing field both mean "no declaration" — both leave the pattern
on the fallback retrieval path.

### Canonical tokens

The initial recognised set covers the six languages lore detects today. Authors must declare the
canonical token in the second column; the display column shows how lore refers to the language in
prose and CLI output. The asymmetry is most visible for Go: the canonical token is `golang` because
bare `go` collides with the English stop-word list and the FTS5 default tokeniser.

| Display    | Canonical token |
| ---------- | --------------- |
| Rust       | `rust`          |
| TypeScript | `typescript`    |
| JavaScript | `javascript`    |
| YAML       | `yaml`          |
| Python     | `python`        |
| Go         | `golang`        |

Authors typing `language: go` will see a tier-2 warning at ingest naming the token as unknown; the
pattern still ingests, but the structural gate will never match it.

### When to declare a single token

Use the scalar form when the pattern is about exactly one language. A pattern that explains Rust's
lifetime elision rules, or one that documents Python virtual-environment conventions, declares a
single token because applying it to any other language would be wrong.

### When to declare a multi-value list

Use the list form when the pattern's content genuinely applies to a small set of languages, even if
the worked examples come from one ecosystem. A pattern that explains "validate at the boundary, not
in the middle" applies equally to TypeScript and JavaScript; a pattern about JSON-LD framing applies
to whichever ecosystem the consumer happens to use. The list captures applicability, not provenance
— it is not a place to enumerate languages the pattern's examples happen to mention.

### When to omit the field

Omit `language:` when the pattern applies to too many languages to enumerate, or when the content is
language-agnostic (cross-cutting concerns like git workflow, code review etiquette, accessibility
heuristics). Retrieval falls back to body-keyword matching — if the body contains relevant
vocabulary, lore will still surface the pattern.

### Composition with `applies_when` and `universal`

The three mechanisms are orthogonal:

- `universal` marks a pattern as always-on at `SessionStart`; the body is pinned into context
  regardless of retrieval.
- `applies_when` gates whether a universal pattern fires on a given `PreToolUse` call.
- `language:` gates retrieval ranking — independent of universality and the tool predicate.

A cross-language always-inject pattern uses `universal` (and optionally `applies_when`) with no
`language:` declaration; a language-specific reference pattern uses `language:` with no `universal`
tag.

### Validation behaviour

Unknown tokens trigger a tier-2 warn-and-proceed at ingest time. The pattern still ingests with the
offending token persisted verbatim, but the structural retrieval gate will never match it (the gate
compares against the canonical-token list above). Lore aggregates per-token across the whole ingest
run, so a 50-pattern repository that all share the same typo surfaces as one warning line, not
fifty.

### Worked examples

**A Rust-specific pattern using prose that lacks the canonical token** — retrieval surfaces it on
any Rust file edit even though the body never says "rust":

```yaml
---
title: Use anyhow for application errors
language: rust
---
```

**A pattern that applies to both JS and TS ecosystems**:

```yaml
---
title: Validate at the boundary, not in the middle
language: [javascript, typescript]
---
```

**A Python-specific pattern using the block-list form**:

```yaml
---
title: Pin pyproject.toml dependencies with caret ranges
language:
  - python
---
```

**A cross-language workflow pattern that omits the field**:

```yaml
---
title: Squash-merge feature branches
tags: [workflow, universal]
applies_when:
  bash_command_starts_with: [git, gh]
---
```

## File Structure and Grouping

Lore splits each markdown file into chunks by heading and indexes them separately. However, when any
chunk in a file matches a search query, lore injects **every chunk from that file** into the agent's
context. This sibling chunk expansion is intentional — it ensures the agent receives the full
picture rather than a fragment. But it has a direct consequence for how you organise your pattern
files.

**Group by domain, not by convenience.** Each file should contain sections that belong together. A
file about "Git Workflow" with sections on branching, commits, and pull requests is cohesive — when
one section matches, the others are almost certainly relevant too. A file that covers "Git Workflow"
and "YAML Formatting" in the same document is not — a YAML edit would drag in git conventions,
wasting context window space on irrelevant content.

**Keep files focused but not fragmented.** The goal is not one section per file. A pattern file with
a single three-line section provides little value from sibling expansion. Aim for files that cover
one coherent domain in enough depth to be useful as a group — typically three to eight sections.

**Practical guidance:**

- One concern per file: all sections should relate to the same domain or workflow
- Split a file when sections serve different audiences or trigger on different queries
- Merge files when you find yourself duplicating context across several tiny patterns about the same
  topic
- A good test: if the agent is editing a Rust file and one section matches, would the other sections
  in this file also be useful? If not, they belong in a separate file

### Excluding non-pattern files with `.loreignore`

A pattern repository often contains markdown files that are not patterns: a README, a CONTRIBUTING
guide, a LICENSE, or documentation about the patterns themselves. If these files end up in the
index, they pollute search results — an agent looking for a pattern about error handling does not
want a chunk from your README about how to clone the repository.

Place a `.loreignore` file at the root of your pattern repository to exclude them. The syntax is the
same as `.gitignore`:

```text
# Repository documentation
README.md
CONTRIBUTING.md
LICENSE

# Tooling and CI
.github/
ci/

# Drafts you do not want indexed yet
drafts/
**/*.draft.md

# Negation: explicitly include a file matched by an earlier pattern
!drafts/important.md
```

Lore reads `.loreignore` from the repository root only — nested files in subdirectories are not
supported. Patterns support bare filenames, trailing-slash directories, wildcards, recursive globs
(`**/*.draft.md`), anchoring (a leading slash anchors a pattern to the repository root), and
negation (`!`).

When you add or modify `.loreignore`, run `lore ingest` afterwards. The next ingest detects the
change and reconciles the database in both directions: files that newly match an exclusion are
removed, and files that are no longer excluded are re-indexed automatically. The same applies when
you delete `.loreignore` entirely — every file that had been excluded comes back into the index on
the next `lore ingest`.

> **Why is `.loreignore` opt-in?** Without a `.loreignore` file, every markdown file in the
> repository is indexed, exactly as before. The feature is purely additive — you only encounter it
> when you choose to.

## Anti-Patterns

### The Reference Document

A pattern that reads like a README section — describing what exists without stating what to do.

**Example:** "dprint is the single formatter for all file types" tells the agent what dprint is, but
not that it must run `dprint check` or `just fmt` before every commit.

**Fix:** Add the behavioural mandate. State the action, the trigger, and the consequence of omitting
it.

### The Vocabulary Island

A pattern that uses narrow terminology, missing the words agents actually search for.

**Example:** A pattern about "creating PRs" that never mentions "editing" or "updating." When an
agent edits a PR, the hook extracts "edit" — a term the pattern does not contain — and the pattern
scores zero.

**Fix:** Use the vocabulary coverage technique. List the verbs and nouns an agent would use, then
verify each appears in the body or tags.

### The Stop-Word Trap

A pattern whose key concept is invisible to search because critical terms are stop words or too
short.

**Example:** A pattern about `just ci` where "just" is a stop word and "ci" is only two characters.
When an agent runs `just ci`, the hook extracts zero queryable terms, and no search occurs for that
tool call.

**What helps:** Include terms that surface the pattern through adjacent queries. "Quality gate,"
"continuous integration," and "pre-commit check" give the search engine vocabulary to find the
pattern when the agent edits a file, runs a related command, or when the user's message provides
context. The pattern will not surface specifically during a `just ci` invocation — that is a
pipeline limitation — but it will surface during the surrounding workflow.

### The Kitchen Sink File

A single file covering unrelated topics. When any section matches, every section is injected — so a
file mixing "Error Handling," "YAML Formatting," and "Docker Configuration" pollutes the agent's
context with two irrelevant domains every time one matches.

**Example:** A file called `conventions.md` with sections on Rust error handling, Git branch naming,
CI pipeline configuration, and YAML quoting rules. An agent editing a `.rs` file triggers a match on
the Rust section, and the agent receives all four topics — three of which are noise.

**Fix:** Split into domain-cohesive files. See
[File Structure and Grouping](#file-structure-and-grouping) above.

### The Passive Observer

A pattern that uses suggestive language ("you might want to consider") rather than directive
language ("always," "never").

**Example:** "Consider using `--body-file` instead of `--body`" versus "Use `--body-file`. Inline
`--body` with heredocs is blocked by permission settings."

**Fix:** Replace "consider" and "might want to" with direct imperatives. Agents deprioritise
suggestions under time pressure; they follow directives.

## Debugging Pattern Injection

When a pattern does not surface as expected, two surfaces tell you where the breakdown happened.
Reach for `lore trace why` first; reach for `LORE_DEBUG` only for live single-invocation
investigation.

### `lore trace why` — durable, queryable, cross-session

Enable tracing once via `[trace] enabled = true` in `lore.toml` (or `LORE_TRACE=1` per session),
then inspect the records:

```sh
lore trace why <session-id>                              # full session
lore trace why <session-id> --tool Edit --json | jq      # filter + structured
lore trace why --recent 20 --event PreToolUse            # across sessions
```

Each record captures the extracted query, every candidate considered with its pre-fusion component
scores (FTS-fallback, FTS-structural, vector) and post-RRF combined score, the predicate outcome,
the deduplication decision, the final `injected` set, and per-phase timing. Records persist until
the retention horizon (default 30 days), so you can investigate "why didn't my pattern surface on
the call three minutes ago" without rerunning anything. See
[Per-Hook Trace Logging](configuration.md#per-hook-trace-logging) in the Configuration Reference for
the full setup.

### `LORE_DEBUG=1` — ephemeral, real-time, per-invocation

```sh
LORE_DEBUG=1 claude
```

Writes diagnostic lines to stderr with the prefix `[lore debug]` as the hook runs. Use this when you
want to watch one hook fire live, or when tracing wasn't enabled at the time of interest. The same
information ends up in trace records (and more), so prefer trace records for after-the- fact
investigation.

### What the diagnostics tell you

Either surface answers the same question: is the problem an **injection gap** (the query did not
produce terms that match your pattern) or a **compliance gap** (the pattern was injected but the
agent did not follow it)? Injection gaps are solved by improving vocabulary coverage. Compliance
gaps are solved by strengthening imperative voice and incident grounding.

## Pattern Review Checklist

Run through these checks before committing a new or updated pattern:

- [ ] **Imperative voice.** Does the pattern state what to do and what not to do? Would an agent
      reading it know exactly how to act?
- [ ] **Vocabulary coverage.** Do the title, body, and tags contain the terms an agent would use
      when this pattern should surface? Have you tested with `lore search`?
- [ ] **Stop-word avoidance.** Are key concepts expressed with terms longer than two characters and
      not in the stop-word list? If a critical term is a stop word, have you included a longer
      synonym?
- [ ] **Incident grounding.** If this rule exists because of a past failure, does the pattern
      explain what happened and why? (Skip this check for patterns without incident history.)
- [ ] **Tag relevance.** Do the tags add vocabulary that the body lacks, rather than duplicating
      prominent terms?
- [ ] **Stemming awareness.** Are you relying on exact word matches where stemming would help?
      "Testing" and "test" share a stem, but "edit" and "create" do not.
- [ ] **Actionable structure.** Does every section pass the "so what?" test — can a reader act on it
      immediately?
