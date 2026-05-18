// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons
)]

//! Shared trace-walk predicate.
//!
//! `is_real_trace_file` is the single decision point for "is this
//! directory entry a real trace data file?" used by
//! [`super::stats::TraceStats::compute`] and
//! `maintenance::enumerate_trace_files`. Codifying the filter in one
//! place keeps the two writer surfaces — `lore status` /
//! `lore_status.trace` JSON and the maintenance pass — self-symmetric
//! on the symlink-safety and extension-gate discipline.
//!
//! `query::list_trace_files_newest_first` deliberately does NOT
//! delegate here — `lore trace why` is a read-only surface where the
//! symlink-safety argument that drove the filter in the two writers
//! does not apply. See the rationale comment above
//! `list_trace_files_newest_first` for details.
//!
//! Predicate semantics: a real trace data file is a regular file whose
//! name has a `.jsonl` or `.jsonl.gz` extension and is not the
//! maintenance state file ([`super::maintenance::LAST_PRUNED_AT_FILE`]).
//! Symlinks are skipped. The predicate owns the single
//! `symlink_metadata` syscall and returns the metadata to its caller —
//! callers retain their own `modified()` fallback policy (stats skips
//! aggregate updates on failure; maintenance treats the missing mtime
//! as `UNIX_EPOCH` so the file is eligible for prune).

use std::fs::Metadata;
use std::path::Path;

use super::maintenance::LAST_PRUNED_AT_FILE;

/// Return the file metadata for `path` when it is a real trace data
/// file (regular file, `.jsonl` or `.jsonl.gz` extension, not a
/// symlink, not [`LAST_PRUNED_AT_FILE`]); otherwise return `None`.
///
/// Filter order: name check (reject the maintenance state file) →
/// extension check (reject anything outside the trace-data accept
/// set) → `symlink_metadata` → reject symlinks and non-regular
/// files. The state-file rejection is explicit by name rather than
/// implicit via the extension filter — a future rename of
/// [`LAST_PRUNED_AT_FILE`] does not silently break the invariant.
///
/// Returns metadata only — no derived mtime — so callers can keep
/// their own per-policy fallback when `meta.modified()` fails.
pub(super) fn is_real_trace_file(path: &Path) -> Option<Metadata> {
    let name = path.file_name().and_then(|s| s.to_str())?;
    if name == LAST_PRUNED_AT_FILE {
        return None;
    }
    if !(name.ends_with(".jsonl") || name.ends_with(".jsonl.gz")) {
        return None;
    }
    // `symlink_metadata` (lstat) so a symlink looks like a symlink
    // rather than its target. The two writer surfaces (stats and
    // maintenance) refuse to count or touch entries that aren't real
    // files inside the trace dir; the predicate owns that discipline
    // for both. The compress/prune phases would otherwise gzip and
    // delete the symlink target — a file outside the lore-managed
    // state tier.
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_symlink() || !meta.is_file() {
        return None;
    }
    Some(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_regular_jsonl_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");
        std::fs::write(&path, b"{}\n").unwrap();
        let meta = is_real_trace_file(&path).expect("regular .jsonl should pass");
        assert_eq!(meta.len(), 3);
    }

    #[test]
    fn accepts_regular_jsonl_gz_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl.gz");
        std::fs::write(&path, b"\x1f\x8b").unwrap();
        let meta = is_real_trace_file(&path).expect("regular .jsonl.gz should pass");
        assert_eq!(meta.len(), 2);
    }

    #[test]
    fn skips_last_pruned_at_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(LAST_PRUNED_AT_FILE);
        std::fs::write(&path, b"0").unwrap();
        assert!(
            is_real_trace_file(&path).is_none(),
            "the maintenance state file must be skipped by explicit name"
        );
    }

    #[test]
    fn skips_non_jsonl_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("README.md");
        std::fs::write(&path, b"# notes").unwrap();
        assert!(is_real_trace_file(&path).is_none());
    }

    #[test]
    fn skips_dotfile_without_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.tmp");
        std::fs::write(&path, b"").unwrap();
        assert!(is_real_trace_file(&path).is_none());
    }

    #[test]
    fn skips_subdirectory_with_jsonl_name() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested.jsonl");
        std::fs::create_dir(&path).unwrap();
        assert!(
            is_real_trace_file(&path).is_none(),
            "a directory with a .jsonl suffix must not be classified as a trace file"
        );
    }

    #[test]
    fn skips_path_that_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing.jsonl");
        assert!(
            is_real_trace_file(&path).is_none(),
            "a non-existent path must not panic and must not be classified as a trace file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlink_pointing_outside_trace_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().join("traces");
        std::fs::create_dir_all(&trace_dir).unwrap();
        let external = tmp.path().join("external.jsonl");
        std::fs::write(&external, b"{}\n").unwrap();
        let link = trace_dir.join("via-symlink.jsonl");
        std::os::unix::fs::symlink(&external, &link).unwrap();
        assert!(
            is_real_trace_file(&link).is_none(),
            "the predicate must refuse symlinks regardless of where they point"
        );
    }
}
