use std::cell::RefCell;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lore::config::Config;
use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, FailingEmbedder, FakeEmbedder};
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
    let db = KnowledgeDB::open(&db_path).expect("failed to open DB");
    db.init().expect("failed to init DB");
    db.ensure_vector_table(dims)
        .expect("failed to create vector table");
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
        &[],
        None,
    )
    .unwrap();

    assert_eq!(write_result.file_path, "logging-guidelines.md");
    assert!(
        write_result.chunks_indexed >= 1,
        "expected >=1 chunks indexed, got {}",
        write_result.chunks_indexed
    );
    assert!(
        matches!(
            write_result.commit_status,
            lore::ingest::CommitStatus::Committed
        ),
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
        Some(&["observability", "production"]),
        None,
        None,
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
        None,
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

// -- Missing vectors ----------------------------------------------------------

/// Commit everything in `dir` and return the new HEAD.
fn commit_all(dir: &Path) -> String {
    for args in [vec!["add", "."], vec!["commit", "-m", "seed"]] {
        Command::new("git")
            .args(&args)
            .current_dir(dir)
            .output()
            .unwrap();
    }
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(head.stdout).unwrap().trim().to_string()
}

fn memory_db() -> KnowledgeDB {
    let db = KnowledgeDB::open(Path::new(":memory:")).unwrap();
    db.init().unwrap();
    db
}

/// Run a plain `lore ingest` against `db`, returning its result and every
/// progress line.
fn ingest_logged(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    dir: &Path,
) -> (ingest::IngestResult, Vec<String>) {
    let lines = RefCell::new(Vec::new());
    let result = ingest::ingest(db, embedder, dir, "heading", &|m| {
        lines.borrow_mut().push(m.to_string());
    });
    (result, lines.into_inner())
}

fn logged(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

/// Embeds like [`FakeEmbedder`] but fails every text containing `refuse`.
struct RefusingEmbedder {
    inner: FakeEmbedder,
    refuse: &'static str,
}

impl Embedder for RefusingEmbedder {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        if input.contains(self.refuse) {
            anyhow::bail!("refused");
        }
        self.inner.embed(input)
    }

    fn provider(&self) -> &str {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }
}

#[test]
fn a_chunk_that_failed_to_embed_gains_its_vector_on_the_next_ingest() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    let head = commit_all(dir);
    let db = memory_db();

    let refusing = RefusingEmbedder {
        inner: FakeEmbedder::new(),
        refuse: "snake_case",
    };
    let (result, _) = ingest_logged(&db, &refusing, dir);
    assert_eq!(
        result.errors,
        ["1 embedding failure(s) while indexing naming-conventions.md"]
    );
    assert_eq!(
        db.get_metadata("last_ingested_commit").unwrap(),
        Some(head.clone()),
        "an embedding failure must not hold back HEAD"
    );
    assert_eq!(db.chunks_without_vectors().unwrap().len(), 1);

    let (result, _) = ingest_logged(&db, &FailingEmbedder::new(768), dir);
    assert_eq!(
        result.errors,
        [
            "1 chunk(s) have no vector and the embedding model did not answer: \
             Ollama is unreachable"
        ]
    );

    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), dir);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(logged(&lines, "Embedded 1 chunk(s) that had no vector"));
    assert!(logged(&lines, "Already up to date"), "{lines:?}");
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));
}

#[test]
fn a_delta_ingest_records_head_when_a_chunk_fails_to_embed() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FakeEmbedder::new(), dir);

    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    let head = commit_all(dir);
    let refusing = RefusingEmbedder {
        inner: FakeEmbedder::new(),
        refuse: "stderr",
    };
    let (result, lines) = ingest_logged(&db, &refusing, dir);
    assert!(
        logged(&lines, "Delta ingest: 1 file(s) changed"),
        "{lines:?}"
    );
    assert_eq!(
        result.errors,
        [
            "1 embedding failure(s) while indexing logging.md",
            "1 embedding failure(s) while adding missing vectors"
        ]
    );
    assert_eq!(
        db.get_metadata("last_ingested_commit").unwrap(),
        Some(head.clone()),
        "an embedding failure must not hold back HEAD"
    );
    assert_eq!(db.chunks_without_vectors().unwrap().len(), 1);

    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), dir);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(logged(&lines, "Already up to date"), "{lines:?}");
    assert!(logged(&lines, "Embedded 1 chunk(s) that had no vector"));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));
}

#[test]
fn a_chunk_replaced_by_the_delta_is_not_embedded_first() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    let refusing = RefusingEmbedder {
        inner: FakeEmbedder::new(),
        refuse: "snake_case",
    };
    ingest_logged(&db, &refusing, dir);
    assert_eq!(db.chunks_without_vectors().unwrap().len(), 1);

    fs::write(
        dir.join("naming-conventions.md"),
        "# Naming\n\nName things plainly.\n",
    )
    .unwrap();
    commit_all(dir);
    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(
        logged(&lines, "Delta ingest: 1 file(s) changed"),
        "{lines:?}"
    );
    assert!(!logged(&lines, "that have no vector"), "{lines:?}");
    assert!(db.chunks_without_vectors().unwrap().is_empty());
}

#[test]
fn a_text_only_index_gains_vectors_on_the_next_ingest_that_reaches_the_model() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    let head = commit_all(dir);
    let db = memory_db();

    let unanswered = |lines: &[String]| {
        lines
            .iter()
            .filter(|l| l.starts_with("Embedding model did not answer (Ollama is unreachable) — "))
            .count()
    };
    let (result, lines) = ingest_logged(&db, &FailingEmbedder::new(768), dir);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(unanswered(&lines), 1, "{lines:?}");
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));

    let (result, lines) = ingest_logged(&db, &FailingEmbedder::new(768), dir);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(logged(&lines, "Already up to date"), "{lines:?}");
    assert_eq!(unanswered(&lines), 1, "{lines:?}");
    assert_eq!(db.vector_dimensions().unwrap(), None);

    let embedder = FakeEmbedder::with_model("some-unlisted-model", 1024);
    let (result, lines) = ingest_logged(&db, &embedder, dir);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(logged(&lines, "Already up to date"), "{lines:?}");
    assert_eq!(db.vector_dimensions().unwrap(), Some(1024));
    assert_eq!(
        db.get_metadata("embedding_model").unwrap().as_deref(),
        Some("some-unlisted-model")
    );
    assert_eq!(
        db.get_metadata("embedding_dimensions").unwrap().as_deref(),
        Some("1024")
    );
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    let query = embedder.embed("error propagation").unwrap();
    assert_eq!(db.search_vector(&query, 10).unwrap().len(), 3);
}

#[test]
fn a_text_only_index_with_new_commits_embeds_old_and_changed_files() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FailingEmbedder::new(768), dir);

    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    commit_all(dir);
    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.files_processed, 1);
    assert!(logged(&lines, "Embedding 4 chunk(s) that have no vector"));
    assert!(logged(&lines, "Embedded 4 chunk(s) that had no vector"));
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.stats().unwrap().chunks, 4);
}

/// An index as lore 0.5.0 left it: a vector for every chunk in a table of
/// `width`, HEAD recorded, and no recorded embedding model.
fn v0_5_index(tmp: &Path, width: usize) -> (PathBuf, KnowledgeDB) {
    let dir = tmp.join("knowledge");
    fs::create_dir_all(&dir).unwrap();
    seed_patterns(&dir);
    git_init(&dir);
    let head = commit_all(&dir);
    let db_path = tmp.join("knowledge.db");
    {
        let db = KnowledgeDB::open(&db_path).unwrap();
        db.init().unwrap();
        ingest::ingest(
            &db,
            &FakeEmbedder::with_dimensions(width),
            &dir,
            "heading",
            &|_| {},
        );
    }
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute(
            "DELETE FROM ingest_metadata WHERE key LIKE 'embedding\\_%' ESCAPE '\\'",
            [],
        )
        .unwrap();
    let db = KnowledgeDB::open(&db_path).unwrap();
    assert_eq!(db.recorded_embedding_model().unwrap(), None);
    assert_eq!(db.vector_dimensions().unwrap(), Some(width));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));
    (dir, db)
}

const REBUILD_UNRECORDED: &str = "No embedding model recorded with the index — running full ingest";

fn rebuilds(lines: &[String]) -> usize {
    lines.iter().filter(|l| *l == REBUILD_UNRECORDED).count()
}

#[test]
fn an_index_without_a_recorded_model_is_rebuilt_once_then_ingested_by_delta() {
    let tmp = tempdir().unwrap();
    let (dir, db) = v0_5_index(tmp.path(), 768);

    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), &dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.mode, ingest::IngestMode::Full);
    assert_eq!(rebuilds(&lines), 1, "{lines:?}");
    assert_eq!(
        db.recorded_embedding_model().unwrap().unwrap().model,
        "nomic-embed-text"
    );
    assert!(db.get_metadata("embedding_probe_vector").unwrap().is_some());
    assert!(db.chunks_without_vectors().unwrap().is_empty());

    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    commit_all(&dir);
    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), &dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.mode, ingest::IngestMode::Delta { unchanged: 3 });
    assert_eq!(rebuilds(&lines), 0, "{lines:?}");
    assert!(db.chunks_without_vectors().unwrap().is_empty());
}

#[test]
fn an_index_without_a_recorded_model_is_rebuilt_at_the_measured_width() {
    let tmp = tempdir().unwrap();
    let (dir, db) = v0_5_index(tmp.path(), 4);

    let embedder = FakeEmbedder::with_model("other-model", 768);
    let (result, lines) = ingest_logged(&db, &embedder, &dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(rebuilds(&lines), 1, "{lines:?}");
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    assert_eq!(
        db.recorded_embedding_model().unwrap().unwrap().model,
        "other-model"
    );
    let query = embedder.embed("error propagation").unwrap();
    assert_eq!(db.search_vector(&query, 10).unwrap().len(), 3);
}

#[test]
fn an_index_without_a_recorded_model_keeps_its_vectors_while_the_model_is_silent() {
    let tmp = tempdir().unwrap();
    let (dir, db) = v0_5_index(tmp.path(), 768);

    let (result, lines) = ingest_logged(&db, &FailingEmbedder::new(768), &dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(rebuilds(&lines), 0, "{lines:?}");
    assert!(logged(&lines, "Already up to date"), "{lines:?}");
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.recorded_embedding_model().unwrap(), None);
}

#[test]
fn an_interrupted_rebuild_resumes_as_a_full_ingest() {
    let tmp = tempdir().unwrap();
    let (dir, db) = v0_5_index(tmp.path(), 768);
    let identity = lore::embeddings::EmbeddingIdentity::measure(&FakeEmbedder::new()).unwrap();
    // The rebuild cleared the index and recorded the model, then stopped
    // before its first file.
    db.clear_all(Some(&identity)).unwrap();

    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), &dir);

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.mode, ingest::IngestMode::Full, "{lines:?}");
    assert!(
        logged(&lines, "No previous ingest recorded — running full ingest"),
        "{lines:?}"
    );
    for term in ["anyhow", "abbreviations", "mocks"] {
        assert_eq!(db.search_fts(term, 10).unwrap().len(), 1, "{term}");
    }
    assert!(db.chunks_without_vectors().unwrap().is_empty());
}

/// Answers under [`FakeEmbedder`]'s name with vectors pointing the other
/// way, as another model pulled under the same name would.
struct SwappedEmbedder(FakeEmbedder);

impl Embedder for SwappedEmbedder {
    fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        Ok(self.0.embed(input)?.into_iter().map(|x| -x).collect())
    }

    fn provider(&self) -> &str {
        self.0.provider()
    }

    fn model(&self) -> &str {
        self.0.model()
    }
}

#[test]
fn ingest_refuses_another_model_behind_the_recorded_name_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    let head = commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FakeEmbedder::new(), dir);
    let loreignore_hash = db.get_metadata("loreignore_hash").unwrap();
    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    fs::write(dir.join(".loreignore"), "naming-conventions.md\n").unwrap();
    commit_all(dir);
    let swapped = SwappedEmbedder(FakeEmbedder::new());
    let probe_refusal = "Refusing to ingest: nomic-embed-text is not the model that built the \
                         index under that name: its vectors match the recorded ones at cosine \
                         similarity -1.000, below 0.99";

    let (result, lines) = ingest_logged(&db, &swapped, dir);

    let refused = result.refused.expect("a swapped model must be refused");
    assert!(refused.starts_with(probe_refusal), "{refused}");
    assert!(refused.ends_with("Run 'lore ingest --force' to rebuild the index."));
    assert!(lines.is_empty(), "{lines:?}");
    assert!(db.search_fts("stderr", 10).unwrap().is_empty());
    assert_eq!(db.search_fts("abbreviations", 10).unwrap().len(), 1);
    assert_eq!(db.get_metadata("loreignore_hash").unwrap(), loreignore_hash);
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));

    let result = ingest::ingest_single_file(
        &db,
        &swapped,
        dir,
        &dir.join("logging.md"),
        "heading",
        false,
        &|_| {},
    );
    assert!(
        result
            .refused
            .as_deref()
            .is_some_and(|r| r.starts_with(probe_refusal)),
        "{:?}",
        result.refused
    );
    assert!(db.search_fts("stderr", 10).unwrap().is_empty());

    ingest::full_ingest(&db, &swapped, dir, "heading", &|_| {});
    let (result, _) = ingest_logged(&db, &swapped, dir);
    assert_eq!(result.refused, None);
    assert_eq!(db.search_fts("stderr", 10).unwrap().len(), 1);
    assert!(db.search_fts("abbreviations", 10).unwrap().is_empty());
}

/// Search `db` the way the hook does with [`FakeEmbedder`], under a
/// relevance floor no hybrid score reaches, so a result proves the search
/// fell back to text.
fn search_error_propagation(db: &KnowledgeDB, dir: &Path) -> (Vec<String>, lore::trace::Phases) {
    let mut config = Config::default_with(
        dir.to_path_buf(),
        dir.join("knowledge.db"),
        "nomic-embed-text",
    );
    config.search.min_relevance = 1.5;
    let (results, phases) = lore::hook::search_with_threshold(
        db,
        &FakeEmbedder::new(),
        &config,
        "anyhow error propagation",
    )
    .unwrap();
    (results.into_iter().map(|r| r.title).collect(), phases)
}

#[test]
fn searches_use_text_only_once_a_write_finds_another_model_under_the_name_until_a_rebuild() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &SwappedEmbedder(FakeEmbedder::new()), dir);

    let (titles, phases) = search_error_propagation(&db, dir);
    assert!(titles.is_empty(), "{titles:?}");
    assert!(phases.search_vector_ms.is_some());

    let (result, _) = ingest_logged(&db, &FakeEmbedder::new(), dir);
    assert!(result.refused.is_some());
    assert!(
        db.recorded_probe_mismatch()
            .unwrap()
            .is_some_and(|similarity| similarity < 0.99)
    );

    let (titles, phases) = search_error_propagation(&db, dir);
    assert_eq!(titles, ["Error Handling"]);
    assert_eq!(phases.embedding_ms, None);

    ingest::full_ingest(&db, &FakeEmbedder::new(), dir, "heading", &|_| {});
    assert_eq!(db.recorded_probe_mismatch().unwrap(), None);
    let (titles, phases) = search_error_propagation(&db, dir);
    assert!(titles.is_empty(), "{titles:?}");
    assert!(phases.search_vector_ms.is_some());
}

#[test]
fn a_write_records_a_probe_mismatch_and_a_write_whose_probe_matches_clears_it() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    let swapped = SwappedEmbedder(FakeEmbedder::new());
    ingest_logged(&db, &swapped, dir);
    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    let ingest_file = |embedder: &dyn Embedder| {
        ingest::ingest_single_file(
            &db,
            embedder,
            dir,
            &dir.join("logging.md"),
            "heading",
            false,
            &|_| {},
        )
    };

    let (result, _) = ingest_logged(&db, &FakeEmbedder::new(), dir);
    assert!(result.refused.is_some());
    assert!(db.recorded_probe_mismatch().unwrap().is_some());
    let (result, _) = ingest_logged(&db, &swapped, dir);
    assert_eq!(result.refused, None);
    assert_eq!(db.recorded_probe_mismatch().unwrap(), None);

    assert!(ingest_file(&FakeEmbedder::new()).refused.is_some());
    assert!(db.recorded_probe_mismatch().unwrap().is_some());
    assert_eq!(ingest_file(&swapped).refused, None);
    assert_eq!(db.recorded_probe_mismatch().unwrap(), None);

    // A model that does not answer compares nothing, so the record stands.
    assert!(ingest_file(&FakeEmbedder::new()).refused.is_some());
    let (result, _) = ingest_logged(&db, &FailingEmbedder::new(768), dir);
    assert_eq!(result.refused, None);
    assert!(db.recorded_probe_mismatch().unwrap().is_some());
}

#[test]
fn ingest_refuses_a_model_of_another_width_behind_the_recorded_name() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FakeEmbedder::new(), dir);
    let wider = FakeEmbedder::with_dimensions(1024);
    let width_refusal = "Refusing to ingest: the index holds 768-dimension vectors from \
                         nomic-embed-text but nomic-embed-text returns 1024 dimensions";

    let (result, _) = ingest_logged(&db, &wider, dir);

    assert!(
        result
            .refused
            .as_deref()
            .is_some_and(|r| r.starts_with(width_refusal)),
        "{:?}",
        result.refused
    );
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));

    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    let result = ingest::ingest_single_file(
        &db,
        &wider,
        dir,
        &dir.join("logging.md"),
        "heading",
        false,
        &|_| {},
    );
    assert!(
        result
            .refused
            .as_deref()
            .is_some_and(|r| r.starts_with(width_refusal)),
        "{:?}",
        result.refused
    );
    assert!(db.search_fts("stderr", 10).unwrap().is_empty());

    ingest::full_ingest(&db, &wider, dir, "heading", &|_| {});
    let (result, _) = ingest_logged(&db, &wider, dir);
    assert_eq!(result.refused, None);
    assert_eq!(db.vector_dimensions().unwrap(), Some(1024));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
}

const WIDTH_SETTING_REFUSAL: &str = "Refusing to ingest: nomic-embed-text returns \
                                     768-dimension vectors but [ollama] dimensions is 1024. \
                                     Set [ollama] dimensions to 768 or \"auto\", or configure \
                                     a model that returns 1024-dimension vectors.";

#[test]
fn a_first_ingest_stops_before_writing_when_the_width_is_not_the_configured_one() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();

    let (result, _) = ingest_logged(&db, &FakeEmbedder::new().configured_for(1024), dir);

    assert_eq!(result.refused.as_deref(), Some(WIDTH_SETTING_REFUSAL));
    assert_eq!(db.stats().unwrap().chunks, 0);
    assert_eq!(db.vector_dimensions().unwrap(), None);
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), None);

    let (result, _) = ingest_logged(&db, &FakeEmbedder::new().configured_for(768), dir);
    assert_eq!(result.refused, None);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
}

#[test]
fn a_forced_rebuild_stops_before_writing_when_the_width_is_not_the_configured_one() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    let head = commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FakeEmbedder::new(), dir);
    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();

    let result = ingest::full_ingest(
        &db,
        &FakeEmbedder::new().configured_for(1024),
        dir,
        "heading",
        &|_| {},
    );

    assert_eq!(result.refused.as_deref(), Some(WIDTH_SETTING_REFUSAL));
    assert!(db.search_fts("stderr", 10).unwrap().is_empty());
    assert_eq!(db.search_fts("abbreviations", 10).unwrap().len(), 1);
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));
}

#[test]
fn an_unrecorded_index_is_not_rebuilt_when_the_width_is_not_the_configured_one() {
    let tmp = tempdir().unwrap();
    let (dir, db) = v0_5_index(tmp.path(), 768);

    let (result, _) = ingest_logged(&db, &FakeEmbedder::new().configured_for(1024), &dir);

    assert_eq!(result.refused.as_deref(), Some(WIDTH_SETTING_REFUSAL));
    assert_eq!(db.recorded_embedding_model().unwrap(), None);
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    assert!(db.chunks_without_vectors().unwrap().is_empty());

    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new().configured_for(768), &dir);
    assert_eq!(result.refused, None);
    assert_eq!(rebuilds(&lines), 1, "{lines:?}");
    assert!(db.recorded_embedding_model().unwrap().is_some());
}

#[test]
fn a_text_only_index_gains_no_table_or_text_when_the_width_is_not_the_configured_one() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FailingEmbedder::new(768), dir);
    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    let head = commit_all(dir);

    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new().configured_for(1024), dir);

    assert_eq!(result.refused.as_deref(), Some(WIDTH_SETTING_REFUSAL));
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(db.vector_dimensions().unwrap(), None);
    assert!(db.search_fts("stderr", 10).unwrap().is_empty());
    assert_ne!(
        db.get_metadata("last_ingested_commit").unwrap(),
        Some(head.clone())
    );

    let (result, _) = ingest_logged(&db, &FakeEmbedder::new().configured_for(768), dir);
    assert_eq!(result.refused, None);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    assert!(db.chunks_without_vectors().unwrap().is_empty());
    assert_eq!(db.get_metadata("last_ingested_commit").unwrap(), Some(head));
}

#[test]
fn every_ingest_names_the_setting_when_a_pinned_width_meets_a_wider_model() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    seed_patterns(dir);
    git_init(dir);
    commit_all(dir);
    let db = memory_db();
    ingest_logged(&db, &FakeEmbedder::new().configured_for(768), dir);
    fs::write(dir.join("logging.md"), "# Logging\n\nLog to stderr.\n").unwrap();
    commit_all(dir);
    let wider = FakeEmbedder::with_dimensions(1024).configured_for(768);
    let setting_refusal = "Refusing to ingest: nomic-embed-text returns 1024-dimension vectors \
                           but [ollama] dimensions is 768. Set [ollama] dimensions to 1024 or \
                           \"auto\", or configure a model that returns 768-dimension vectors.";

    let (result, _) = ingest_logged(&db, &wider, dir);
    assert_eq!(result.refused.as_deref(), Some(setting_refusal));

    let result = ingest::ingest_single_file(
        &db,
        &wider,
        dir,
        &dir.join("logging.md"),
        "heading",
        false,
        &|_| {},
    );
    assert_eq!(result.refused.as_deref(), Some(setting_refusal));

    let result = ingest::full_ingest(&db, &wider, dir, "heading", &|_| {});
    assert_eq!(result.refused.as_deref(), Some(setting_refusal));

    assert!(db.search_fts("stderr", 10).unwrap().is_empty());
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
}

/// Answer `/api/embed` the way Ollama does, with [`FakeEmbedder`] vectors,
/// and every other request, such as `lore status`'s health and model checks,
/// with an empty success. Returns the base URL and a count of the embed
/// requests served.
fn serve_fake_ollama() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    std::thread::spawn(move || {
        let embedder = FakeEmbedder::new();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    content_length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let response = match request["input"][0].as_str() {
                Some(input) => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    serde_json::json!({ "embeddings": [embedder.embed(input).unwrap()] })
                }
                None => serde_json::json!({}),
            }
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
        }
    });
    (url, served)
}

fn lore() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("lore").unwrap()
}

#[test]
fn plain_ingest_adds_vectors_to_a_text_only_index_once_the_model_answers() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let mut config = Config::default_with(
        knowledge_dir,
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = "http://127.0.0.1:1".to_string();
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    let text_only_note = "Note: the index has no vectors, so searches use text only. \
                          Run 'lore ingest' once the embedding model answers to add them.";
    let cause = "Embedding model did not answer (";

    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("running full ingest"))
        .stderr(predicate::str::contains(cause).count(1))
        .stderr(predicate::str::contains(text_only_note));
    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Already up to date"))
        .stderr(predicate::str::contains(cause).count(1))
        .stderr(predicate::str::contains(text_only_note));

    let (url, served) = serve_fake_ollama();
    config.ollama.host = url;
    config.save(&config_path).unwrap();

    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Already up to date"))
        .stderr(predicate::str::contains(
            "Embedded 3 chunk(s) that had no vector",
        ))
        .stderr(predicate::str::contains("Note:").not())
        .stderr(predicate::str::contains("Errors:").not());
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        assert_eq!(db.vector_dimensions().unwrap(), Some(768));
        assert!(db.chunks_without_vectors().unwrap().is_empty());
        assert_eq!(
            db.get_metadata("embedding_model").unwrap().as_deref(),
            Some("nomic-embed-text")
        );
    }

    let before_search = served.load(Ordering::SeqCst);
    lore()
        .env("LORE_DEBUG", "1")
        .args(["search", "--config", config_arg, "error propagation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Error Handling"))
        .stderr(predicate::str::contains(
            "search: embedding succeeded (768 dims)",
        ))
        .stderr(predicate::str::contains("text search only").not());
    assert_eq!(served.load(Ordering::SeqCst), before_search + 1);
}

#[test]
fn ingest_file_says_the_text_was_saved_when_the_model_cannot_embed() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = "http://127.0.0.1:1".to_string();
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        db.init().unwrap();
        let (result, _) = ingest_logged(&db, &FakeEmbedder::new(), &knowledge_dir);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }
    let draft = knowledge_dir.join("draft.md");
    fs::write(
        &draft,
        "# Draft\n\nPrefer borrowed slices in function arguments.\n",
    )
    .unwrap();
    let draft_arg = draft.to_str().unwrap();
    let saved = "single-file ingest saved the text of draft.md but not all of its vectors. \
                 Run 'lore ingest' once the embedding model answers to add them.";
    let ignored = "If .loreignore excludes the file, that ingest may remove it instead.";

    lore()
        .args(["ingest", "--config", config_arg, "--file", draft_arg])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(saved))
        .stderr(predicate::str::contains(ignored).not())
        .stderr(predicate::str::contains("single-file ingest failed").not());
    lore()
        .args([
            "ingest", "--config", config_arg, "--file", draft_arg, "--force",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(format!("{saved} {ignored}")));

    let db = KnowledgeDB::open(&config.database).unwrap();
    let missing: Vec<String> = db
        .chunks_without_vectors()
        .unwrap()
        .into_iter()
        .map(|chunk| chunk.source_file)
        .collect();
    assert_eq!(missing, ["draft.md"]);
    let (result, lines) = ingest_logged(&db, &FakeEmbedder::new(), &knowledge_dir);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(
        logged(&lines, "Embedded 1 chunk(s) that had no vector"),
        "{lines:?}"
    );
    assert!(db.chunks_without_vectors().unwrap().is_empty());
}

#[test]
fn ingest_file_names_ingest_force_when_a_probe_mismatch_is_recorded() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = "http://127.0.0.1:1".to_string();
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        db.init().unwrap();
        ingest::full_ingest(
            &db,
            &SwappedEmbedder(FakeEmbedder::new()),
            &knowledge_dir,
            "heading",
            &|_| {},
        );
        let (result, _) = ingest_logged(&db, &FakeEmbedder::new(), &knowledge_dir);
        assert!(result.refused.is_some());
    }
    let draft = knowledge_dir.join("draft.md");
    fs::write(
        &draft,
        "# Draft\n\nPrefer borrowed slices in function arguments.\n",
    )
    .unwrap();
    let draft_arg = draft.to_str().unwrap();

    lore()
        .args(["ingest", "--config", config_arg, "--file", draft_arg])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "single-file ingest saved the text of draft.md but not all of its vectors. \
             nomic-embed-text is not the model that built the index under that name: its \
             vectors match the recorded ones at cosine similarity -1.000, below 0.99, so \
             searches use text only. Run 'lore ingest --force' to rebuild the index.",
        ))
        .stderr(predicate::str::contains("once the embedding model answers").not());
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        assert!(db.recorded_probe_mismatch().unwrap().is_some());
        assert!(db.chunk_count_for_source("draft.md").unwrap() > 0);
    }

    let (url, _) = serve_fake_ollama();
    config.ollama.host = url;
    config.save(&config_path).unwrap();
    lore()
        .args(["ingest", "--force", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Errors:").not());
    let db = KnowledgeDB::open(&config.database).unwrap();
    assert_eq!(db.recorded_probe_mismatch().unwrap(), None);
    assert!(db.chunk_count_for_source("draft.md").unwrap() > 0);
    assert!(db.chunks_without_vectors().unwrap().is_empty());
}

#[test]
fn ingest_force_rebuilds_an_index_of_another_model_with_the_configured_one() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let (url, served) = serve_fake_ollama();
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = url;
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        db.init().unwrap();
        ingest::ingest(
            &db,
            &FakeEmbedder::with_model("other-model", 1024),
            &knowledge_dir,
            "heading",
            &|_| {},
        );
    }

    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "the index was built with other-model but lore is configured for \
             nomic-embed-text",
        ))
        .stderr(predicate::str::contains(
            "Run 'lore ingest --force' to rebuild the index",
        ));
    assert_eq!(served.load(Ordering::SeqCst), 0);

    lore()
        .args(["ingest", "--force", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Errors:").not())
        .stderr(predicate::str::contains("Note:").not());
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        assert_eq!(db.vector_dimensions().unwrap(), Some(768));
        assert_eq!(
            db.recorded_embedding_model().unwrap().unwrap().model,
            "nomic-embed-text"
        );
        assert!(db.chunks_without_vectors().unwrap().is_empty());
    }

    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Already up to date"));
    lore()
        .env("LORE_DEBUG", "1")
        .args(["search", "--config", config_arg, "error propagation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Error Handling"))
        .stderr(predicate::str::contains(
            "search: embedding succeeded (768 dims)",
        ))
        .stderr(predicate::str::contains("lore ingest --force").not());
}

#[test]
fn ingest_force_clears_a_probe_mismatch_and_searches_return_to_hybrid() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = "http://127.0.0.1:1".to_string();
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        db.init().unwrap();
        ingest::full_ingest(
            &db,
            &SwappedEmbedder(FakeEmbedder::new()),
            &knowledge_dir,
            "heading",
            &|_| {},
        );
        let (result, _) = ingest_logged(&db, &FakeEmbedder::new(), &knowledge_dir);
        assert!(result.refused.is_some());
    }
    let warning = "Warning: nomic-embed-text is not the model that built the index under that \
                   name: its vectors match the recorded ones at cosine similarity -1.000, below \
                   0.99, so searches use text only. Run 'lore ingest --force' to rebuild the \
                   index.";

    // The configured host refuses connections, so an embed attempt would
    // add its own warning; its absence proves the search never embedded.
    lore()
        .args(["search", "--config", config_arg, "error propagation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Error Handling"))
        .stderr(predicate::str::contains(warning))
        .stderr(predicate::str::contains("Ollama embed failed").not());
    lore()
        .args([
            "--json",
            "search",
            "--config",
            config_arg,
            "error propagation",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Error Handling"))
        .stderr(predicate::str::is_empty());
    lore()
        .args(["status", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Index model:  ✗ {}",
            warning.strip_prefix("Warning: ").unwrap()
        )));

    let (url, served) = serve_fake_ollama();
    config.ollama.host = url;
    config.save(&config_path).unwrap();
    lore()
        .args(["ingest", "--force", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Errors:").not());
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        assert_eq!(db.recorded_probe_mismatch().unwrap(), None);
        assert!(db.chunks_without_vectors().unwrap().is_empty());
    }

    let before_search = served.load(Ordering::SeqCst);
    lore()
        .env("LORE_DEBUG", "1")
        .args(["search", "--config", config_arg, "error propagation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Error Handling"))
        .stderr(predicate::str::contains(
            "search: embedding succeeded (768 dims)",
        ))
        .stderr(predicate::str::contains("lore ingest --force").not());
    assert_eq!(served.load(Ordering::SeqCst), before_search + 1);
}

#[test]
fn plain_ingest_clears_a_probe_mismatch_once_the_model_answers_as_recorded() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let (url, served) = serve_fake_ollama();
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = url;
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        db.init().unwrap();
        ingest::full_ingest(
            &db,
            &FakeEmbedder::new(),
            &knowledge_dir,
            "heading",
            &|_| {},
        );
        let (result, _) = ingest_logged(&db, &SwappedEmbedder(FakeEmbedder::new()), &knowledge_dir);
        assert!(result.refused.is_some());
        assert!(db.recorded_probe_mismatch().unwrap().is_some());
    }

    lore()
        .args(["status", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Index model:  ✗ nomic-embed-text is not the model that built the index",
        ));
    lore()
        .args(["status", "--full", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Runtime:      ✓ inference OK"))
        .stderr(predicate::str::contains(
            "Index model:  ✗ an earlier write found another model under the name \
             nomic-embed-text, but it now answers as the model that built the index; searches \
             use text only until that record clears. Run 'lore ingest' to clear it.",
        ));

    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Errors:").not());
    assert_eq!(
        KnowledgeDB::open(&config.database)
            .unwrap()
            .recorded_probe_mismatch()
            .unwrap(),
        None
    );

    let before_search = served.load(Ordering::SeqCst);
    lore()
        .env("LORE_DEBUG", "1")
        .args(["search", "--config", config_arg, "error propagation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Error Handling"))
        .stderr(predicate::str::contains(
            "search: embedding succeeded (768 dims)",
        ))
        .stderr(predicate::str::contains("lore ingest").not());
    assert_eq!(served.load(Ordering::SeqCst), before_search + 1);
}

#[test]
fn ingest_exits_1_when_the_model_width_is_not_the_configured_dimensions() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let (url, _) = serve_fake_ollama();
    let mut config = Config::default_with(
        knowledge_dir,
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = url;
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let saved = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        saved.replace(
            "model = \"nomic-embed-text\"\n",
            "model = \"nomic-embed-text\"\ndimensions = 1024\n",
        ),
    )
    .unwrap();
    let config_arg = config_path.to_str().unwrap();

    for force in [false, true] {
        let mut args = vec!["ingest", "--config", config_arg];
        if force {
            args.push("--force");
        }
        lore()
            .args(&args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(format!(
                "Error: {WIDTH_SETTING_REFUSAL}"
            )))
            .stderr(predicate::str::contains("lore ingest --force").not());
    }
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        assert_eq!(db.stats().unwrap().chunks, 0);
        assert_eq!(db.vector_dimensions().unwrap(), None);
    }

    let pinned = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        pinned.replace("dimensions = 1024\n", "dimensions = 768\n"),
    )
    .unwrap();
    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success();
    let db = KnowledgeDB::open(&config.database).unwrap();
    assert!(db.stats().unwrap().chunks > 0);
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
}

#[test]
fn ingest_refuses_an_unreadable_recorded_probe_until_a_forced_rebuild() {
    use predicates::prelude::*;

    let tmp = tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    fs::create_dir_all(&knowledge_dir).unwrap();
    seed_patterns(&knowledge_dir);
    git_init(&knowledge_dir);
    commit_all(&knowledge_dir);
    let (url, _) = serve_fake_ollama();
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.path().join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.host = url;
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();
    let config_arg = config_path.to_str().unwrap();
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        db.init().unwrap();
        ingest::full_ingest(
            &db,
            &FakeEmbedder::new(),
            &knowledge_dir,
            "heading",
            &|_| {},
        );
        db.set_metadata("embedding_probe_vector", "[0.5,").unwrap();
    }

    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: Refusing to ingest: cannot read the index's embedding model",
        ))
        .stderr(predicate::str::contains(
            "Run 'lore ingest --force' to rebuild the index",
        ));

    lore()
        .args(["ingest", "--force", "--config", config_arg])
        .assert()
        .success();
    lore()
        .args(["ingest", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Already up to date"));
}
