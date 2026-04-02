---
title: "FTS5 query construction: language anchor + OR enrichment for hook-based search"
date: 2026-04-02
category: database-issues
module: database
problem_type: database_issue
component: database
symptoms:
  - "Single-keyword FTS5 queries return too few results from the knowledge base"
  - "File path queries crash FTS5 (see separate sanitization learning)"
  - "MIN(id) SQL selects wrong chunk when heading names sort before 'root'"
  - "Language-only query with empty enrichment terms produces invalid FTS5 syntax"
root_cause: logic_error
resolution_type: code_fix
severity: medium
tags:
  - fts5
  - sqlite
  - query-construction
  - language-anchor
  - enrichment
  - hooks
  - search-relevance
---

# FTS5 query construction: language anchor + OR enrichment for hook-based search

## Problem

Building effective FTS5 search queries from Claude Code hook input signals required iterative
experimentation. Single keywords, raw file paths, and naive term extraction all produced poor results
or outright failures.

## Symptoms

- Query `"typescript"` returned only one chunk (e.g., Error Handling) but missed Functions, Naming,
  Exports sections from the same TypeScript conventions document
- Query `"validateEmail.ts"` crashed FTS5 due to dots and slashes
- Query `"typescript validateEmail"` returned nothing because `"validateEmail"` is not a term in the
  knowledge base (FTS5 uses implicit AND for space-separated terms)

## What Didn't Work

- **Single keyword** (`"typescript"`) — too few results, misses related chunks
- **Raw file path** (`"src/validateEmail.ts"`) — FTS5 crashes on special chars
- **Space-separated terms** (`"typescript validateEmail"`) — implicit AND semantics returns nothing
  when both terms must match

## Solution

Three validated techniques that compose into an effective query strategy:

### 1. Language anchor + OR enrichment

Build FTS5 queries as `lang AND (term1 OR term2 OR ...)`:

```rust
// Language from file extension is mandatory AND anchor
// Enrichment terms are OR'd for broad matching
match (language, cleaned.is_empty()) {
    (Some(lang), false) => {
        let or_clause = cleaned.join(" OR ");
        Some(format!("{lang} AND ({or_clause})"))
    }
    // Language only (no enrichment survived cleaning)
    (Some(lang), true) => Some(lang),
    // No language, just enrichment terms
    (None, false) => Some(cleaned.join(" OR ")),
    (None, true) => None,
}
```

The language anchor (`.ts` -> `typescript`, `.rs` -> `rust`) prevents cross-domain pollution. OR
enrichment terms from filename camelCase split, transcript tail, and Bash description broaden
matching without requiring all terms to be present.

**Edge case:** when all enrichment terms are filtered by the cleaning pipeline (stop words, short
terms, hex-like strings), the query must fall back to language-only rather than producing invalid
`lang AND ()` syntax.

### 2. Sibling chunk injection

When any chunk from a source file matches, fetch ALL chunks from that file:

```rust
let source_files: Vec<&str> = results.iter()
    .filter_map(|r| /* deduplicate by source_file */)
    .collect();
let results = db.chunks_by_sources(&source_files).unwrap_or(results);
```

This ensures related conventions (Functions, Naming, Exports) come along when one section (Error
Handling) matches. Without this, a query matching only one heading would inject incomplete guidance.

### 3. Shallowest chunk selection for pattern index

When selecting one representative chunk per document (e.g., for `lore list`), do NOT use `MIN(id)`:

```sql
-- WRONG: lexicographic MIN — "Error Handling" sorts before "root"
SELECT ... WHERE id IN (SELECT MIN(id) FROM chunks GROUP BY source_file)

-- CORRECT: shortest heading_path = shallowest/root chunk
SELECT ... WHERE id IN (
    SELECT id FROM chunks c1
    WHERE LENGTH(c1.heading_path) = (
        SELECT MIN(LENGTH(c2.heading_path))
        FROM chunks c2 WHERE c2.source_file = c1.source_file
    )
    GROUP BY source_file
)
```

Chunk IDs are `source_file:heading_path` (e.g., `conventions.md:Error Handling`). Lexicographic
`MIN` picks whichever heading name sorts first alphabetically, not the document root.

## Why This Works

- **OR mode** matches any enrichment term, not all of them, dramatically increasing recall
- **Language anchor** as mandatory AND prevents TypeScript conventions from appearing when editing
  Rust files
- **Sibling expansion** leverages the document's heading structure — if any section is relevant, the
  whole document likely contains useful conventions
- **heading_path length** is a reliable proxy for heading depth: root chunks have empty
  `heading_path` (length 0), first-level headings have short paths, nested headings have longer
  paths with `>` separators

## Prevention

- Always use explicit OR when combining multiple optional search terms in FTS5
- Never use `MIN(id)` on text IDs to find "first" or "shallowest" — use a semantic ordering column
- Test query construction with edge cases: all terms filtered, language-only, no language, file paths
  with special characters
