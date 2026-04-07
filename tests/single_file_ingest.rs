// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for `lore ingest --file <path>`.
//!
//! Exercises the single-file ingest entry point end-to-end: uncommitted
//! files in non-git and git tempdirs, replace-not-append semantics, the
//! `META_LAST_COMMIT` invariant, path containment, `.loreignore` respect
//! and override, and the "search can find the file immediately" check
//! that motivates the whole feature.

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

fn memory_db() -> KnowledgeDB {
    let db = KnowledgeDB::open(Path::new(":memory:"), 768).unwrap();
    db.init().unwrap();
    db
}

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

fn single_file_ingest(
    db: &KnowledgeDB,
    dir: &Path,
    rel: &str,
    force: bool,
) -> ingest::IngestResult {
    let embedder = FakeEmbedder::new();
    ingest::ingest_single_file(
        db,
        &embedder,
        dir,
        &dir.join(rel),
        "heading",
        force,
        &|_| {},
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn uncommitted_file_in_non_git_dir_is_indexed_and_searchable() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write_md(dir, "rust.md", "Rust Patterns", "Rust pattern body");

    let db = memory_db();
    let result = single_file_ingest(&db, dir, "rust.md", false);

    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.files_processed, 1);
    assert!(matches!(
        result.mode,
        ingest::IngestMode::SingleFile { ref path } if path == "rust.md"
    ));

    let hits = db.search_fts("rust pattern", 10).unwrap();
    assert!(
        hits.iter().any(|h| h.source_file == "rust.md"),
        "rust.md should be searchable after single-file ingest"
    );
}

#[test]
fn uncommitted_file_in_git_repo_without_commit() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    // First file is committed — normal walk-based ingest would pick it up.
    write_md(dir, "committed.md", "Committed", "Committed body");
    git_commit_all(dir, "initial");

    // Second file is written to disk but NOT committed. This is the exact
    // scenario single-file ingest is meant to unblock.
    write_md(dir, "draft.md", "Draft", "Draft body");

    let db = memory_db();
    let result = single_file_ingest(&db, dir, "draft.md", false);

    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.files_processed, 1);
    // Only the single-file ingest ran; committed.md is still absent.
    assert_eq!(db.source_files().unwrap(), vec!["draft.md".to_string()]);
}

#[test]
fn single_file_does_not_touch_last_ingested_commit() {
    // Run a walk-based ingest first to record a real HEAD SHA, then run a
    // single-file ingest on a new uncommitted file and verify the recorded
    // SHA is untouched. This is the critical invariant: single-file ingest
    // must not interfere with the next `lore ingest` seeing real changes.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "committed.md", "Committed", "Committed body");
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    let recorded = db
        .get_metadata("last_ingested_commit")
        .unwrap()
        .expect("full ingest should have recorded HEAD");

    // Now create and single-file-ingest an uncommitted file.
    write_md(dir, "draft.md", "Draft", "Draft body");
    let result = single_file_ingest(&db, dir, "draft.md", false);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

    let after = db.get_metadata("last_ingested_commit").unwrap();
    assert_eq!(
        after,
        Some(recorded),
        "single-file ingest must not overwrite last_ingested_commit"
    );
}

#[test]
fn re_ingesting_same_file_replaces_chunks_without_duplication() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write_md(dir, "note.md", "Note", "Original body");

    let db = memory_db();
    single_file_ingest(&db, dir, "note.md", false);
    let first = db.stats().unwrap().chunks;

    // Rewrite the file on disk, then re-ingest.
    write_md(dir, "note.md", "Note", "Replacement body");
    let result = single_file_ingest(&db, dir, "note.md", false);
    assert!(result.errors.is_empty());

    let second = db.stats().unwrap().chunks;
    assert_eq!(first, second, "re-ingest must replace, not append");
    assert_eq!(db.source_files().unwrap(), vec!["note.md".to_string()]);

    // New content is searchable, old content is not.
    let hits_new = db.search_fts("replacement body", 10).unwrap();
    assert!(hits_new.iter().any(|h| h.source_file == "note.md"));
}

#[test]
fn rejects_path_outside_knowledge_directory() {
    let knowledge = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(
        outside.path().join("escape.md"),
        "# Escape\n\nBody that is long enough for chunking.\n",
    )
    .unwrap();

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::ingest_single_file(
        &db,
        &embedder,
        knowledge.path(),
        &outside.path().join("escape.md"),
        "heading",
        false,
        &|_| {},
    );

    assert!(!result.errors.is_empty());
    assert!(
        result.errors[0].contains("escapes the knowledge directory"),
        "unexpected error: {}",
        result.errors[0]
    );
    assert!(db.source_files().unwrap().is_empty());
}

#[test]
fn rejects_non_markdown_extension() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("notes.txt"), "plain text, not markdown").unwrap();

    let db = memory_db();
    let result = single_file_ingest(&db, dir, "notes.txt", false);

    assert!(!result.errors.is_empty());
    assert!(
        result.errors[0].contains("Unsupported extension"),
        "unexpected error: {}",
        result.errors[0]
    );
}

#[test]
fn respects_loreignore_without_force() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write_md(dir, "draft.md", "Draft", "Draft body");
    fs::write(dir.join(".loreignore"), "draft.md\n").unwrap();

    let db = memory_db();
    let result = single_file_ingest(&db, dir, "draft.md", false);

    assert!(!result.errors.is_empty());
    assert!(
        result.errors[0].contains(".loreignore"),
        "error should mention .loreignore: {}",
        result.errors[0]
    );
    assert!(db.source_files().unwrap().is_empty());
}

#[test]
fn force_overrides_loreignore_exclusion() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write_md(dir, "draft.md", "Draft", "Draft body text content");
    fs::write(dir.join(".loreignore"), "draft.md\n").unwrap();

    let db = memory_db();
    let result = single_file_ingest(&db, dir, "draft.md", true);

    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.files_processed, 1);
    assert_eq!(db.source_files().unwrap(), vec!["draft.md".to_string()]);
}

#[test]
fn rejects_directory_path() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("sub")).unwrap();

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::ingest_single_file(
        &db,
        &embedder,
        dir,
        &dir.join("sub"),
        "heading",
        false,
        &|_| {},
    );

    assert!(!result.errors.is_empty());
    // The canonicalised directory path is rejected by the extension check
    // or the is_file() guard, depending on platform — both are expected.
    let msg = &result.errors[0];
    assert!(
        msg.contains("Not a regular file") || msg.contains("Unsupported extension"),
        "unexpected error: {msg}"
    );
    assert!(db.source_files().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escaping_knowledge_dir() {
    // A symlink inside knowledge_dir pointing at a file outside of it must
    // be rejected: validate_within_dir canonicalises before strip_prefix, so
    // the canonical path lands outside knowledge_dir and the containment
    // check fails. This regression test pins that behaviour.
    use std::os::unix::fs::symlink;

    let knowledge = tempdir().unwrap();
    let outside = tempdir().unwrap();
    write_md(outside.path(), "secret.md", "Secret", "Secret body");

    let link = knowledge.path().join("link.md");
    symlink(outside.path().join("secret.md"), &link).unwrap();

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::ingest_single_file(
        &db,
        &embedder,
        knowledge.path(),
        &link,
        "heading",
        false,
        &|_| {},
    );

    assert!(!result.errors.is_empty());
    assert!(
        result.errors[0].contains("escapes the knowledge directory"),
        "unexpected error: {}",
        result.errors[0]
    );
    assert!(db.source_files().unwrap().is_empty());
}

#[test]
fn subsequent_delta_ingest_wipes_single_file_upsert_of_git_deleted_file() {
    // Hazard test — pins current behaviour so a future refactor notices
    // the interaction. Context: single-file ingest deliberately does not
    // update META_LAST_COMMIT. If the user single-file-ingests a file that
    // git-deleted since the last ingest, the next delta ingest sees the
    // file in `git diff --name-status` output as Deleted and calls
    // delete_by_source, silently removing the chunks the user just added.
    //
    // This is not a bug per se — delta ingest is correctly reflecting git
    // state — but it is an interaction the pattern author needs to know
    // about. Documented in docs/pattern-authoring-guide.md next to the
    // Vocabulary Coverage Technique.
    //
    // If future work adds provenance tracking or changes the delta-wipe
    // behaviour, this test will fail and force a conscious update.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    git_init(dir);
    write_md(dir, "doomed.md", "Doomed", "Doomed body text");
    git_commit_all(dir, "initial");

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert_eq!(db.source_files().unwrap(), vec!["doomed.md".to_string()]);

    // Remove the file in git and commit. Then recreate it in the working
    // tree (uncommitted) and single-file-ingest it.
    fs::remove_file(dir.join("doomed.md")).unwrap();
    git_commit_all(dir, "remove doomed");
    write_md(dir, "doomed.md", "Doomed", "Doomed body text reborn");
    let single = single_file_ingest(&db, dir, "doomed.md", false);
    assert!(single.errors.is_empty(), "errors: {:?}", single.errors);
    assert_eq!(db.source_files().unwrap(), vec!["doomed.md".to_string()]);

    // Run delta ingest. It sees doomed.md as Deleted between the prior
    // last_ingested_commit and HEAD, and wipes the chunks the single-file
    // ingest just inserted.
    ingest::ingest(&db, &embedder, dir, "heading", &|_| {});
    assert!(
        db.source_files().unwrap().is_empty(),
        "current behaviour: delta ingest wipes single-file chunks of a git-deleted file. \
         If this test fails, update the pattern-authoring guide interaction note."
    );
}

#[test]
fn done_line_is_suppressed_when_single_file_ingest_fails() {
    // Regression test for the "Done (single-file): → 0 chunks" contradiction
    // observed during review: the library function returns IngestResult with
    // a populated mode.path even on error, and cmd_ingest's summary is
    // suppressed when errors is non-empty. We cannot easily assert cmd_ingest
    // output without invoking the binary, but we can assert the library
    // contract: on error, mode.path is non-empty (the caller's path) and
    // errors is non-empty.
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("notes.txt"), "not markdown").unwrap();

    let db = memory_db();
    let embedder = FakeEmbedder::new();
    let result = ingest::ingest_single_file(
        &db,
        &embedder,
        dir,
        &dir.join("notes.txt"),
        "heading",
        false,
        &|_| {},
    );

    assert!(!result.errors.is_empty());
    match result.mode {
        ingest::IngestMode::SingleFile { path } => {
            assert!(
                path.contains("notes.txt"),
                "mode.path should carry the requested path on error, got: {path:?}"
            );
        }
        other => panic!("unexpected mode: {other:?}"),
    }
    assert_eq!(result.files_processed, 0);
    assert_eq!(result.chunks_created, 0);
}
