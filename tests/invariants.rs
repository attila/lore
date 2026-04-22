//! Architectural invariants, enforced by static grep over `src/`.
//!
//! These tests exist to catch the class of regressions that silently broke
//! the DB-as-sole-read-surface invariant in PR #33. Static text-grep is
//! brittle by design: easy to update when an exemption is legitimately
//! added, loud when a regression tries to sneak in unreviewed.
//!
//! See `docs/architecture.md` for the invariants being enforced here.

use std::fs;
use std::path::PathBuf;

/// Collect all non-test `.rs` files under `src/`, excluding `#[cfg(test)]`
/// modules by the coarse proxy of "files whose names match `tests*` or sit
/// inside a `tests/` directory". `src/` itself has no tests subdirectory
/// (tests live in child modules via `#[cfg(test)] mod tests`), so this
/// returns all `src/*.rs` plus subdirectory source files — and the audit
/// strips `#[cfg(test)]` blocks textually before counting.
fn read_source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// Strip `#[cfg(test)] mod tests { ... }` blocks from source so the audit
/// focuses on runtime code. Matches a `#[cfg(test)]` attribute followed by
/// `mod ` on the same or next line; elides everything up to the matching
/// closing `}` by tracking brace depth.
///
/// Not robust against arbitrary `#[cfg(test)]` placement, but sufficient
/// for this codebase where all test modules follow the same shape.
fn strip_test_modules(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(idx) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..idx]);
        // Find the opening brace of the `mod tests` block.
        let after_attr = &rest[idx + "#[cfg(test)]".len()..];
        let Some(brace_rel) = after_attr.find('{') else {
            rest = after_attr;
            break;
        };
        // Walk from the opening brace tracking depth.
        let mut depth: i32 = 0;
        let mut end = None;
        for (i, c) in after_attr[brace_rel..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(brace_rel + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = end {
            rest = &after_attr[e..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

fn count_substring(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

// ---------------------------------------------------------------------------
// Invariant 1: pattern-level queries do not scan `chunks`
// ---------------------------------------------------------------------------

/// `list_patterns`, `universal_patterns`, `stats().sources`, and
/// `source_files` all query the `patterns` table directly (as of the
/// db-sole-read-surface PR). The only remaining `DISTINCT` / `GROUP BY`
/// over `source_file` in `src/` is the `search_hybrid` aggregation at
/// `src/database.rs:362` (or wherever the query has moved), which operates
/// over search matches — not pattern-level state. Any new match fails this
/// test and forces the author to either add a documented exemption or
/// migrate the caller to `patterns`.
#[test]
fn no_distinct_or_group_by_over_source_file_in_runtime_src() {
    // After the db-sole-read-surface migration, every pattern-level
    // question ("which files exist?", "which are universal?", "how many
    // sources?") is answered by querying the `patterns` table. No runtime
    // code derives pattern-level state by aggregating `chunks` rows.
    //
    // `search_hybrid` does not use GROUP BY on source_file — it uses
    // reciprocal rank fusion over FTS and vector result lists.
    //
    // Any match here is a regression. Update this test only if a future
    // query legitimately needs per-source aggregation (e.g. a hypothetical
    // "count chunks per source" diagnostic) — and in that case, document
    // the exemption inline.
    for file in [
        "database.rs",
        "ingest.rs",
        "hook.rs",
        "server.rs",
        "main.rs",
    ] {
        let source = strip_test_modules(&read_source(file));
        let hits = count_substring(&source, "DISTINCT source_file")
            + count_substring(&source, "COUNT(DISTINCT source_file)")
            + count_substring(&source, "GROUP BY c1.source_file")
            + count_substring(&source, "GROUP BY source_file");
        assert_eq!(
            hits, 0,
            "src/{file} must not aggregate pattern-level state from chunks; \
             query the `patterns` table instead"
        );
    }
}

// ---------------------------------------------------------------------------
// Invariant 2: no runtime disk reads outside the sanctioned allow-list
// ---------------------------------------------------------------------------

/// The DB-as-sole-read-surface invariant: runtime code (the hook, MCP
/// server, CLI dispatcher) must not read indexed-content files from disk.
/// Ingest is the sole sanctioned disk→DB pipeline; authoring writes via
/// `add_pattern` / `update_pattern` / `append_to_pattern` pass through
/// ingest afterwards. This test catches the class of regression that PR
/// #33 introduced — a new `std::fs::read_to_string` call in a runtime
/// code path without an architectural justification.
///
/// The allow-list below names every *non*-indexed-content disk read in
/// the runtime modules. If a new one legitimately needs to land, the
/// author updates this test (forcing the reviewer to see the change) and
/// documents the carve-out in `docs/architecture.md`.
#[test]
fn no_unsanctioned_runtime_disk_reads_in_hook_server_main() {
    // src/hook.rs — runtime disk I/O is strictly session-local state and
    // agent-harness inputs, never indexed content:
    //   * dedup file ops: read_dedup, write_dedup, reset_dedup,
    //     dedup_filter_and_record — 4 `OpenOptions` usages plus one
    //     `fs::read_to_string` (read_dedup).
    //   * transcript tail: last_user_message — 1 `File::open`.
    let hook = strip_test_modules(&read_source("hook.rs"));
    assert_eq!(
        count_substring(&hook, "std::fs::read_to_string"),
        1,
        "hook.rs runtime fs::read_to_string count changed — the only allowed \
         call is the dedup-file read in read_dedup. A new occurrence means \
         either a new dedup-file operation (update this test) or a regression \
         against the DB-as-sole-read-surface invariant."
    );
    assert_eq!(
        count_substring(&hook, "std::fs::File::open"),
        1,
        "hook.rs File::open count changed — the only allowed call is the \
         transcript tail read in last_user_message."
    );
    assert_eq!(
        count_substring(&hook, "std::fs::OpenOptions"),
        3,
        "hook.rs OpenOptions count changed — allowed: write_dedup, \
         reset_dedup, dedup_filter_and_record."
    );

    // src/server.rs — the MCP server does not read the filesystem at
    // runtime. Its write-path tools (add_pattern etc.) route through
    // ingest, not through direct fs calls in server.rs.
    let server = strip_test_modules(&read_source("server.rs"));
    assert_eq!(
        count_substring(&server, "std::fs::read_to_string")
            + count_substring(&server, "std::fs::File::open")
            + count_substring(&server, "std::fs::OpenOptions"),
        0,
        "server.rs must not contain runtime disk reads outside test modules"
    );

    // src/main.rs — the CLI dispatcher reads stdin for the hook
    // subcommand, which is not a filesystem read. No runtime fs:: reads
    // should appear here; ingest and config loading live elsewhere.
    let main = strip_test_modules(&read_source("main.rs"));
    assert_eq!(
        count_substring(&main, "std::fs::read_to_string")
            + count_substring(&main, "std::fs::File::open")
            + count_substring(&main, "std::fs::OpenOptions"),
        0,
        "main.rs must not read files directly — config goes through \
         config.rs::Config::load, ingest through ingest.rs."
    );
}
