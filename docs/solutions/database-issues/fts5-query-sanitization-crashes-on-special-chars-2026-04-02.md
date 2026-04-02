---
title: "FTS5 MATCH crashes on dots, slashes, and special characters in queries"
date: 2026-04-02
category: database-issues
module: database
problem_type: database_issue
component: database
symptoms:
  - "lore search 'path/to/file.ts' exits with SQLite error"
  - "FTS5 MATCH query fails silently when input contains dots or slashes"
  - "Queries with colons, braces, quotes, or asterisks crash the search"
root_cause: missing_validation
resolution_type: code_fix
severity: high
tags:
  - fts5
  - sqlite
  - sanitization
  - query
  - special-characters
  - search
---

# FTS5 MATCH crashes on dots, slashes, and special characters in queries

## Problem

SQLite FTS5 interprets certain characters as syntax operators. When user input containing dots,
slashes, colons, braces, quotes, asterisks, or carets is passed directly to a MATCH clause, FTS5
throws a parse error and the query fails.

## Symptoms

- `lore search "path/to/file.ts"` exits with a non-zero exit code
- `lore search "typescript validateEmail.ts"` returns an error instead of results
- Any query containing `.`, `/`, `\`, `:`, `{`, `}`, `[`, `]`, `"`, `'`, `*`, `^` fails

## What Didn't Work

- Quoting the entire query string (FTS5 interprets quotes as phrase search delimiters)
- Escaping individual characters with backslash (FTS5 does not support backslash escaping)

## Solution

Sanitize queries before passing to MATCH by replacing FTS5-unsafe characters with spaces, then
collapsing whitespace:

```rust
pub fn sanitize_fts_query(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|c| match c {
            '.' | '/' | '\\' | ':' | '{' | '}' | '[' | ']' | '"' | '\'' | '*' | '^' => ' ',
            _ => c,
        })
        .collect();

    // Strip leading minus from each term (FTS5 NOT operator).
    let result: Vec<&str> = cleaned
        .split_whitespace()
        .map(|term| term.trim_start_matches('-'))
        .filter(|term| !term.is_empty())
        .collect();

    result.join(" ")
}
```

**Key decisions:**

- Parentheses `()` and keywords `AND`, `OR`, `NOT` are preserved so callers can construct structured
  FTS5 queries (e.g., `rust AND (error OR handling)`)
- Leading minus is stripped because FTS5 treats it as the NOT operator
- The function applies to all user-facing search paths (CLI `lore search`, MCP `search_patterns`,
  hook pipeline)

## Why This Works

FTS5 has its own query grammar where certain characters are operators:

- `.` is a column filter separator
- `*` is a prefix search operator
- `^` is a phrase start marker
- `"` and `'` delimit phrases
- `-` at term start is the NOT operator

Replacing these with spaces converts the query into a simple term list, which FTS5 handles safely.
The terms are then matched individually with implicit OR semantics (FTS5's default behavior for
space-separated terms).

## Prevention

- Always sanitize user input before FTS5 MATCH — never pass raw strings
- Apply sanitization in a single shared function called by all search paths
- Test with file paths, URLs, and punctuation-heavy strings as queries
- Consider adding a test case for every FTS5 special character to catch regressions
