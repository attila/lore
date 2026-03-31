use std::fs;
use std::path::Path;
use std::process::Command;

use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, FakeEmbedder};
use lore::ingest;
use tempfile::tempdir;

/// Initialise a git repo in `dir` with a test user identity.
fn git_init(dir: &Path) {
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir)
            .output()
            .expect("git command failed");
    }
}

/// Open an on-disk `KnowledgeDB` in the given directory.
fn open_db(dir: &Path, dims: usize) -> KnowledgeDB {
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, dims).expect("failed to open DB");
    db.init().expect("failed to init DB");
    db
}

/// Seed the knowledge directory with realistic markdown pattern files.
///
/// Each file uses distinctive terms that won't overlap in FTS queries.
fn seed_patterns(dir: &Path) {
    fs::write(
        dir.join("error-handling.md"),
        "# Error Handling\n\n\
         Always use anyhow for application-level error propagation.\n\
         Reserve thiserror for library crates that need typed error variants.\n\
         Never use unwrap in production paths; prefer context-rich bail macros.\n",
    )
    .unwrap();

    fs::write(
        dir.join("naming-conventions.md"),
        "# Naming Conventions\n\n\
         Use snake_case for all Rust function and variable names.\n\
         Prefer descriptive identifiers over abbreviations.\n\
         Module names should be singular nouns, not pluralised.\n",
    )
    .unwrap();

    fs::write(
        dir.join("testing-strategy.md"),
        "# Testing Strategy\n\n\
         Prefer integration tests that exercise real dependencies over mocks.\n\
         Use deterministic fakes only for external services like Ollama.\n\
         Every public function should have at least one happy-path test.\n",
    )
    .unwrap();
}

#[test]
#[allow(clippy::too_many_lines)]
fn full_lifecycle() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // -- Setup: seed files, init git, open DB ---------------------------------
    seed_patterns(dir);
    git_init(dir);

    // Initial commit so HEAD exists for later commits.
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial seed"])
        .current_dir(dir)
        .output()
        .unwrap();

    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    // -- Ingest ---------------------------------------------------------------
    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});

    assert_eq!(result.files_processed, 3);
    assert!(
        result.chunks_created >= 3,
        "expected >=3 chunks, got {}",
        result.chunks_created
    );
    assert!(
        result.errors.is_empty(),
        "unexpected ingest errors: {:?}",
        result.errors
    );

    // -- FTS search -----------------------------------------------------------
    let results = db.search_fts("anyhow", 10).unwrap();
    assert!(
        !results.is_empty(),
        "FTS search for 'anyhow' should find the error-handling pattern"
    );
    assert_eq!(results[0].source_file, "error-handling.md");

    let results = db.search_fts("Ollama", 10).unwrap();
    assert!(
        !results.is_empty(),
        "FTS search for 'Ollama' should find the testing-strategy pattern"
    );
    assert_eq!(results[0].source_file, "testing-strategy.md");

    // -- Hybrid search --------------------------------------------------------
    let query = "error propagation";
    let query_emb = embedder.embed(query).unwrap();
    let results = db.search_hybrid(query, Some(&query_emb), 5).unwrap();
    assert!(
        !results.is_empty(),
        "hybrid search for 'error propagation' should return results"
    );

    // -- Add pattern ----------------------------------------------------------
    let write_result = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Logging Guidelines",
        "Use structured logging with tracing crate.\n\
         Always include span context for distributed tracing.\n\
         Log at warn level for recoverable errors, error level for unrecoverable.\n",
        &["observability", "rust"],
    )
    .unwrap();

    assert_eq!(write_result.file_path, "logging-guidelines.md");
    assert!(
        write_result.chunks_indexed >= 1,
        "expected >=1 chunks indexed, got {}",
        write_result.chunks_indexed
    );
    assert!(
        write_result.committed,
        "add_pattern should commit in a git repo"
    );
    assert!(
        dir.join("logging-guidelines.md").exists(),
        "pattern file should exist on disk"
    );

    // -- Search finds newly added pattern -------------------------------------
    let results = db.search_fts("tracing", 10).unwrap();
    assert!(
        !results.is_empty(),
        "FTS search for 'tracing' should find the newly added logging pattern"
    );
    assert!(
        results
            .iter()
            .any(|r| r.source_file == "logging-guidelines.md"),
        "search results should include logging-guidelines.md"
    );

    // -- Update pattern -------------------------------------------------------
    let write_result = ingest::update_pattern(
        &db,
        &embedder,
        dir,
        "logging-guidelines.md",
        "Use structured logging exclusively via the tracing crate.\n\
         Instrument all async functions with tracing spans.\n\
         Never use println for diagnostic output in production.\n",
        &["observability", "production"],
    )
    .unwrap();

    assert!(write_result.chunks_indexed >= 1);

    // Old content should be gone from search results.
    let results = db.search_fts("distributed", 10).unwrap();
    assert!(
        results.is_empty()
            || results
                .iter()
                .all(|r| r.source_file != "logging-guidelines.md"),
        "old content 'distributed' should not appear in logging-guidelines results"
    );

    // New content should be findable.
    let results = db.search_fts("println", 10).unwrap();
    assert!(
        results
            .iter()
            .any(|r| r.source_file == "logging-guidelines.md"),
        "updated content 'println' should be found in logging-guidelines.md"
    );

    // -- Append to pattern ----------------------------------------------------
    let write_result = ingest::append_to_pattern(
        &db,
        &embedder,
        dir,
        "logging-guidelines.md",
        "Metrics",
        "Export Prometheus metrics for all critical paths.\n\
         Use histogram buckets aligned to SLO thresholds.\n",
    )
    .unwrap();

    assert!(write_result.chunks_indexed >= 1);

    // Appended content should be findable.
    let results = db.search_fts("Prometheus", 10).unwrap();
    assert!(
        results
            .iter()
            .any(|r| r.source_file == "logging-guidelines.md"),
        "appended content 'Prometheus' should be found in logging-guidelines.md"
    );

    // -- Stats ----------------------------------------------------------------
    let stats = db.stats().unwrap();
    // 3 seed files + 1 added = 4 sources
    assert_eq!(
        stats.sources, 4,
        "expected 4 source files, got {}",
        stats.sources
    );
    assert!(
        stats.chunks >= 4,
        "expected >=4 total chunks, got {}",
        stats.chunks
    );
}

#[test]
fn ingest_replaces_stale_data() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    // -- Seed two files and ingest --------------------------------------------
    fs::write(
        dir.join("alpha.md"),
        "# Alpha\n\nAlpha uses a waterfall deployment model for releases.\n",
    )
    .unwrap();
    fs::write(
        dir.join("beta.md"),
        "# Beta\n\nBeta requires canary deployments with gradual rollout.\n",
    )
    .unwrap();

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(result.files_processed, 2);

    let stats = db.stats().unwrap();
    assert_eq!(stats.sources, 2);

    // Verify both are searchable.
    assert!(!db.search_fts("waterfall", 10).unwrap().is_empty());
    assert!(!db.search_fts("canary", 10).unwrap().is_empty());

    // -- Delete alpha, modify beta, re-ingest ---------------------------------
    fs::remove_file(dir.join("alpha.md")).unwrap();
    fs::write(
        dir.join("beta.md"),
        "# Beta\n\nBeta now uses immutable infrastructure with automated rollback.\n",
    )
    .unwrap();

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(result.files_processed, 1);
    assert!(result.errors.is_empty());

    // -- Verify stale data is gone --------------------------------------------
    let stats = db.stats().unwrap();
    assert_eq!(stats.sources, 1, "only beta.md should remain");

    // Deleted file's content should not be found.
    let results = db.search_fts("waterfall", 10).unwrap();
    assert!(
        results.is_empty(),
        "deleted file's content 'waterfall' should not be found"
    );

    // Old content of modified file should not be found.
    let results = db.search_fts("canary", 10).unwrap();
    assert!(
        results.is_empty(),
        "old content 'canary' should not be found after re-ingest"
    );

    // New content of modified file should be found.
    let results = db.search_fts("immutable", 10).unwrap();
    assert!(
        !results.is_empty(),
        "new content 'immutable' should be found after re-ingest"
    );
    assert_eq!(results[0].source_file, "beta.md");
}
