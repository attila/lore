// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the `.loreignore` feature.
//!
//! Exercises the full lifecycle: full ingest with filtering, delta ingest
//! with reconciliation, search reflecting filtered state, and edge cases
//! around `.loreignore` add/modify/delete and negation patterns.

use std::fs;
use std::path::Path;
use std::process::Command;

use lore::database::KnowledgeDB;
use lore::embeddings::FakeEmbedder;
use lore::ingest;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Initialise an in-memory `KnowledgeDB` matching the production embedding
/// dimensions used in unit tests (768).
fn memory_db() -> KnowledgeDB {
    let db = KnowledgeDB::open(Path::new(":memory:"), 768).unwrap();
    db.init().unwrap();
    db
}

/// Initialise a git repo with a test identity and disabled GPG signing.
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

/// Stage all changes and commit with the given message.
fn git_commit_all(dir: &Path, message: &str) {
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .output()
        .unwrap();
}

/// Write a markdown file with title and body, creating parent directories.
fn write_md(dir: &Path, name: &str, title: &str, body: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(
        &path,
        format!("# {title}\n\n{body} that is long enough for chunking.\n"),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn full_ingest_with_loreignore_excludes_readme_from_search() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "README.md", "Project Readme", "Project introduction");
    write_md(dir, "rust.md", "Rust Patterns", "Rust pattern body");
    fs::write(dir.join(".loreignore"), "README.md\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});

    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

    // Search for readme content — should return nothing.
    let hits = db.search_fts("project introduction", 10).unwrap();
    assert!(
        hits.iter().all(|h| h.source_file != "README.md"),
        "README chunks should not appear in search results"
    );
    // The other file should still be searchable.
    let hits = db.search_fts("rust pattern", 10).unwrap();
    assert!(
        hits.iter().any(|h| h.source_file == "rust.md"),
        "rust.md should be searchable"
    );
}

#[test]
fn add_loreignore_after_initial_ingest_removes_readme_chunks() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "README.md", "Readme", "Readme body");
    write_md(dir, "rust.md", "Rust", "Rust body");
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(db.stats().unwrap().sources, 2);

    // Add .loreignore in a new commit and run delta ingest.
    fs::write(dir.join(".loreignore"), "README.md\n").unwrap();
    git_commit_all(dir, "add loreignore");

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(result.errors.is_empty());
    let files = db.source_files().unwrap();
    assert_eq!(
        files,
        vec!["rust.md".to_string()],
        "README should be reconciled out"
    );
}

#[test]
fn modifying_loreignore_to_exclude_more_removes_additional_files() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "rust.md", "Rust", "Rust body");
    write_md(dir, "go.md", "Go", "Go body");
    write_md(dir, "scratch.md", "Scratch", "Scratch body");
    fs::write(dir.join(".loreignore"), "scratch.md\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(db.stats().unwrap().sources, 2);

    // Expand .loreignore to also exclude go.md.
    fs::write(dir.join(".loreignore"), "scratch.md\ngo.md\n").unwrap();
    git_commit_all(dir, "exclude go");

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(result.errors.is_empty());
    assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
}

#[test]
fn deleting_loreignore_keeps_existing_indexed_files() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "rust.md", "Rust", "Rust body");
    fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});

    fs::remove_file(dir.join(".loreignore")).unwrap();
    git_commit_all(dir, "remove loreignore");

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(result.errors.is_empty());
    // rust.md remains indexed. There's no drafts/ directory in this test,
    // so the cumulative reconciliation pass has nothing extra to add.
    // (See `deleting_loreignore_re_indexes_previously_excluded_files` for
    // the case where files actually come back.)
    assert_eq!(db.source_files().unwrap(), vec!["rust.md".to_string()]);
}

#[test]
fn loreignore_excluding_all_markdown_yields_empty_index() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "a.md", "A", "Body a");
    write_md(dir, "b.md", "B", "Body b");
    fs::write(dir.join(".loreignore"), "*.md\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let messages = std::cell::RefCell::new(Vec::<String>::new());
    let result = ingest::full_ingest(&db, &embedder, dir, "heading", &|m| {
        messages.borrow_mut().push(m.to_string());
    });

    assert!(result.errors.is_empty());
    assert_eq!(db.stats().unwrap().sources, 0);
    let captured = messages.borrow();
    assert!(
        captured
            .iter()
            .any(|m| m.contains("matched every markdown")),
        "expected warning about empty result, got: {captured:?}"
    );
}

#[test]
fn negation_pattern_un_ignores_specific_file_in_search() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "important.md", "Important", "Important body");
    write_md(dir, "draft.md", "Draft", "Draft body");
    write_md(dir, "scratch.md", "Scratch", "Scratch body");
    fs::write(dir.join(".loreignore"), "*.md\n!important.md\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(result.errors.is_empty());

    let files = db.source_files().unwrap();
    assert_eq!(files, vec!["important.md".to_string()]);
}

#[test]
fn compound_loreignore_edit_removes_excluded_and_re_indexes_un_ignored() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "alpha.md", "Alpha", "Alpha body");
    write_md(dir, "beta.md", "Beta", "Beta body");
    write_md(dir, "gamma.md", "Gamma", "Gamma body");
    // Initial: gamma.md is excluded.
    fs::write(dir.join(".loreignore"), "gamma.md\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(db.stats().unwrap().sources, 2);

    // Compound edit: un-ignore gamma.md, exclude beta.md.
    fs::write(dir.join(".loreignore"), "beta.md\n").unwrap();
    git_commit_all(dir, "swap exclusions");

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(result.errors.is_empty());
    // Reconciliation is cumulative: beta.md removed, gamma.md re-indexed.
    let files = db.source_files().unwrap();
    assert_eq!(
        files,
        vec!["alpha.md".to_string(), "gamma.md".to_string()],
        "expected alpha.md and gamma.md, got: {files:?}"
    );
}

#[test]
fn deleting_loreignore_re_indexes_previously_excluded_files() {
    // The opposite of `deleting_loreignore_keeps_existing_indexed_files`:
    // when .loreignore is removed entirely, files that had been excluded
    // are re-indexed automatically.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "rust.md", "Rust", "Rust body");
    write_md(dir, "drafts/wip.md", "WIP", "Draft body");
    fs::write(dir.join(".loreignore"), "drafts/\n").unwrap();
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(
        db.source_files().unwrap(),
        vec!["rust.md".to_string()],
        "drafts/ excluded by initial .loreignore"
    );

    // Delete .loreignore entirely.
    fs::remove_file(dir.join(".loreignore")).unwrap();
    git_commit_all(dir, "remove loreignore");

    let result = ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let files = db.source_files().unwrap();
    assert_eq!(
        files,
        vec!["drafts/wip.md".to_string(), "rust.md".to_string()],
        "drafts/wip.md should be re-indexed after .loreignore removal"
    );
}

#[test]
fn repository_without_loreignore_behaves_identically() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "rust.md", "Rust", "Rust body");
    write_md(dir, "go.md", "Go", "Go body");
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::full_ingest(&db, &embedder, dir, "heading", &|_| {});

    assert!(result.errors.is_empty());
    assert_eq!(result.files_processed, 2);
    assert_eq!(db.stats().unwrap().sources, 2);
}
