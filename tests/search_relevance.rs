// SPDX-License-Identifier: MIT OR Apache-2.0

//! Search relevance regression tests.
//!
//! These tests validate that FTS5 search returns the correct patterns for
//! hook-generated queries. They catch regressions that would degrade hook
//! injection precision.
//!
//! All tests use `FakeEmbedder` and `hybrid = false` for deterministic,
//! Ollama-independent results.

use std::fs;
use std::path::Path;

use lore::config::Config;
use lore::database::{KnowledgeDB, SearchResult};
use lore::embeddings::{Embedder, FakeEmbedder};
use lore::hook;
use lore::ingest;

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

/// Seed the knowledge directory with pattern files covering distinct domains.
///
/// Each file uses distinctive terms that allow targeted FTS queries.
fn seed_patterns(dir: &Path) {
    // Rust conventions
    fs::create_dir_all(dir.join("rust")).unwrap();
    fs::write(
        dir.join("rust/error-handling.md"),
        "# Rust Error Handling\n\n\
         tags: rust, error-handling, conventions\n\n\
         Use anyhow for application-level error propagation.\n\
         Reserve thiserror for library crates that need typed error variants.\n\
         Never use unwrap in production paths; prefer context-rich bail macros.\n",
    )
    .unwrap();

    fs::write(
        dir.join("rust/testing-strategy.md"),
        "# Rust Testing Strategy\n\n\
         tags: rust, testing, strategy\n\n\
         Prefer integration tests that exercise real dependencies over mocks.\n\
         Use deterministic fakes only for external services like Ollama.\n\
         Every public function should have at least one happy-path test.\n\
         Use cargo test for running the full test suite.\n",
    )
    .unwrap();

    fs::write(
        dir.join("rust/clippy-pedantic.md"),
        "# Rust Clippy Pedantic\n\n\
         tags: rust, clippy, linting, pedantic\n\n\
         Enable clippy pedantic lints with `#![warn(clippy::pedantic)]`.\n\
         Allow `missing_errors_doc` and `must_use_candidate` at crate level.\n\
         Fix all pedantic warnings before merging; never suppress without reason.\n",
    )
    .unwrap();

    // JavaScript/TypeScript conventions
    fs::create_dir_all(dir.join("javascript-typescript")).unwrap();
    fs::write(
        dir.join("javascript-typescript/conventions.md"),
        "# TypeScript Conventions\n\n\
         tags: typescript, javascript, conventions\n\n\
         Prefer type over interface for object shapes.\n\
         Use arrow functions for all callbacks.\n\
         Always use named exports, never default exports.\n\
         Use strict TypeScript configuration with noImplicitAny.\n",
    )
    .unwrap();

    fs::write(
        dir.join("javascript-typescript/testing.md"),
        "# TypeScript Testing\n\n\
         tags: typescript, testing, vitest\n\n\
         Use vitest for all TypeScript test files.\n\
         Colocate test files next to source using the .test.ts suffix.\n\
         Prefer explicit assertions over snapshot tests.\n\
         Mock external HTTP calls with msw in integration tests.\n",
    )
    .unwrap();

    // YAML conventions
    fs::write(
        dir.join("yaml-formatting.md"),
        "# YAML Formatting\n\n\
         tags: yaml, formatting, configuration\n\n\
         Use 2-space indentation for all YAML files.\n\
         Quote strings that could be interpreted as booleans.\n\
         Prefer block scalars over flow style for multiline strings.\n",
    )
    .unwrap();

    // Git workflows
    fs::create_dir_all(dir.join("workflows")).unwrap();
    fs::write(
        dir.join("workflows/git-branch-pr.md"),
        "# Git Branch and Pull Request Workflow\n\n\
         tags: git, branch, pull-request, workflow\n\n\
         Never push directly to main; always use feature branches.\n\
         Keep pull request descriptions concise with a summary section.\n\
         Squash-merge feature branches to keep main history linear.\n\
         Use conventional commit messages in branch commits.\n",
    )
    .unwrap();

    // Agent conventions
    fs::create_dir_all(dir.join("agents")).unwrap();
    fs::write(
        dir.join("agents/unattended-work.md"),
        "# Unattended Work\n\n\
         tags: agent, unattended, command, bash, composite, git, gh\n\n\
         When running unattended or in composite shell commands, prefer \
         file-based arguments over inline strings. Use --body-file for \
         GitHub CLI operations instead of --body with heredocs.\n\
         Always verify exit codes in chained commands.\n\
         Never use interactive prompts in unattended agent workflows.\n",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Set up a temp directory with patterns ingested into a database.
///
/// Returns `(tmp_dir, config, db)`. The `tmp_dir` handle must stay alive.
fn setup_test_env() -> (tempfile::TempDir, Config, KnowledgeDB) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    seed_patterns(dir);

    let embedder = FakeEmbedder::new();
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, embedder.dimensions()).unwrap();
    db.init().unwrap();

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(
        result.chunks_created >= 8,
        "expected at least 8 chunks from 8 pattern files, got {}",
        result.chunks_created
    );

    let mut config = Config::default_with(dir.to_path_buf(), db_path, "nomic-embed-text");
    config.search.hybrid = false;
    config.search.min_relevance = 0.0;

    (tmp, config, db)
}

/// Run a search and return matching results.
fn search(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
    query: &str,
) -> Vec<SearchResult> {
    hook::search_with_threshold(db, embedder, config, query).expect("search should not fail")
}

/// Assert that at least one result's `source_file` contains the given substring.
fn assert_has_source(results: &[SearchResult], substring: &str, query: &str) {
    assert!(
        results.iter().any(|r| r.source_file.contains(substring)),
        "query {query:?} should return a result with source_file containing {substring:?}, \
         got: {:?}",
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Tests: Language-scoped queries
// ---------------------------------------------------------------------------

#[test]
fn query_rust_returns_rust_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(&db, &embedder, &config, "rust");
    assert!(!results.is_empty(), "query \"rust\" should return results");
    // All results should be from rust/ pattern files.
    for r in &results {
        assert!(
            r.source_file.starts_with("rust/"),
            "query \"rust\" should only return rust patterns, got: {}",
            r.source_file
        );
    }
}

#[test]
fn query_typescript_returns_typescript_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(&db, &embedder, &config, "typescript");
    assert!(
        !results.is_empty(),
        "query \"typescript\" should return results"
    );
    assert_has_source(&results, "javascript-typescript/", "typescript");
}

#[test]
fn query_yaml_returns_yaml_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(&db, &embedder, &config, "yaml");
    assert!(!results.is_empty(), "query \"yaml\" should return results");
    assert_has_source(&results, "yaml-formatting", "yaml");
}

// ---------------------------------------------------------------------------
// Tests: Compound AND/OR queries (simulating hook-generated queries)
// ---------------------------------------------------------------------------

#[test]
fn query_rust_and_testing_returns_testing_strategy() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(
        &db,
        &embedder,
        &config,
        "rust AND (testing OR test OR strategy)",
    );
    assert!(
        !results.is_empty(),
        "query for rust testing should return results"
    );
    assert_has_source(
        &results,
        "rust/testing-strategy",
        "rust AND (testing OR test OR strategy)",
    );
}

#[test]
fn query_git_branch_returns_workflow_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(&db, &embedder, &config, "git OR branch OR pull OR request");
    assert!(
        !results.is_empty(),
        "query for git/branch/pull/request should return results"
    );
    assert_has_source(
        &results,
        "workflows/git-branch-pr",
        "git OR branch OR pull OR request",
    );
}

#[test]
fn query_clippy_pedantic_returns_linting_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(&db, &embedder, &config, "clippy OR pedantic OR linting");
    assert!(
        !results.is_empty(),
        "query for clippy/pedantic/linting should return results"
    );
    assert_has_source(
        &results,
        "rust/clippy-pedantic",
        "clippy OR pedantic OR linting",
    );
}

// ---------------------------------------------------------------------------
// Tests: Uncovered domain returns no results
// ---------------------------------------------------------------------------

#[test]
fn query_uncovered_domain_returns_no_results() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();
    let results = search(&db, &embedder, &config, "kubernetes AND deployment");
    assert!(
        results.is_empty(),
        "query for uncovered domain \"kubernetes AND deployment\" should return no results, \
         got {} results: {:?}",
        results.len(),
        results.iter().map(|r| &r.source_file).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Tests: sanitize_fts_query preserves structured queries
// ---------------------------------------------------------------------------

#[test]
fn sanitize_preserves_and_or_queries() {
    use lore::database::sanitize_fts_query;

    let query = "rust AND (error OR handling)";
    let sanitized = sanitize_fts_query(query);
    assert_eq!(
        sanitized, query,
        "sanitize_fts_query should preserve AND/OR/parentheses"
    );
}

#[test]
fn sanitize_preserves_complex_hook_query() {
    use lore::database::sanitize_fts_query;

    let query = "typescript AND (validate OR email OR profile)";
    let sanitized = sanitize_fts_query(query);
    assert_eq!(
        sanitized, query,
        "sanitize_fts_query should preserve structured hook queries"
    );
}

// ---------------------------------------------------------------------------
// Tests: Hook-simulated queries (extract_query -> search pipeline)
// ---------------------------------------------------------------------------

#[test]
fn hook_query_for_rs_file_finds_rust_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    let input = lore::hook::HookInput {
        hook_event_name: "PreToolUse".to_string(),
        session_id: None,
        tool_name: Some("Edit".to_string()),
        tool_input: Some(serde_json::json!({"file_path": "src/error_handling.rs"})),
        agent_type: None,
        transcript_path: None,
        tool_response: None,
    };

    let query = hook::extract_query(&input).expect("should extract a query from .rs file");
    assert!(
        query.contains("rust"),
        "query should contain language anchor 'rust': {query}"
    );

    let results = search(&db, &embedder, &config, &query);
    assert!(
        !results.is_empty(),
        "hook query for .rs file should match rust patterns"
    );
    for r in &results {
        assert!(
            r.source_file.starts_with("rust/"),
            "hook query for .rs file should only return rust patterns, got: {}",
            r.source_file
        );
    }
}

#[test]
fn hook_query_for_ts_test_file_finds_typescript_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    let input = lore::hook::HookInput {
        hook_event_name: "PreToolUse".to_string(),
        session_id: None,
        tool_name: Some("Edit".to_string()),
        tool_input: Some(serde_json::json!({"file_path": "src/validators/email.test.ts"})),
        agent_type: None,
        transcript_path: None,
        tool_response: None,
    };

    let query = hook::extract_query(&input).expect("should extract a query from .test.ts file");
    assert!(
        query.contains("typescript"),
        "query should contain language anchor 'typescript': {query}"
    );

    let results = search(&db, &embedder, &config, &query);
    assert!(
        !results.is_empty(),
        "hook query for .test.ts file should match typescript patterns, query: {query}"
    );
    assert_has_source(&results, "javascript-typescript/", &query);
    // The testing pattern should surface because the filename "email.test"
    // generates the term "test" which matches the testing pattern.
    assert_has_source(&results, "testing.md", &query);
}

#[test]
fn hook_query_for_cargo_bash_finds_rust_patterns() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    let input = lore::hook::HookInput {
        hook_event_name: "PreToolUse".to_string(),
        session_id: None,
        tool_name: Some("Bash".to_string()),
        tool_input: Some(
            serde_json::json!({"description": "Run cargo clippy to check linting compliance"}),
        ),
        agent_type: None,
        transcript_path: None,
        tool_response: None,
    };

    let query = hook::extract_query(&input).expect("should extract a query from cargo bash");
    assert!(
        query.contains("rust"),
        "query should infer 'rust' from cargo: {query}"
    );

    let results = search(&db, &embedder, &config, &query);
    assert!(
        !results.is_empty(),
        "hook query for cargo clippy should match rust patterns"
    );
}

// ---------------------------------------------------------------------------
// Tests: Porter stemming — morphological variant recall
// ---------------------------------------------------------------------------

#[test]
fn stemming_testing_matches_test() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // "testing" appears in the testing-strategy pattern body.
    // Query with "test" should match via porter stemming (both stem to "test").
    let results = search(&db, &embedder, &config, "test");
    assert_has_source(&results, "rust/testing-strategy", "test");
}

#[test]
fn stemming_test_matches_testing_reverse() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // The testing-strategy pattern contains "test" in the title.
    // Query with "testing" should match via stemming.
    let results = search(&db, &embedder, &config, "testing");
    assert_has_source(&results, "rust/testing-strategy", "testing");
}

#[test]
fn stemming_fakes_matches_fakes_in_body() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // The testing-strategy pattern body contains "fakes".
    // Query with "fake" should match via porter stemming.
    let results = search(&db, &embedder, &config, "fake");
    assert_has_source(&results, "rust/testing-strategy", "fake");
}

#[test]
fn stemming_structured_query_with_and() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // "rust AND testing" should still work with stemming — both operands
    // are stemmed independently.
    let results = search(&db, &embedder, &config, "rust AND testing");
    assert!(
        !results.is_empty(),
        "structured AND query should work with stemming"
    );
    assert_has_source(&results, "rust/testing-strategy", "rust AND testing");
}

// ---------------------------------------------------------------------------
// Tests: Dogfooding regression — query reformulation gaps (PR #19 findings)
// ---------------------------------------------------------------------------

#[test]
fn dogfooding_natural_query_fake_matches_testing_strategy() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // Original dogfooding finding: "testing sqlite fake embedder" returned 0 hits.
    // We test the narrower "testing fake" (dropping "sqlite" and "embedder"
    // which don't appear in the target pattern) to verify that stemming alone
    // enables short, partial-match queries: "testing"→"test" and "fake"→"fakes".
    let results = search(&db, &embedder, &config, "testing fake");
    assert_has_source(&results, "rust/testing-strategy", "testing fake");
}

#[test]
fn dogfooding_verbose_query_matches_testing_strategy() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // The verbose query that originally worked (1.0 relevance).
    // Confirm it still works as a regression baseline.
    let results = search(
        &db,
        &embedder,
        &config,
        "testing strategy real dependencies fake externals",
    );
    assert_has_source(
        &results,
        "rust/testing-strategy",
        "testing strategy real dependencies fake externals",
    );
}

#[test]
fn dogfooding_natural_query_unattended_agent() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // Original dogfooding finding: "unattended agent work" returned 0 hits.
    // The pattern title and tags now contain these exact terms.
    let results = search(&db, &embedder, &config, "unattended agent work");
    assert_has_source(&results, "agents/unattended-work", "unattended agent work");
}

#[test]
fn dogfooding_verbose_query_matches_unattended_pattern() {
    let (_tmp, config, db) = setup_test_env();
    let embedder = FakeEmbedder::new();

    // The verbose query that originally worked (0.98 relevance).
    // Confirm it works against the seeded pattern.
    let results = search(&db, &embedder, &config, "agent unattended composite shell");
    assert_has_source(
        &results,
        "agents/unattended-work",
        "agent unattended composite shell",
    );
}
