// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ingestion orchestration and write operations.
//!
//! Walks markdown directories, chunks files, embeds them via any [`Embedder`],
//! and stores results in the [`KnowledgeDB`].  Write helpers (`add_pattern`,
//! `update_pattern`, `append_to_pattern`) create or modify markdown files
//! on disk, re-index them, and optionally commit via git.

use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use walkdir::WalkDir;

use crate::chunking::{Chunk, chunk_as_document, chunk_by_heading, extract_title};
use crate::database::KnowledgeDB;
use crate::embeddings::Embedder;
use crate::git;

// ---------------------------------------------------------------------------
// Embedding helpers
// ---------------------------------------------------------------------------

/// Build the composite text used for embedding a chunk.
///
/// Includes title and tags alongside the body so that vector search
/// carries domain signal, not just body content.
fn embed_text(chunk: &Chunk) -> String {
    format!("{}\n{}\n{}", chunk.title, chunk.tags, chunk.body)
}

// ---------------------------------------------------------------------------
// Metadata keys
// ---------------------------------------------------------------------------

/// Key used to store the last successfully ingested commit SHA.
pub(crate) const META_LAST_COMMIT: &str = "last_ingested_commit";

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Whether the ingest ran in full or delta mode.
#[derive(Debug, PartialEq, Eq)]
pub enum IngestMode {
    /// Full re-index (cleared database and re-embedded everything).
    Full,
    /// Delta update — only changed files were processed.
    Delta {
        /// Number of files that were unchanged and skipped.
        unchanged: usize,
    },
}

/// Summary returned after a directory ingest.
#[derive(Debug)]
pub struct IngestResult {
    pub mode: IngestMode,
    pub files_processed: usize,
    pub chunks_created: usize,
    pub errors: Vec<String>,
}

/// Outcome of the git step in a write operation.
#[derive(Debug)]
pub enum CommitStatus {
    /// No git repo or commit was not attempted.
    NotCommitted,
    /// Committed locally on the checked-out branch.
    Committed,
    /// Committed and pushed to a per-submission inbox branch.
    Pushed { branch: String },
}

/// Summary returned after a single-file write operation.
#[derive(Debug)]
pub struct WriteResult {
    pub file_path: String,
    pub chunks_indexed: usize,
    pub commit_status: CommitStatus,
    pub embedding_failures: usize,
}

// ---------------------------------------------------------------------------
// Directory ingest (delta and full)
// ---------------------------------------------------------------------------

/// Ingest the knowledge base, using delta mode when possible.
///
/// Tries to detect changes via `git diff --name-status` against the last
/// successfully ingested commit. Falls back to a full re-index when:
/// - `knowledge_dir` is not a git repository
/// - No previous commit SHA is stored in the database
/// - The stored commit no longer exists in the repository history
///
/// Use [`full_ingest`] directly to force a complete re-index.
pub fn ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    // Not a git repo — full ingest is the only option.
    if !git::is_git_repo(knowledge_dir) {
        on_progress("Not a git repository — running full ingest");
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    }

    // No stored commit — first ingest or metadata was cleared.
    let Ok(Some(last_commit)) = db.get_metadata(META_LAST_COMMIT) else {
        on_progress("No previous ingest recorded — running full ingest");
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    };

    // Stored commit no longer exists (history rewrite, shallow clone, etc.).
    if !git::commit_exists(knowledge_dir, &last_commit) {
        on_progress("Previous commit not found in history — running full ingest");
        return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
    }

    // Capture HEAD once to avoid TOCTOU between diff and recording.
    let head = match git::head_commit(knowledge_dir) {
        Ok(h) => h,
        Err(e) => {
            on_progress(&format!(
                "Failed to resolve HEAD ({e}) — running full ingest"
            ));
            return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
        }
    };

    // Run delta detection.
    let changes = match git::diff_name_status(knowledge_dir, &last_commit) {
        Ok(c) => c,
        Err(e) => {
            on_progress(&format!("git diff failed ({e}) — running full ingest"));
            return full_ingest(db, embedder, knowledge_dir, strategy, on_progress);
        }
    };

    if changes.is_empty() {
        on_progress("Already up to date — no files changed since last ingest");
        return IngestResult {
            mode: IngestMode::Delta { unchanged: 0 },
            files_processed: 0,
            chunks_created: 0,
            errors: Vec::new(),
        };
    }

    delta_ingest(
        db,
        embedder,
        knowledge_dir,
        strategy,
        &changes,
        &head,
        on_progress,
    )
}

/// Process only the files that changed since the last ingest.
fn delta_ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    changes: &[git::FileChange],
    head: &str,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    // Count unchanged files: existing sources minus those being modified/deleted/renamed.
    let sources_before = db.stats().map(|s| s.sources).unwrap_or(0);
    let existing_changed = changes
        .iter()
        .filter(|c| !matches!(c, git::FileChange::Added(_)))
        .count();
    let unchanged = sources_before.saturating_sub(existing_changed);

    on_progress(&format!("Delta ingest: {} file(s) changed", changes.len()));

    let mut result = IngestResult {
        mode: IngestMode::Delta { unchanged },
        files_processed: 0,
        chunks_created: 0,
        errors: Vec::new(),
    };

    for change in changes {
        match change {
            git::FileChange::Added(path) | git::FileChange::Modified(path) => {
                let file_path = knowledge_dir.join(path);
                match index_single_file(db, embedder, knowledge_dir, &file_path, strategy) {
                    Ok((chunks, _)) => {
                        result.chunks_created += chunks;
                        on_progress(&format!("  {path} → {chunks} chunks"));
                    }
                    Err(e) => {
                        result.errors.push(format!("Failed to index {path}: {e}"));
                    }
                }
            }
            git::FileChange::Deleted(path) => {
                if let Err(e) = db.delete_by_source(path) {
                    result.errors.push(format!("Failed to delete {path}: {e}"));
                } else {
                    on_progress(&format!("  {path} (deleted)"));
                }
            }
            git::FileChange::Renamed { from, to } => {
                if let Err(e) = db.delete_by_source(from) {
                    result
                        .errors
                        .push(format!("Failed to delete old path {from}: {e}"));
                }
                let file_path = knowledge_dir.join(to);
                match index_single_file(db, embedder, knowledge_dir, &file_path, strategy) {
                    Ok((chunks, _)) => {
                        result.chunks_created += chunks;
                        on_progress(&format!("  {from} → {to} ({chunks} chunks)"));
                    }
                    Err(e) => {
                        result.errors.push(format!("Failed to index {to}: {e}"));
                    }
                }
            }
        }
        result.files_processed += 1;
    }

    // Record the pre-captured HEAD on success (no errors).
    if result.errors.is_empty()
        && let Err(e) = db.set_metadata(META_LAST_COMMIT, head)
    {
        on_progress(&format!("Warning: failed to record commit SHA: {e}"));
    }

    result
}

/// Clear the database and re-index every markdown file from scratch.
///
/// Records the current HEAD commit SHA on success so that subsequent
/// [`ingest`] calls can use delta mode.
pub fn full_ingest(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    strategy: &str,
    on_progress: &dyn Fn(&str),
) -> IngestResult {
    let mut result = IngestResult {
        mode: IngestMode::Full,
        files_processed: 0,
        chunks_created: 0,
        errors: Vec::new(),
    };

    let mut md_files: Vec<_> = WalkDir::new(knowledge_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file()
                && matches!(
                    e.path().extension().and_then(|s| s.to_str()),
                    Some("md" | "markdown")
                )
        })
        .map(walkdir::DirEntry::into_path)
        .collect();

    md_files.sort();
    on_progress(&format!("Found {} markdown files", md_files.len()));

    if let Err(e) = db.clear_all() {
        result.errors.push(format!("Failed to clear database: {e}"));
        return result;
    }

    for file_path in &md_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to read {}: {e}", file_path.display()));
                continue;
            }
        };

        let rel_path = file_path
            .strip_prefix(knowledge_dir)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let chunks = dispatch_chunking(strategy, &content, &rel_path);

        for chunk in &chunks {
            let embedding = match embedder.embed(&embed_text(chunk)) {
                Ok(emb) => Some(emb),
                Err(e) => {
                    result
                        .errors
                        .push(format!("Embedding failed for {}: {e}", chunk.id));
                    None
                }
            };

            if let Err(e) = db.insert_chunk(chunk, embedding.as_deref()) {
                result
                    .errors
                    .push(format!("Insert failed for {}: {e}", chunk.id));
            } else {
                result.chunks_created += 1;
            }
        }

        result.files_processed += 1;
        on_progress(&format!("  {} → {} chunks", rel_path, chunks.len()));
    }

    // Record the HEAD commit for future delta ingests.
    if result.errors.is_empty()
        && git::is_git_repo(knowledge_dir)
        && let Ok(head) = git::head_commit(knowledge_dir)
        && let Err(e) = db.set_metadata(META_LAST_COMMIT, &head)
    {
        on_progress(&format!("Warning: failed to record commit SHA: {e}"));
    }

    result
}

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

/// Create a new pattern file, index it, and commit.
///
/// When `inbox_branch_prefix` is `Some`, the file is committed to a
/// per-submission branch and pushed to the remote instead of being written
/// to disk and indexed locally.
pub fn add_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    title: &str,
    body: &str,
    tags: &[&str],
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    let slug = slugify(title);
    if slug.is_empty() {
        anyhow::bail!("Title must contain at least one alphanumeric character");
    }
    let filename = slug.clone() + ".md";

    // Validate slug doesn't contain path traversal components.
    validate_slug(&filename)?;

    let content = build_file_content(title, body, tags);

    if let Some(prefix) = inbox_branch_prefix {
        let branch = git::commit_to_new_branch(
            knowledge_dir,
            prefix,
            &slug,
            &filename,
            &content,
            &format!("lore: add pattern \"{title}\""),
        )?;
        push_or_cleanup(knowledge_dir, &branch)?;

        return Ok(WriteResult {
            file_path: filename,
            chunks_indexed: 0,
            commit_status: CommitStatus::Pushed { branch },
            embedding_failures: 0,
        });
    }

    let file_path = knowledge_dir.join(&filename);

    if file_path.exists() {
        anyhow::bail!("File already exists: {filename}. Use update_pattern instead.");
    }

    std::fs::write(&file_path, &content)?;

    let (chunks, embedding_failures) =
        index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

    let commit_status = try_commit(
        knowledge_dir,
        &file_path,
        &format!("lore: add pattern \"{title}\""),
    );

    Ok(WriteResult {
        file_path: filename,
        chunks_indexed: chunks,
        commit_status,
        embedding_failures,
    })
}

/// Overwrite an existing pattern file, re-index it, and commit.
///
/// When `inbox_branch_prefix` is `Some`, the modification is committed to a
/// per-submission branch and pushed. The file must exist on the working tree
/// (trunk) — inbox-only files are not supported.
pub fn update_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    source_file: &str,
    body: &str,
    tags: &[&str],
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    let file_path = knowledge_dir.join(source_file);

    if !file_path.exists() {
        anyhow::bail!("File not found: {source_file}");
    }

    validate_within_dir(knowledge_dir, &file_path)?;

    let title = extract_title(&std::fs::read_to_string(&file_path)?)
        .unwrap_or_else(|| file_stem(source_file));

    let content = build_file_content(&title, body, tags);

    if let Some(prefix) = inbox_branch_prefix {
        let slug = file_stem(source_file);
        let branch = git::commit_to_new_branch(
            knowledge_dir,
            prefix,
            &slug,
            source_file,
            &content,
            &format!("lore: update pattern \"{title}\""),
        )?;
        push_or_cleanup(knowledge_dir, &branch)?;

        return Ok(WriteResult {
            file_path: source_file.to_string(),
            chunks_indexed: 0,
            commit_status: CommitStatus::Pushed { branch },
            embedding_failures: 0,
        });
    }

    std::fs::write(&file_path, &content)?;

    let (chunks, embedding_failures) =
        index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

    let commit_status = try_commit(
        knowledge_dir,
        &file_path,
        &format!("lore: update pattern \"{title}\""),
    );

    Ok(WriteResult {
        file_path: source_file.to_string(),
        chunks_indexed: chunks,
        commit_status,
        embedding_failures,
    })
}

/// Append a new section to an existing pattern file, re-index, and commit.
///
/// When `inbox_branch_prefix` is `Some`, the modification is committed to a
/// per-submission branch and pushed. The file must exist on the working tree
/// (trunk) — inbox-only files are not supported.
pub fn append_to_pattern(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    source_file: &str,
    heading: &str,
    body: &str,
    inbox_branch_prefix: Option<&str>,
) -> anyhow::Result<WriteResult> {
    let file_path = knowledge_dir.join(source_file);

    if !file_path.exists() {
        anyhow::bail!("File not found: {source_file}");
    }

    validate_within_dir(knowledge_dir, &file_path)?;

    let existing = std::fs::read_to_string(&file_path)?;
    let title = extract_title(&existing).unwrap_or_else(|| file_stem(source_file));

    let mut content = existing;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let _ = write!(content, "\n## {heading}\n\n");
    content.push_str(body);
    content.push('\n');

    if let Some(prefix) = inbox_branch_prefix {
        let slug = file_stem(source_file);
        let branch = git::commit_to_new_branch(
            knowledge_dir,
            prefix,
            &slug,
            source_file,
            &content,
            &format!("lore: append to \"{title}\" — {heading}"),
        )?;
        push_or_cleanup(knowledge_dir, &branch)?;

        return Ok(WriteResult {
            file_path: source_file.to_string(),
            chunks_indexed: 0,
            commit_status: CommitStatus::Pushed { branch },
            embedding_failures: 0,
        });
    }

    std::fs::write(&file_path, &content)?;

    let (chunks, embedding_failures) =
        index_single_file(db, embedder, knowledge_dir, &file_path, "heading")?;

    let commit_status = try_commit(
        knowledge_dir,
        &file_path,
        &format!("lore: append to \"{title}\" — {heading}"),
    );

    Ok(WriteResult {
        file_path: source_file.to_string(),
        chunks_indexed: chunks,
        commit_status,
        embedding_failures,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Index (or re-index) a single file: delete old chunks, chunk, embed, insert.
///
/// The `strategy` parameter selects the chunking approach (`"heading"` or
/// `"document"`).
///
/// Returns `(chunks_indexed, embedding_failures)`.
fn index_single_file(
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    knowledge_dir: &Path,
    file_path: &Path,
    strategy: &str,
) -> anyhow::Result<(usize, usize)> {
    let content = std::fs::read_to_string(file_path)?;
    let rel_path = file_path
        .strip_prefix(knowledge_dir)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    // Remove old chunks for this file.
    db.delete_by_source(&rel_path)?;

    let chunks = dispatch_chunking(strategy, &content, &rel_path);
    let mut count = 0;
    let mut embedding_failures = 0;

    for chunk in &chunks {
        let embedding = if let Ok(emb) = embedder.embed(&embed_text(chunk)) {
            Some(emb)
        } else {
            embedding_failures += 1;
            None
        };
        db.insert_chunk(chunk, embedding.as_deref())?;
        count += 1;
    }

    Ok((count, embedding_failures))
}

/// Validate that `file_path` lies within `knowledge_dir` after canonicalization.
///
/// This prevents path traversal attacks where a `source_file` like
/// `../../../etc/passwd` could escape the knowledge directory.
fn validate_within_dir(knowledge_dir: &Path, file_path: &Path) -> anyhow::Result<()> {
    let canon_dir = knowledge_dir.canonicalize()?;
    let canon_file = file_path.canonicalize()?;
    if !canon_file.starts_with(&canon_dir) {
        anyhow::bail!(
            "Path escapes the knowledge directory: {}",
            file_path.display()
        );
    }
    Ok(())
}

/// Validate that a slug-derived filename does not contain path traversal components.
fn validate_slug(filename: &str) -> anyhow::Result<()> {
    let path = PathBuf::from(filename);
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        ) {
            anyhow::bail!("Path escapes the knowledge directory: {filename}");
        }
    }
    if filename.contains("..") {
        anyhow::bail!("Path escapes the knowledge directory: {filename}");
    }
    Ok(())
}

/// Dispatch to the appropriate chunking function based on `strategy`.
fn dispatch_chunking(strategy: &str, content: &str, rel_path: &str) -> Vec<Chunk> {
    if strategy == "heading" {
        chunk_by_heading(content, rel_path)
    } else {
        chunk_as_document(content, rel_path)
    }
}

/// Push a branch to the remote, deleting the local ref if the push fails.
fn push_or_cleanup(knowledge_dir: &Path, branch: &str) -> anyhow::Result<()> {
    if let Err(e) = git::push_branch(knowledge_dir, branch) {
        // Clean up the orphaned local branch ref before propagating.
        let _ = std::process::Command::new("git")
            .args(["update-ref", "-d", &format!("refs/heads/{branch}")])
            .current_dir(knowledge_dir)
            .output();
        return Err(e);
    }
    Ok(())
}

/// Attempt a git commit; return [`CommitStatus::NotCommitted`] if not a git
/// repo or the commit fails.
fn try_commit(knowledge_dir: &Path, file_path: &Path, message: &str) -> CommitStatus {
    if !git::is_git_repo(knowledge_dir) {
        return CommitStatus::NotCommitted;
    }
    if git::add_and_commit(knowledge_dir, file_path, message).is_ok() {
        CommitStatus::Committed
    } else {
        CommitStatus::NotCommitted
    }
}

/// Turn a title into a filename-safe slug.
fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Build a markdown file from title, body, and optional frontmatter tags.
fn build_file_content(title: &str, body: &str, tags: &[&str]) -> String {
    let mut content = String::new();
    if !tags.is_empty() {
        content.push_str("---\n");
        let _ = writeln!(content, "tags: [{}]", tags.join(", "));
        content.push_str("---\n\n");
    }
    let _ = write!(content, "# {title}\n\n");
    content.push_str(body);
    content.push('\n');
    content
}

/// Extract the filename stem (without extension) from a path string.
fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;
    use crate::database::KnowledgeDB;

    // -- embed_text -------------------------------------------------------

    #[test]
    fn embed_text_includes_title_tags_body() {
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Error Handling".into(),
            body: "Use anyhow for errors".into(),
            tags: "rust, anyhow".into(),
            source_file: "errors.md".into(),
            heading_path: String::new(),
        };
        assert_eq!(
            embed_text(&chunk),
            "Error Handling\nrust, anyhow\nUse anyhow for errors"
        );
    }

    #[test]
    fn embed_text_with_empty_tags() {
        let chunk = crate::chunking::Chunk {
            id: "c1".into(),
            title: "Title".into(),
            body: "Body".into(),
            tags: String::new(),
            source_file: "test.md".into(),
            heading_path: String::new(),
        };
        assert_eq!(embed_text(&chunk), "Title\n\nBody");
    }
    use crate::embeddings::FakeEmbedder;

    /// Open an in-memory `KnowledgeDB` with 768-dimension embeddings.
    fn memory_db() -> KnowledgeDB {
        let db = KnowledgeDB::open(Path::new(":memory:"), 768).unwrap();
        db.init().unwrap();
        db
    }

    /// Initialise a git repo in `dir` with a test user identity.
    fn git_init(dir: &Path) {
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
            // Disable GPG signing for test repos so commits don't require a key.
            vec!["config", "commit.gpgsign", "false"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git command failed");
        }
    }

    // -- slugify -----------------------------------------------------------

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_special_characters() {
        assert_eq!(slugify("Foo: Bar / Baz (v2)"), "foo-bar-baz-v2");
    }

    #[test]
    fn slugify_leading_trailing_dashes() {
        assert_eq!(slugify("  --Title-- "), "title");
    }

    // -- ingest (full directory) -------------------------------------------

    #[test]
    fn ingest_tempdir_with_markdown_files() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        fs::write(
            dir.join("alpha.md"),
            "# Alpha\n\nAlpha body text that is long enough for a chunk.\n",
        )
        .unwrap();
        fs::write(
            dir.join("beta.md"),
            "# Beta\n\nBeta body text that is long enough for a chunk.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.files_processed, 2);
        assert!(
            result.chunks_created >= 2,
            "expected >=2 chunks, got {}",
            result.chunks_created
        );
        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn ingest_empty_directory_returns_zero() {
        let tmp = tempdir().unwrap();
        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, tmp.path(), "heading", &|_| {});

        assert_eq!(result.files_processed, 0);
        assert_eq!(result.chunks_created, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn ingest_uses_document_strategy() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // A file with multiple headings — document strategy should produce 1 chunk.
        fs::write(
            dir.join("multi.md"),
            "# Top\n\nIntro text that is long enough.\n\n\
             ## Section\n\nSection body that is long enough.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "document", &|_| {});

        assert_eq!(result.files_processed, 1);
        // document mode produces exactly 1 chunk per file.
        assert_eq!(result.chunks_created, 1);
    }

    // -- add_pattern -------------------------------------------------------

    #[test]
    fn add_pattern_creates_file_with_frontmatter() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(
            &db,
            &embedder,
            dir,
            "My Pattern",
            "Pattern body that is long enough for chunking.",
            &["design", "rust"],
            None,
        )
        .unwrap();

        assert_eq!(result.file_path, "my-pattern.md");
        assert!(result.chunks_indexed >= 1);
        // Non-git directory: file is written and indexed, but not committed.
        assert!(matches!(result.commit_status, CommitStatus::NotCommitted));

        let content = fs::read_to_string(dir.join("my-pattern.md")).unwrap();
        assert!(content.contains("tags: [design, rust]"));
        assert!(content.contains("# My Pattern"));
        assert!(content.contains("Pattern body"));
    }

    #[test]
    fn add_pattern_rejects_existing_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(dir.join("existing.md"), "# Existing\n").unwrap();

        let result = add_pattern(&db, &embedder, dir, "Existing", "body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("already exists"), "unexpected error: {msg}");
    }

    // -- update_pattern ----------------------------------------------------

    #[test]
    fn update_pattern_overwrites_content_preserves_title() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("doc.md"),
            "# Original Title\n\nOld body that is long enough.\n",
        )
        .unwrap();

        let result = update_pattern(
            &db,
            &embedder,
            dir,
            "doc.md",
            "Brand new body that is long enough for a chunk.",
            &["updated"],
            None,
        )
        .unwrap();

        assert!(result.chunks_indexed >= 1);
        // Non-git directory: file is written and indexed, but not committed.
        assert!(matches!(result.commit_status, CommitStatus::NotCommitted));

        let content = fs::read_to_string(dir.join("doc.md")).unwrap();
        // Title should be preserved from the original file.
        assert!(content.contains("# Original Title"));
        assert!(content.contains("Brand new body"));
        assert!(content.contains("tags: [updated]"));
        // Old body should be gone.
        assert!(!content.contains("Old body"));
    }

    // -- append_to_pattern -------------------------------------------------

    #[test]
    fn append_to_pattern_adds_section() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        fs::write(
            dir.join("doc.md"),
            "# Doc Title\n\nExisting body that is long enough for chunking.\n",
        )
        .unwrap();

        let result = append_to_pattern(
            &db,
            &embedder,
            dir,
            "doc.md",
            "New Section",
            "Appended body that is long enough for a chunk.",
            None,
        )
        .unwrap();

        assert!(result.chunks_indexed >= 1);
        // Non-git directory: file is written and indexed, but not committed.
        assert!(matches!(result.commit_status, CommitStatus::NotCommitted));

        let content = fs::read_to_string(dir.join("doc.md")).unwrap();
        assert!(content.contains("# Doc Title"));
        assert!(content.contains("Existing body"));
        assert!(content.contains("## New Section"));
        assert!(content.contains("Appended body"));
    }

    // -- index_single_file strategy ----------------------------------------

    #[test]
    fn index_single_file_respects_strategy() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // File with multiple headings.
        let md = "# Top\n\nIntro text that is definitely long enough.\n\n\
                  ## Sub\n\nSub body text that is definitely long enough.\n";
        fs::write(dir.join("multi.md"), md).unwrap();

        let db_heading = memory_db();
        let db_doc = memory_db();
        let embedder = FakeEmbedder::new();

        let (heading_count, _) = index_single_file(
            &db_heading,
            &embedder,
            dir,
            &dir.join("multi.md"),
            "heading",
        )
        .unwrap();

        let (doc_count, _) =
            index_single_file(&db_doc, &embedder, dir, &dir.join("multi.md"), "document").unwrap();

        // heading strategy should produce more chunks than document strategy.
        assert!(
            heading_count > doc_count,
            "heading ({heading_count}) should produce more chunks than document ({doc_count})"
        );
        assert_eq!(doc_count, 1);
    }

    // -- try_commit (git integration) --------------------------------------

    #[test]
    fn try_commit_succeeds_in_git_repo() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("test.md");
        fs::write(&file, "# Test\n").unwrap();

        assert!(matches!(
            try_commit(dir, &file, "lore: test commit"),
            CommitStatus::Committed
        ));
    }

    #[test]
    fn try_commit_returns_not_committed_without_git() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        let file = dir.join("test.md");
        fs::write(&file, "# Test\n").unwrap();

        assert!(matches!(
            try_commit(dir, &file, "lore: test commit"),
            CommitStatus::NotCommitted
        ));
    }

    // -- write operations with git -----------------------------------------

    #[test]
    fn add_pattern_commits_in_git_repo() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        // Need an initial commit so HEAD exists.
        let seed = dir.join("seed.md");
        fs::write(&seed, "seed\n").unwrap();
        git::add_and_commit(dir, &seed, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(
            &db,
            &embedder,
            dir,
            "Git Test",
            "Body text that is long enough for a chunk.",
            &["test"],
            None,
        )
        .unwrap();

        assert!(matches!(result.commit_status, CommitStatus::Committed));

        // Verify the commit message prefix was renamed.
        let output = Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(dir)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert!(
            log.contains("lore: add pattern"),
            "commit message should start with 'lore:', got: {log}"
        );
    }

    // -- empty slug -------------------------------------------------------

    #[test]
    fn add_pattern_rejects_all_punctuation_title() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = add_pattern(&db, &embedder, dir, "!@#$%^&*()", "body text", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("alphanumeric"),
            "error should mention alphanumeric, got: {msg}"
        );
    }

    // -- path traversal ---------------------------------------------------

    #[test]
    fn update_pattern_rejects_path_traversal() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // Create a file outside the knowledge directory.
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("secret.md");
        fs::write(
            &outside_file,
            "# Secret\n\nSecret body that is long enough.\n",
        )
        .unwrap();

        // Construct a relative path that escapes the knowledge directory.
        let rel = pathdiff_relative(dir, &outside_file);

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = update_pattern(&db, &embedder, dir, &rel, "new body", &[], None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("escapes") || msg.contains("Path"),
            "error should mention path escaping, got: {msg}"
        );
    }

    #[test]
    fn append_to_pattern_rejects_path_traversal() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        // Create a file outside the knowledge directory.
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("secret.md");
        fs::write(
            &outside_file,
            "# Secret\n\nSecret body that is long enough.\n",
        )
        .unwrap();

        // Construct a relative path that escapes the knowledge directory.
        let rel = pathdiff_relative(dir, &outside_file);

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        let result = append_to_pattern(&db, &embedder, dir, &rel, "Hacked", "evil body", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("escapes") || msg.contains("Path"),
            "error should mention path escaping, got: {msg}"
        );
    }

    /// Compute a relative path from `base` to `target` using `..` components.
    fn pathdiff_relative(base: &Path, target: &Path) -> String {
        let base = fs::canonicalize(base).unwrap();
        let target = fs::canonicalize(target).unwrap();

        let mut base_parts: Vec<_> = base.components().collect();
        let target_parts: Vec<_> = target.components().collect();

        // Find common prefix length.
        let common = base_parts
            .iter()
            .zip(target_parts.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let ups = base_parts.len() - common;
        base_parts.clear();

        let mut rel = String::new();
        for _ in 0..ups {
            rel.push_str("../");
        }
        for part in &target_parts[common..] {
            rel.push_str(&part.as_os_str().to_string_lossy());
            rel.push('/');
        }
        // Remove trailing slash.
        if rel.ends_with('/') {
            rel.pop();
        }
        rel
    }

    // -- validate_slug rejects path traversal components ------------------

    #[test]
    fn validate_slug_rejects_dot_dot() {
        let result = validate_slug("../../../etc/passwd.md");
        assert!(result.is_err());
    }

    #[test]
    fn validate_slug_accepts_normal_filename() {
        let result = validate_slug("my-pattern.md");
        assert!(result.is_ok());
    }

    // -- full_ingest records commit SHA ------------------------------------

    #[test]
    fn full_ingest_records_head_commit() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("doc.md");
        fs::write(
            &file,
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.mode, IngestMode::Full);
        assert!(result.errors.is_empty());

        let stored = db.get_metadata(META_LAST_COMMIT).unwrap();
        assert!(stored.is_some(), "should have stored a commit SHA");
        assert_eq!(stored.unwrap().len(), 40);
    }

    // -- delta ingest tests ------------------------------------------------

    #[test]
    fn delta_ingest_processes_only_changed_files() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        // Create two files and do initial full ingest.
        let alpha = dir.join("alpha.md");
        let beta = dir.join("beta.md");
        fs::write(
            &alpha,
            "# Alpha\n\nAlpha body text that is long enough for a chunk.\n",
        )
        .unwrap();
        fs::write(
            &beta,
            "# Beta\n\nBeta body text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &alpha, "add alpha").unwrap();
        git::add_and_commit(dir, &beta, "add beta").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = full_ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(result.errors.is_empty());
        let initial_chunks = result.chunks_created;

        // Modify only alpha.
        fs::write(
            &alpha,
            "# Alpha\n\nUpdated alpha body that is long enough.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &alpha, "update alpha").unwrap();

        // Delta ingest should only process the modified file.
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(
            matches!(result.mode, IngestMode::Delta { .. }),
            "expected Delta mode, got {:?}",
            result.mode
        );
        assert_eq!(
            result.files_processed, 1,
            "should only process the modified file"
        );
        assert!(result.errors.is_empty());

        // Beta chunks should still be in the database.
        let stats = db.stats().unwrap();
        assert!(
            stats.chunks >= initial_chunks,
            "beta chunks should be preserved"
        );
    }

    #[test]
    fn delta_ingest_handles_deleted_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let alpha = dir.join("alpha.md");
        let beta = dir.join("beta.md");
        fs::write(
            &alpha,
            "# Alpha\n\nAlpha body text that is long enough for a chunk.\n",
        )
        .unwrap();
        fs::write(
            &beta,
            "# Beta\n\nBeta body text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &alpha, "add alpha").unwrap();
        git::add_and_commit(dir, &beta, "add beta").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Delete alpha.
        fs::remove_file(&alpha).unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "delete alpha"])
            .current_dir(dir)
            .output()
            .unwrap();

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert_eq!(result.files_processed, 1);
        assert!(result.errors.is_empty());

        // Alpha chunks should be gone, beta chunks should remain.
        let stats = db.stats().unwrap();
        assert_eq!(stats.sources, 1, "only beta should remain");
    }

    #[test]
    fn delta_ingest_handles_renamed_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let old_name = dir.join("old-name.md");
        fs::write(
            &old_name,
            "# Rename Test\n\nBody text that is long enough for rename detection by git.\n\nThis extra paragraph makes the content substantial.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &old_name, "add file").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // Verify old-name chunks exist.
        let stats_before = db.stats().unwrap();
        assert_eq!(stats_before.sources, 1);

        // Rename via git mv.
        Command::new("git")
            .args(["mv", "old-name.md", "new-name.md"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "rename file"])
            .current_dir(dir)
            .output()
            .unwrap();

        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert!(result.errors.is_empty());

        // Old source should be gone, new source should exist.
        let stats_after = db.stats().unwrap();
        assert_eq!(stats_after.sources, 1, "should still have exactly 1 source");

        // Search should find chunks under the new source file.
        let results = db.search_fts("rename", 10).unwrap();
        assert!(!results.is_empty(), "should find chunks from renamed file");
        assert_eq!(results[0].source_file, "new-name.md");
    }

    #[test]
    fn delta_ingest_no_changes_returns_early() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("doc.md");
        fs::write(
            &file,
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        full_ingest(&db, &embedder, dir, "heading", &|_| {});

        // No changes — delta should return immediately.
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert!(matches!(result.mode, IngestMode::Delta { .. }));
        assert_eq!(result.files_processed, 0);
        assert_eq!(result.chunks_created, 0);
    }

    #[test]
    fn ingest_falls_back_to_full_without_git() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        fs::write(
            dir.join("doc.md"),
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});

        assert_eq!(result.mode, IngestMode::Full);
        assert_eq!(result.files_processed, 1);
    }

    #[test]
    fn ingest_falls_back_to_full_without_stored_commit() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("doc.md");
        fs::write(
            &file,
            "# Doc\n\nBody text that is long enough for a chunk.\n",
        )
        .unwrap();
        git::add_and_commit(dir, &file, "initial").unwrap();

        let db = memory_db();
        let embedder = FakeEmbedder::new();

        // First ingest() call with no stored commit falls back to full.
        let result = ingest(&db, &embedder, dir, "heading", &|_| {});
        assert_eq!(result.mode, IngestMode::Full);
    }
}
