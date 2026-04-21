use std::fs;
use std::path::Path;
use std::process::Command;

use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, FakeEmbedder};
use lore::ingest::{self, CommitStatus};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

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

/// Set up a working repo with a bare remote. Returns the bare repo `TempDir`.
fn setup_repo_with_remote(dir: &Path) -> tempfile::TempDir {
    git_init(dir);

    // Create bare remote.
    let bare = tempdir().unwrap();
    Command::new("git")
        .args(["init", "--bare"])
        .current_dir(bare.path())
        .output()
        .expect("git init --bare failed");

    Command::new("git")
        .args(["remote", "add", "origin", &bare.path().to_string_lossy()])
        .current_dir(dir)
        .output()
        .expect("git remote add failed");

    // Initial commit so HEAD exists.
    fs::write(dir.join("README.md"), "# Patterns\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir)
        .output()
        .expect("git add failed");
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir)
        .output()
        .expect("git commit failed");
    Command::new("git")
        .args(["push", "-u", "origin", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git push failed");

    bare
}

fn open_db(dir: &Path, dims: usize) -> KnowledgeDB {
    let db_path = dir.join("knowledge.db");
    let db = KnowledgeDB::open(&db_path, dims).expect("failed to open DB");
    db.init().expect("failed to init DB");
    db
}

fn git_show_bare(bare_dir: &Path, refspec: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["--git-dir", &bare_dir.to_string_lossy(), "show", refspec])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn head_sha(dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn add_pattern_pushes_to_inbox_branch() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let bare = setup_repo_with_remote(dir);
    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let head_before = head_sha(dir);

    let result = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Error Handling",
        "Use anyhow for application errors.\n",
        &["rust"],
        Some("inbox/"),
    )
    .unwrap();

    // Should be pushed, not committed locally.
    assert!(
        matches!(&result.commit_status, CommitStatus::Pushed { branch } if branch == "inbox/error-handling"),
        "expected Pushed, got {:?}",
        result.commit_status
    );
    assert_eq!(result.chunks_indexed, 0, "should not index in inbox mode");
    assert_eq!(result.file_path, "error-handling.md");

    // HEAD unchanged.
    assert_eq!(head_sha(dir), head_before);

    // File NOT on working tree.
    assert!(!dir.join("error-handling.md").exists());

    // File IS on the remote inbox branch.
    let remote_content =
        git_show_bare(bare.path(), "inbox/error-handling:error-handling.md").unwrap();
    assert!(remote_content.contains("Error Handling"));
    assert!(remote_content.contains("anyhow"));

    // DB has no chunks for this file.
    let results = db.search_fts("anyhow", 10).unwrap();
    assert!(results.is_empty(), "inbox content should not be indexed");
}

#[test]
fn update_pattern_pushes_modified_file() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let bare = setup_repo_with_remote(dir);
    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    // Create a trunk file to update.
    fs::write(
        dir.join("testing.md"),
        "# Testing\n\nOld testing content.\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "testing.md"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add testing"])
        .current_dir(dir)
        .output()
        .unwrap();

    let result = ingest::update_pattern(
        &db,
        &embedder,
        dir,
        "testing.md",
        "New testing content with property-based tests.\n",
        Some(&["testing"]),
        Some("inbox/"),
    )
    .unwrap();

    assert!(matches!(&result.commit_status, CommitStatus::Pushed { .. }));

    // Trunk file unchanged.
    let trunk_content = fs::read_to_string(dir.join("testing.md")).unwrap();
    assert!(
        trunk_content.contains("Old testing content"),
        "trunk file should not be modified"
    );

    // Remote branch has updated content.
    let remote_content = git_show_bare(bare.path(), "inbox/testing:testing.md").unwrap();
    assert!(remote_content.contains("property-based tests"));
}

#[test]
fn append_to_pattern_pushes_appended_file() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let bare = setup_repo_with_remote(dir);
    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    // Create a trunk file to append to.
    fs::write(
        dir.join("conventions.md"),
        "# Conventions\n\nExisting content.\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "conventions.md"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add conventions"])
        .current_dir(dir)
        .output()
        .unwrap();

    let result = ingest::append_to_pattern(
        &db,
        &embedder,
        dir,
        "conventions.md",
        "Naming",
        "Use snake_case for all Rust identifiers.\n",
        Some("inbox/"),
    )
    .unwrap();

    assert!(matches!(&result.commit_status, CommitStatus::Pushed { .. }));

    // Trunk file unchanged.
    let trunk_content = fs::read_to_string(dir.join("conventions.md")).unwrap();
    assert!(
        !trunk_content.contains("Naming"),
        "trunk file should not be modified"
    );

    // Remote branch has appended content.
    let remote_content = git_show_bare(bare.path(), "inbox/conventions:conventions.md").unwrap();
    assert!(remote_content.contains("Existing content"));
    assert!(remote_content.contains("## Naming"));
    assert!(remote_content.contains("snake_case"));
}

#[test]
fn two_adds_create_independent_branches() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let bare = setup_repo_with_remote(dir);
    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let r1 = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Pattern Alpha",
        "Alpha content.\n",
        &[],
        Some("inbox/"),
    )
    .unwrap();

    let r2 = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Pattern Beta",
        "Beta content.\n",
        &[],
        Some("inbox/"),
    )
    .unwrap();

    let b1 = match &r1.commit_status {
        CommitStatus::Pushed { branch } => branch.clone(),
        other => panic!("expected Pushed, got {other:?}"),
    };
    let b2 = match &r2.commit_status {
        CommitStatus::Pushed { branch } => branch.clone(),
        other => panic!("expected Pushed, got {other:?}"),
    };

    assert_ne!(b1, b2, "branches should be different");

    // Both files on respective remote branches.
    assert!(git_show_bare(bare.path(), &format!("{b1}:pattern-alpha.md")).is_some());
    assert!(git_show_bare(bare.path(), &format!("{b2}:pattern-beta.md")).is_some());
}

#[test]
fn same_title_disambiguates_branch_name() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let _bare = setup_repo_with_remote(dir);
    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let r1 = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "My Pattern",
        "First version.\n",
        &[],
        Some("inbox/"),
    )
    .unwrap();

    let r2 = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "My Pattern",
        "Second version.\n",
        &[],
        Some("inbox/"),
    )
    .unwrap();

    let b1 = match &r1.commit_status {
        CommitStatus::Pushed { branch } => branch.as_str(),
        _ => panic!("expected Pushed"),
    };
    let b2 = match &r2.commit_status {
        CommitStatus::Pushed { branch } => branch.as_str(),
        _ => panic!("expected Pushed"),
    };

    assert_eq!(b1, "inbox/my-pattern");
    assert_eq!(b2, "inbox/my-pattern-2");
}

#[test]
fn no_git_config_preserves_default_behavior() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let result = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Local Pattern",
        "Body content that is long enough for a chunk.\n",
        &["local"],
        None,
    )
    .unwrap();

    assert!(
        matches!(result.commit_status, CommitStatus::NotCommitted),
        "without git repo, should be NotCommitted"
    );
    assert!(result.chunks_indexed >= 1, "should be indexed locally");
    assert!(dir.join("local-pattern.md").exists(), "file on disk");
}

#[test]
fn push_failure_is_hard_error() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    // Init git but no remote — push will fail.
    git_init(dir);
    fs::write(dir.join("seed.md"), "seed\n").unwrap();
    Command::new("git")
        .args(["add", "seed.md"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir)
        .output()
        .unwrap();

    let embedder = FakeEmbedder::new();
    let db = open_db(dir, embedder.dimensions());

    let result = ingest::add_pattern(
        &db,
        &embedder,
        dir,
        "Will Fail",
        "This should fail on push.\n",
        &[],
        Some("inbox/"),
    );

    assert!(result.is_err(), "push failure should propagate as error");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("push") || msg.contains("remote"),
        "error should mention push/remote, got: {msg}"
    );
}
