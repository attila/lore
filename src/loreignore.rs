// SPDX-License-Identifier: MIT OR Apache-2.0

//! `.loreignore` file parsing and matching.
//!
//! Provides gitignore-style file exclusion for pattern repositories. A
//! `.loreignore` file at the repository root specifies files and directories
//! to exclude from indexing during both full and delta ingest.
//!
//! Supports gitignore semantics natively via the `ignore` crate: bare
//! filenames, trailing-slash directory patterns, wildcards, recursive globs
//! (`**/*.draft.md`), anchoring rules (patterns with `/` are repo-rooted), and
//! negation patterns (`!important.md` un-ignores a previously matched file).

use crate::hash::fnv1a;
use crate::lore_debug;
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io::Read as _;
use std::path::Path;

/// Maximum bytes to read from a `.loreignore` file. Files exceeding this
/// limit are rejected with a warning to stderr (no filtering applied).
///
/// Consistent with the bounded-read security convention from commit `18ac741`.
pub const MAX_LOREIGNORE_BYTES: usize = 65_536;

/// Filename of the ignore file at the repository root.
pub const LOREIGNORE_FILENAME: &str = ".loreignore";

/// Result of loading a `.loreignore` file once.
///
/// Both the matcher and the content hash are derived from the same byte
/// sequence, eliminating the race window where two sequential reads of the
/// file could observe different content.
#[derive(Debug)]
pub struct LoadedIgnore {
    /// Compiled gitignore matcher, or `None` when there are no effective
    /// patterns (file absent, empty, oversized, or comment-only).
    pub matcher: Option<Gitignore>,
    /// Hex-encoded FNV-1a hash of the file contents, or empty string when the
    /// file is absent or oversized. Empty string is the canonical sentinel for
    /// "no `.loreignore` file" — both ingest paths compare against this value.
    pub hash: String,
}

impl LoadedIgnore {
    /// Sentinel value used when there is no `.loreignore` file or it is
    /// rejected for being too large.
    pub const fn empty() -> Self {
        Self {
            matcher: None,
            hash: String::new(),
        }
    }
}

/// Load `.loreignore` from a knowledge directory in a single read.
///
/// Returns a [`LoadedIgnore`] containing both the compiled matcher (if any)
/// and the content hash, both derived from the same byte sequence. This
/// ensures the matcher and hash always reflect the same file state, even if
/// the file is modified concurrently between calls.
///
/// On absent, unreadable, or oversized files, the matcher is `None` and the
/// hash is empty — preserving the opt-in invariant (R3) that without a
/// `.loreignore` file, all markdown files are indexed.
pub fn load(knowledge_dir: &Path) -> LoadedIgnore {
    let path = knowledge_dir.join(LOREIGNORE_FILENAME);
    if !path.exists() {
        lore_debug!("loreignore: no file at {}", path.display());
        return LoadedIgnore::empty();
    }

    let bytes = match read_bounded(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("warning: could not read {}: {e}", path.display());
            return LoadedIgnore::empty();
        }
    };

    lore_debug!(
        "loreignore: read {} bytes from {}",
        bytes.len(),
        path.display()
    );
    let hash = format!("{:016x}", fnv1a(&bytes));
    let matcher = build_matcher(&path, knowledge_dir, &bytes);
    LoadedIgnore { matcher, hash }
}

/// Build a [`Gitignore`] matcher from already-read bytes.
///
/// Returns `None` when the file produces zero effective patterns. Malformed
/// individual patterns emit a warning to stderr but do not abort parsing.
fn build_matcher(path: &Path, knowledge_dir: &Path, bytes: &[u8]) -> Option<Gitignore> {
    let contents = std::str::from_utf8(bytes).ok()?;
    let mut builder = GitignoreBuilder::new(knowledge_dir);
    let mut had_error = false;
    for (lineno, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Err(e) = builder.add_line(None, line) {
            eprintln!(
                "warning: {}:{}: invalid ignore pattern '{}': {e}",
                path.display(),
                lineno + 1,
                line
            );
            had_error = true;
        }
    }

    match builder.build() {
        Ok(gi) if gi.num_ignores() == 0 && gi.num_whitelists() == 0 => {
            if !had_error {
                lore_debug!("loreignore: no effective patterns in {}", path.display());
            }
            None
        }
        Ok(gi) => {
            lore_debug!(
                "loreignore: loaded {} patterns ({} ignore, {} whitelist) from {}",
                gi.num_ignores() + gi.num_whitelists(),
                gi.num_ignores(),
                gi.num_whitelists(),
                path.display()
            );
            Some(gi)
        }
        Err(e) => {
            eprintln!(
                "warning: failed to build ignore matcher from {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// Read a file with a hard byte limit.
///
/// Returns an error if the file exceeds `MAX_LOREIGNORE_BYTES`. The limit is
/// enforced before parsing to prevent OOM on hostile or accidentally huge
/// ignore files.
fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_LOREIGNORE_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} exceeds {} byte limit ({} bytes)",
                path.display(),
                MAX_LOREIGNORE_BYTES,
                metadata.len()
            ),
        ));
    }

    // The size check above guarantees metadata.len() ≤ MAX_LOREIGNORE_BYTES,
    // which fits in usize on every supported target.
    let capacity = usize::try_from(metadata.len()).unwrap_or(MAX_LOREIGNORE_BYTES);
    let mut buf = Vec::with_capacity(capacity);
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Check whether a relative path should be excluded from indexing.
///
/// Returns `true` for paths matched by an ignore pattern with no negation,
/// and `false` for paths that are explicitly whitelisted (`!`) or unmatched.
///
/// `rel_path` must be relative to the knowledge directory (the same root
/// passed to `load()`). `is_dir` indicates whether the path refers to a
/// directory — necessary for trailing-slash patterns to match correctly.
pub fn is_ignored(gitignore: &Gitignore, rel_path: &Path, is_dir: bool) -> bool {
    matches!(
        gitignore.matched_path_or_any_parents(rel_path, is_dir),
        Match::Ignore(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_loreignore(dir: &Path, contents: &str) {
        fs::write(dir.join(LOREIGNORE_FILENAME), contents).unwrap();
    }

    fn matcher(dir: &Path) -> Gitignore {
        load(dir).matcher.expect("expected a matcher")
    }

    #[test]
    fn load_returns_empty_when_file_absent() {
        let dir = tempdir().unwrap();
        let loaded = load(dir.path());
        assert!(loaded.matcher.is_none());
        assert!(loaded.hash.is_empty());
    }

    #[test]
    fn load_matches_bare_filenames_directories_and_wildcards() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "README.md\ndocs/\n*.txt\n");
        let gi = matcher(dir.path());

        assert!(is_ignored(&gi, Path::new("README.md"), false));
        assert!(is_ignored(&gi, Path::new("docs"), true));
        assert!(is_ignored(&gi, Path::new("docs/intro.md"), false));
        assert!(is_ignored(&gi, Path::new("notes.txt"), false));
        assert!(!is_ignored(&gi, Path::new("rust/error-handling.md"), false));
    }

    #[test]
    fn trailing_slash_patterns_match_directories_only() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "build/\n");
        let gi = matcher(dir.path());

        assert!(is_ignored(&gi, Path::new("build"), true));
        assert!(!is_ignored(&gi, Path::new("build"), false));
    }

    #[test]
    fn anchored_patterns_match_from_root_only() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "/top.md\n");
        let gi = matcher(dir.path());

        assert!(is_ignored(&gi, Path::new("top.md"), false));
        assert!(!is_ignored(&gi, Path::new("nested/top.md"), false));
    }

    #[test]
    fn recursive_globs_match_in_subdirectories() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "**/*.draft.md\n");
        let gi = matcher(dir.path());

        assert!(is_ignored(&gi, Path::new("notes.draft.md"), false));
        assert!(is_ignored(&gi, Path::new("rust/idea.draft.md"), false));
        assert!(!is_ignored(&gi, Path::new("rust/idea.md"), false));
    }

    #[test]
    fn empty_or_comment_only_file_returns_no_matcher_but_real_hash() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "# comment one\n\n  # indented comment\n\n");
        let loaded = load(dir.path());
        assert!(loaded.matcher.is_none());
        // Hash is non-empty because the file exists and is within the size
        // limit, even though no patterns are effective.
        assert!(!loaded.hash.is_empty());
    }

    #[test]
    fn oversized_file_returns_empty_with_warning() {
        let dir = tempdir().unwrap();
        let oversized = "*.md\n".repeat(MAX_LOREIGNORE_BYTES);
        write_loreignore(dir.path(), &oversized);
        let loaded = load(dir.path());
        assert!(loaded.matcher.is_none());
        assert!(loaded.hash.is_empty());
    }

    #[test]
    fn malformed_pattern_skipped_other_patterns_apply() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "README.md\n[invalid\n");
        let gi = matcher(dir.path());
        assert!(is_ignored(&gi, Path::new("README.md"), false));
    }

    #[test]
    fn negation_un_ignores_a_previously_matched_file() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "*.md\n!important.md\n");
        let gi = matcher(dir.path());

        assert!(is_ignored(&gi, Path::new("README.md"), false));
        assert!(!is_ignored(&gi, Path::new("important.md"), false));
    }

    #[test]
    fn negation_without_preceding_exclusion_is_no_op() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "!important.md\n");
        let loaded = load(dir.path());
        if let Some(gi) = loaded.matcher {
            assert!(!is_ignored(&gi, Path::new("important.md"), false));
        }
    }

    #[test]
    fn pattern_matching_loreignore_itself_does_not_panic() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), ".loreignore\n");
        let gi = matcher(dir.path());
        let _ = is_ignored(&gi, Path::new(".loreignore"), false);
    }

    #[test]
    fn hash_changes_when_file_changes() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "README.md\n");
        let h1 = load(dir.path()).hash;
        write_loreignore(dir.path(), "README.md\nLICENSE\n");
        let h2 = load(dir.path()).hash;
        assert_ne!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn hash_stable_for_identical_content() {
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "README.md\n");
        let h1 = load(dir.path()).hash;
        let h2 = load(dir.path()).hash;
        assert_eq!(h1, h2);
    }

    #[test]
    fn pattern_with_slash_matches_nested_file() {
        // Regression: a pattern like `yaml/formatting.md` (containing a
        // slash but no leading slash) is anchored to the matcher root.
        // It must match a relative path "yaml/formatting.md" exactly.
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "yaml/formatting.md\n");
        let gi = matcher(dir.path());

        assert!(
            is_ignored(&gi, Path::new("yaml/formatting.md"), false),
            "anchored slash pattern should match its exact relative path"
        );
        // Sanity: should not match the same filename in a different dir.
        assert!(!is_ignored(&gi, Path::new("other/formatting.md"), false));
    }

    #[test]
    fn matcher_and_hash_derived_from_same_bytes() {
        // Sanity check the atomicity invariant: two consecutive load() calls
        // on an unchanged file produce both the same hash AND a matcher that
        // behaves identically. This is the regression test for the previous
        // TOCTOU race where the matcher and hash came from separate reads.
        let dir = tempdir().unwrap();
        write_loreignore(dir.path(), "README.md\ndocs/\n");
        let a = load(dir.path());
        let b = load(dir.path());
        assert_eq!(a.hash, b.hash);
        let gi_a = a.matcher.unwrap();
        let gi_b = b.matcher.unwrap();
        assert_eq!(
            is_ignored(&gi_a, Path::new("README.md"), false),
            is_ignored(&gi_b, Path::new("README.md"), false),
        );
    }
}
