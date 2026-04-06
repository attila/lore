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
`rust AND (validate OR email)`. When no language is detected, terms are joined with `OR`.

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

Before finalising a pattern, audit its term coverage. Delta ingest only detects committed changes,
so you need to commit the pattern before searching for it:

1. List the verbs and nouns an agent would use when the pattern should surface. For a pattern about
   pull request workflows, this might include: create, edit, update, merge, push, branch, PR, pull
   request, draft, review.

2. Commit the pattern and run an ingest so the search engine can find it:

   ```sh
   git add patterns/my-new-pattern.md
   git commit -m "wip: draft pattern for review"
   lore ingest
   ```

3. Search for your pattern using the terms from step 1:

   ```sh
   lore search "edit pull request" --top-k 3
   lore search "merge branch workflow" --top-k 3
   ```

4. If your pattern does not appear in the results, add the missing terms to the body or tags, amend
   the commit, and run `lore ingest` again.

5. Remember that stop words and short terms are invisible. If a key concept such as "CI" is too
   short, include a longer form such as "continuous integration" or "quality gate pipeline."

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

## Debugging with LORE_DEBUG

When a pattern does not surface as expected, trace the hook pipeline to identify where the breakdown
occurs.

Set the environment variable before running a session:

```sh
LORE_DEBUG=1 claude
```

The debug output writes to stderr with the prefix `[lore debug]` and shows:

- The extracted query terms and assembled FTS5 query
- The search results with scores and source files
- The deduplication decisions (which chunks were filtered as already injected)
- The final injected content

This tells you whether the problem is an **injection gap** (the query did not produce terms that
match your pattern) or a **compliance gap** (the pattern was injected but the agent did not follow
it). Injection gaps are solved by improving vocabulary coverage. Compliance gaps are solved by
strengthening imperative voice and incident grounding.

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
