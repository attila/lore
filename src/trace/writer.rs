// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::doc_markdown)]

//! Append-only JSONL writer for [`TraceRecord`]s.
//!
//! The writer follows the fire-and-forget discipline that lore's hook
//! contract requires: every error is swallowed so the agent never breaks,
//! diagnostics surface only under `LORE_DEBUG=1`. Each call opens, writes
//! one line, and closes — no long-lived file handle, matching the
//! open-write-close pattern in `dedup_filter_and_record`.
//!
//! Files are created mode `0o600` and the parent directory mode `0o700` on
//! Unix, mirroring the `~/.ssh/id_rsa` discipline. Trace files contain
//! command heads, queries, and optionally full command bodies and
//! transcript tails — world-readable defaults are unsafe on multi-user
//! systems, shared CI runners, and containers with shared home volumes.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::lore_debug;

use super::record::TraceRecord;

/// File mode for trace JSONL files on Unix. World-unreachable by design.
#[cfg(unix)]
pub const TRACE_FILE_MODE: u32 = 0o600;

/// File mode for the trace directory on Unix. World-unreachable by design.
#[cfg(unix)]
pub const TRACE_DIR_MODE: u32 = 0o700;

/// Resolve the per-session trace file path under `trace_dir`.
///
/// File extension is `.jsonl`. Gzipped successors land at the same stem
/// with a `.gz` suffix; readers transparently auto-decompress.
///
/// `session_id` is hashed via FNV-1a into a deterministic 16-hex-char
/// filename. Mirrors the `dedup_file_path` discipline at
/// `src/hook.rs:dedup_file_path` and prevents an agent-controlled
/// `session_id` (e.g. `../../tmp/exfil`) from escaping the trace
/// directory or otherwise leaking into a path-component context.
pub fn trace_file_path(trace_dir: &Path, session_id: &str) -> PathBuf {
    let hash = crate::hash::fnv1a(session_id.as_bytes());
    trace_dir.join(format!("{hash:016x}.jsonl"))
}

/// Append one [`TraceRecord`] to the per-session JSONL file under
/// `trace_dir`. Fire-and-forget: errors are emitted to stderr only when
/// `LORE_DEBUG=1` is set; the hook contract is preserved regardless.
///
/// The caller is responsible for the `Config::trace_enabled()` gate. The
/// writer assumes tracing is already authorised and concerns itself only
/// with the I/O contract.
pub fn append_record(trace_dir: &Path, record: &TraceRecord) {
    if let Err(e) = append_record_inner(trace_dir, record) {
        lore_debug!("trace write failed: {e}");
    }
}

fn append_record_inner(trace_dir: &Path, record: &TraceRecord) -> anyhow::Result<()> {
    ensure_trace_dir(trace_dir)?;
    let path = trace_file_path(trace_dir, record.session_id());
    let mut file = open_append(&path)?;
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Create the trace directory (recursively) with the locked-down mode on
/// Unix. Re-application of permissions on every call is fine: the cost is
/// one `chmod` per write, and it self-heals if an operator widens the
/// directory mode out-of-band.
fn ensure_trace_dir(trace_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(trace_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let perms = std::fs::Permissions::from_mode(TRACE_DIR_MODE);
        // Swallow permission errors on the chmod — the directory exists,
        // the writer's job is preserving the hook contract, and the
        // `lore status` Trace block surfaces audit posture for the
        // operator. The `LORE_DEBUG` macro is fired by the caller if the
        // outer Result fails.
        let _ = std::fs::set_permissions(trace_dir, perms);
    }
    Ok(())
}

#[cfg(unix)]
fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(TRACE_FILE_MODE)
        .open(path)
}

#[cfg(not(unix))]
fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    // Best-effort on Windows; ACL semantics are documented as a gap.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::super::record::{AGENT_CLAUDE_CODE, PostCompactRecord, SCHEMA_VERSION, TraceRecord};
    use super::*;

    fn sample_post_compact(session_id: &str) -> TraceRecord {
        TraceRecord::PostCompact(PostCompactRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:23:01.234Z".to_string(),
            session_id: session_id.to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            duration_ms: 5,
        })
    }

    #[test]
    fn append_record_creates_file_with_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces");
        append_record(&trace_dir, &sample_post_compact("s1"));
        let path = trace_file_path(&trace_dir, "s1");
        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = contents.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: TraceRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.event_name(), "PostCompact");
        assert_eq!(parsed.session_id(), "s1");
    }

    #[test]
    fn two_appends_produce_two_lines_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces");
        append_record(&trace_dir, &sample_post_compact("s1"));
        append_record(&trace_dir, &sample_post_compact("s1"));
        let contents = std::fs::read_to_string(trace_file_path(&trace_dir, "s1")).unwrap();
        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn missing_parent_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        // Deliberately point at a deep nonexistent path.
        let trace_dir = dir.path().join("a").join("b").join("c");
        append_record(&trace_dir, &sample_post_compact("s1"));
        assert!(trace_file_path(&trace_dir, "s1").exists());
    }

    #[test]
    fn append_swallows_errors_silently() {
        // Pointing at a path component that already exists as a file (not
        // a directory) makes `create_dir_all` fail. The call must not
        // panic and must not propagate the error.
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"").unwrap();
        let trace_dir = blocker.join("traces");
        append_record(&trace_dir, &sample_post_compact("s1"));
        // No panic = test passes. The file is not created.
        assert!(!trace_file_path(&trace_dir, "s1").exists());
    }

    #[cfg(unix)]
    #[test]
    fn trace_files_are_mode_0o600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces");
        append_record(&trace_dir, &sample_post_compact("s1"));
        let path = trace_file_path(&trace_dir, "s1");
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, TRACE_FILE_MODE);
    }

    #[cfg(unix)]
    #[test]
    fn trace_directory_is_mode_0o700() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces");
        append_record(&trace_dir, &sample_post_compact("s1"));
        let meta = std::fs::metadata(&trace_dir).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, TRACE_DIR_MODE);
    }

    #[test]
    fn verbatim_capture_round_trips_control_chars() {
        use super::super::record::{CallContextSnapshot, ConfigSnapshot, Phases, PreToolUseRecord};
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces");
        let record = TraceRecord::PreToolUse(PreToolUseRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:23:01.234Z".to_string(),
            session_id: "s1".to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            call_context: CallContextSnapshot {
                tool_name: "Bash".to_string(),
                command_head: Some("git".to_string()),
                command_full: None,
                file_path: Some("../../../etc/passwd".to_string()),
                description: Some("\x1b[2J evil".to_string()),
                inferred_languages: vec![],
                transcript_tail: None,
            },
            query: None,
            candidates: vec![],
            injected: vec![],
            config: ConfigSnapshot {
                hybrid: true,
                top_k: 5,
                min_relevance: 0.6,
                min_relevance_universal: 0.6,
                embedder_model: "nomic-embed-text".to_string(),
            },
            ollama: None,
            duration_ms: 1,
            phases: Phases::default(),
        });
        append_record(&trace_dir, &record);
        let raw = std::fs::read_to_string(trace_file_path(&trace_dir, "s1")).unwrap();
        // JSON encodes the escape character; the disk file is safe to cat.
        assert!(
            !raw.contains('\x1b'),
            "raw ESC byte must not appear on disk, got: {raw}"
        );
        assert!(
            raw.contains("[2J evil"),
            "payload tail must survive escape, got: {raw}"
        );
        // Path-traversal-like content round-trips through serde without mutation.
        let parsed: TraceRecord = serde_json::from_str(raw.trim()).unwrap();
        match parsed {
            TraceRecord::PreToolUse(r) => {
                assert_eq!(
                    r.call_context.file_path.as_deref(),
                    Some("../../../etc/passwd")
                );
                assert_eq!(r.call_context.description.as_deref(), Some("\x1b[2J evil"));
            }
            _ => panic!("expected PreToolUse"),
        }
    }
}
