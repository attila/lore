// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons
)]

//! Trace-directory statistics surfaced through `lore status` and the
//! MCP `lore_status` tool.
//!
//! The CLI block and the MCP object both consume [`TraceStats`]; the
//! CLI renders human-formatted lines and the MCP handler emits the
//! JSON-shaped equivalent so operators see the same posture from
//! either surface.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::Config;

use super::maintenance::LAST_PRUNED_AT_FILE;

/// Snapshot of the trace directory's on-disk state plus the operator's
/// configured capture posture.
#[derive(Debug, Clone)]
pub struct TraceStats {
    pub directory: PathBuf,
    pub directory_exists: bool,
    pub session_count: usize,
    pub total_bytes: u64,
    pub oldest: Option<SystemTime>,
    pub newest: Option<SystemTime>,
    pub last_pruned_at: Option<SystemTime>,
    pub capture: CapturePosture,
}

/// Privacy-posture summary surfaced alongside the trace stats.
#[derive(Debug, Clone)]
pub struct CapturePosture {
    pub command_head_only: bool,
    pub transcript_tail_included: bool,
    /// String tokens describing privacy-elevated toggles that are on,
    /// e.g. `"full_command_body_captured"` and
    /// `"transcript_tail_captured"`. Empty in the default posture.
    pub warnings: Vec<&'static str>,
}

impl TraceStats {
    /// Compute stats by walking `trace_dir` and reading the
    /// `.last_pruned_at` state file. Returns a struct whose
    /// `directory_exists = false` when the trace directory hasn't been
    /// created yet — semantically equivalent to "empty trace dir".
    pub fn compute(trace_dir: &Path, config: &Config) -> Self {
        let capture = CapturePosture::from_config(config);
        let mut stats = Self {
            directory: trace_dir.to_path_buf(),
            directory_exists: trace_dir.exists(),
            session_count: 0,
            total_bytes: 0,
            oldest: None,
            newest: None,
            last_pruned_at: None,
            capture,
        };

        if !stats.directory_exists {
            return stats;
        }

        // Scan files. Treat both .jsonl and .jsonl.gz as sessions —
        // the count is sessions-on-disk, not retained-uncompressed.
        if let Ok(entries) = std::fs::read_dir(trace_dir) {
            for e in entries.flatten() {
                let path = e.path();
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name == LAST_PRUNED_AT_FILE {
                    let last = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .and_then(|secs| {
                            SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
                        });
                    stats.last_pruned_at = last;
                    continue;
                }
                if !(name.ends_with(".jsonl") || name.ends_with(".jsonl.gz")) {
                    continue;
                }
                if let Ok(meta) = e.metadata() {
                    stats.session_count += 1;
                    stats.total_bytes = stats.total_bytes.saturating_add(meta.len());
                    if let Ok(modified) = meta.modified() {
                        stats.oldest = Some(stats.oldest.map_or(modified, |o| o.min(modified)));
                        stats.newest = Some(stats.newest.map_or(modified, |n| n.max(modified)));
                    }
                }
            }
        }
        stats
    }
}

/// One row of the [`PRIVACY_ELEVATED_TOGGLES`] registry. Each entry maps a
/// `TraceConfig` accessor to the warning token surfaced through
/// `lore status` and the MCP `lore_status` `capture.warnings` array.
///
/// Future privacy-elevated toggles add a row here. The hand-maintained
/// `if config.trace.<toggle> { warnings.push(...) }` ladder was the
/// previous shape and silently bypassed audit when a contributor forgot
/// to update both surfaces.
struct PrivacyToggle {
    /// Read `true` iff this toggle is currently turned on for the config.
    is_elevated: fn(&crate::config::TraceConfig) -> bool,
    /// Stable token surfaced to MCP consumers in `capture.warnings`.
    /// Must not change across releases — agents pattern-match on it.
    warning_token: &'static str,
}

/// Single source of truth for privacy-elevated trace toggles. Walked by
/// [`CapturePosture::from_config`]; extend here when adding a new toggle.
const PRIVACY_ELEVATED_TOGGLES: &[PrivacyToggle] = &[
    PrivacyToggle {
        is_elevated: |t| t.include_full_command,
        warning_token: "full_command_body_captured",
    },
    PrivacyToggle {
        is_elevated: |t| t.include_transcript_tail,
        warning_token: "transcript_tail_captured",
    },
];

impl CapturePosture {
    pub fn from_config(config: &Config) -> Self {
        let warnings: Vec<&'static str> = PRIVACY_ELEVATED_TOGGLES
            .iter()
            .filter(|toggle| (toggle.is_elevated)(&config.trace))
            .map(|toggle| toggle.warning_token)
            .collect();
        Self {
            command_head_only: !config.trace.include_full_command,
            transcript_tail_included: config.trace.include_transcript_tail,
            warnings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::record::{AGENT_CLAUDE_CODE, PostCompactRecord, SCHEMA_VERSION, TraceRecord};
    use super::super::writer::append_record;
    use super::*;
    use crate::config::Config;

    fn sample_config() -> Config {
        Config::default_with(
            std::path::PathBuf::from("docs"),
            std::path::PathBuf::from("lore.db"),
            "nomic-embed-text",
        )
    }

    #[test]
    fn empty_trace_dir_reports_zero_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let stats = TraceStats::compute(tmp.path(), &sample_config());
        assert!(stats.directory_exists);
        assert_eq!(stats.session_count, 0);
        assert_eq!(stats.total_bytes, 0);
    }

    #[test]
    fn missing_trace_dir_is_graceful() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("missing");
        let stats = TraceStats::compute(&dir, &sample_config());
        assert!(!stats.directory_exists);
        assert_eq!(stats.session_count, 0);
    }

    #[test]
    fn populated_trace_dir_reports_counts_and_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().join("t");
        for id in ["a", "b"] {
            let rec = TraceRecord::PostCompact(PostCompactRecord {
                schema_version: SCHEMA_VERSION,
                ts: "2026-05-15T14:00:00.000Z".to_string(),
                session_id: id.to_string(),
                agent: AGENT_CLAUDE_CODE.to_string(),
                duration_ms: 0,
            });
            append_record(&trace_dir, &rec);
        }
        let stats = TraceStats::compute(&trace_dir, &sample_config());
        assert_eq!(stats.session_count, 2);
        assert!(stats.total_bytes > 0);
        assert!(stats.oldest.is_some());
        assert!(stats.newest.is_some());
    }

    #[test]
    fn capture_posture_default_has_no_warnings() {
        let cfg = sample_config();
        let posture = CapturePosture::from_config(&cfg);
        assert!(posture.command_head_only);
        assert!(!posture.transcript_tail_included);
        assert!(posture.warnings.is_empty());
    }

    #[test]
    fn capture_posture_full_command_emits_warning() {
        let mut cfg = sample_config();
        cfg.trace.include_full_command = true;
        let posture = CapturePosture::from_config(&cfg);
        assert!(!posture.command_head_only);
        assert!(posture.warnings.contains(&"full_command_body_captured"));
    }

    #[test]
    fn capture_posture_transcript_tail_emits_warning() {
        let mut cfg = sample_config();
        cfg.trace.include_transcript_tail = true;
        let posture = CapturePosture::from_config(&cfg);
        assert!(posture.transcript_tail_included);
        assert!(posture.warnings.contains(&"transcript_tail_captured"));
    }
}
