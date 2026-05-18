// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]

//! Read-side for trace files: collect, filter, and render records for
//! `lore trace why`.
//!
//! Both plain `.jsonl` and `.jsonl.gz` files are read transparently.
//! Pretty-print sanitises `description`, `file_path`, and `command_head`
//! via `escape_debug` before emitting them to the terminal; the
//! `--json` pass-through stays raw — the on-disk bytes are already JSON-
//! encoded escapes, so a downstream `jq` consumer can rely on the same
//! shape it would get straight off disk.

use std::io::Read as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::record::TraceRecord;

/// Collect records matching the given filters.
///
/// If `session` is `Some`, reads only that session's trace file. If
/// `recent` is `Some(N)`, walks every trace file in the directory
/// newest-first and returns at most `N` records across them all. Filters
/// are AND-combined; an unset filter matches anything.
pub fn collect(
    trace_dir: &Path,
    session: Option<&str>,
    recent: Option<usize>,
    event: Option<&str>,
    tool: Option<&str>,
    agent: Option<&str>,
) -> anyhow::Result<Vec<TraceRecord>> {
    collect_with_diagnostics(trace_dir, session, recent, event, tool, agent, true)
}

/// Variant of [`collect`] that lets the caller suppress stderr
/// diagnostics for malformed JSON lines. `lore trace why --json`
/// passes `false` to keep its pass-through quiet for downstream `jq`.
pub fn collect_with_diagnostics(
    trace_dir: &Path,
    session: Option<&str>,
    recent: Option<usize>,
    event: Option<&str>,
    tool: Option<&str>,
    agent: Option<&str>,
    diagnostics: bool,
) -> anyhow::Result<Vec<TraceRecord>> {
    let files = if let Some(s) = session {
        let hash = crate::hash::fnv1a(s.as_bytes());
        let plain = trace_dir.join(format!("{hash:016x}.jsonl"));
        let gz = trace_dir.join(format!("{hash:016x}.jsonl.gz"));
        // Prefer `.jsonl` over `.jsonl.gz` when both exist — that's the
        // mid-gzip state. Reading both would surface duplicate records
        // until the pending unlink completes.
        let mut out = Vec::new();
        if plain.exists() {
            out.push(plain);
        } else if gz.exists() {
            out.push(gz);
        }
        out
    } else {
        list_trace_files_newest_first(trace_dir)?
    };

    let predicate = |r: &TraceRecord| {
        let event_ok = event.is_none_or(|e| r.event_name() == e);
        let tool_ok = tool.is_none_or(|t| r.tool_name() == Some(t));
        let agent_ok = agent.is_none_or(|a| r.agent() == a);
        event_ok && tool_ok && agent_ok
    };

    let mut all: Vec<TraceRecord> = Vec::new();
    for path in &files {
        let mut records = read_records(path, diagnostics)?;
        // Files are walked newest-first by mtime, but records inside a
        // file are appended oldest-first. Reverse per-file so the
        // `--recent` cap drops the OLDEST records, not the newest.
        records.reverse();
        all.extend(records.into_iter().filter(&predicate));
        // Apply the cap AFTER the filter so a `--recent 5 --event X`
        // request doesn't terminate the walk on an unfiltered tail and
        // miss matching records in older files.
        if let Some(cap) = recent
            && all.len() >= cap
        {
            all.truncate(cap);
            break;
        }
    }
    Ok(all)
}

/// Walk `trace_dir`, returning `.jsonl[.gz]` files in newest-first
/// mtime order. Skips the throttle state file.
///
/// Deliberately does NOT delegate to [`super::walk::is_real_trace_file`].
/// `lore trace why` is a read-only consumer surface: the
/// symlink-safety argument that drove the predicate in
/// [`super::stats::TraceStats::compute`] and
/// [`super::maintenance::enumerate_trace_files`] (don't gzip / delete
/// files outside the trace directory) doesn't apply here — reading a
/// symlinked trace file can't lose operator data. Following symlinks
/// (via `e.metadata()` rather than `symlink_metadata`) is the
/// historical behaviour and is preserved on purpose.
///
/// Consequence: `lore trace why` may surface sessions that
/// `lore status` does not count. That asymmetry is small and
/// acknowledged; a future contributor "unifying" the walk should
/// re-read this comment and the plan at
/// `docs/plans/2026-05-16-001-feat-trace-walk-predicate-plan.md`
/// before changing it.
fn list_trace_files_newest_first(trace_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !trace_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for e in std::fs::read_dir(trace_dir)?.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name == super::maintenance::LAST_PRUNED_AT_FILE {
            continue;
        }
        if !(name.ends_with(".jsonl") || name.ends_with(".jsonl.gz")) {
            continue;
        }
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((path, mtime));
    }
    entries.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
    Ok(entries.into_iter().map(|(p, _)| p).collect())
}

/// Read all records from one trace file. Transparently decompresses
/// `.gz` files. Malformed JSON lines are skipped — under `diagnostics`
/// a warning is emitted to stderr (tier-2 per the CLI behaviour
/// ladder); under `--json` pass-through the warning is suppressed via
/// the `diagnostics = false` path so downstream `jq` consumers see a
/// clean stream.
fn read_records(path: &Path, diagnostics: bool) -> anyhow::Result<Vec<TraceRecord>> {
    let raw = read_maybe_gzipped(path)?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<TraceRecord>(line) {
            Ok(r) => out.push(r),
            Err(e) => {
                if diagnostics {
                    eprintln!(
                        "lore trace: malformed line {} in {}: {e}",
                        i + 1,
                        path.display()
                    );
                } else {
                    crate::lore_debug!(
                        "lore trace: malformed line {} in {}: {e}",
                        i + 1,
                        path.display()
                    );
                }
            }
        }
    }
    Ok(out)
}

fn read_maybe_gzipped(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == "gz")
    {
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut s = String::new();
        decoder.read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(String::from_utf8(bytes)?)
    }
}

/// Pretty-print records to `out`. One block per record; control
/// characters in `description`, `file_path`, and `command_head` are
/// escaped via `escape_debug` so a tampered record cannot drive the
/// operator's terminal.
pub fn pretty_print<W: Write>(out: &mut W, records: &[TraceRecord]) -> std::io::Result<()> {
    for (i, rec) in records.iter().enumerate() {
        if i > 0 {
            writeln!(out, "---")?;
        }
        match rec {
            TraceRecord::PreToolUse(r) => {
                writeln!(
                    out,
                    "[{}] PreToolUse  session={}",
                    r.ts,
                    sanitize(&r.session_id)
                )?;
                writeln!(out, "  tool: {}", sanitize(&r.call_context.tool_name))?;
                if let Some(head) = &r.call_context.command_head {
                    writeln!(out, "  command_head: {}", sanitize(head))?;
                }
                if let Some(fp) = &r.call_context.file_path {
                    writeln!(out, "  file_path: {}", sanitize(fp))?;
                }
                if let Some(desc) = &r.call_context.description {
                    writeln!(out, "  description: {}", sanitize(desc))?;
                }
                if let Some(q) = &r.query {
                    writeln!(out, "  query: {}", sanitize(q))?;
                }
                writeln!(
                    out,
                    "  candidates: {} (injected: {})",
                    r.candidates.len(),
                    r.injected.len()
                )?;
                for c in &r.candidates {
                    let marker = if r.injected.contains(&c.chunk_id) {
                        "*"
                    } else {
                        " "
                    };
                    writeln!(
                        out,
                        "    {marker} {:.4} {} ({})",
                        c.score_combined,
                        sanitize(&c.source_file),
                        sanitize(&c.chunk_id)
                    )?;
                }
                writeln!(out, "  duration: {}ms", r.duration_ms)?;
            }
            TraceRecord::PostToolUse(r) => {
                writeln!(
                    out,
                    "[{}] PostToolUse session={}",
                    r.ts,
                    sanitize(&r.session_id)
                )?;
                writeln!(out, "  tool: {}", sanitize(&r.call_context.tool_name))?;
                if let Some(q) = &r.query {
                    writeln!(out, "  query: {}", sanitize(q))?;
                }
                writeln!(out, "  candidates: {}", r.candidates.len())?;
                writeln!(out, "  duration: {}ms", r.duration_ms)?;
            }
            TraceRecord::SessionStart(r) => {
                writeln!(
                    out,
                    "[{}] SessionStart session={}",
                    r.ts,
                    sanitize(&r.session_id)
                )?;
                writeln!(
                    out,
                    "  knowledge_dir: {}",
                    sanitize(&r.config.knowledge_dir)
                )?;
                writeln!(
                    out,
                    "  search: hybrid={} top_k={} min_relevance={:.4}",
                    r.config.search.hybrid, r.config.search.top_k, r.config.search.min_relevance
                )?;
                writeln!(out, "  duration: {}ms", r.duration_ms)?;
            }
            TraceRecord::PostCompact(r) => {
                writeln!(
                    out,
                    "[{}] PostCompact session={}",
                    r.ts,
                    sanitize(&r.session_id)
                )?;
                writeln!(out, "  duration: {}ms", r.duration_ms)?;
            }
        }
    }
    Ok(())
}

/// Escape control characters in semi-trusted strings before they reach
/// a terminal. Single source: [`crate::hook::sanitize_for_log`].
fn sanitize(s: &str) -> String {
    crate::hook::sanitize_for_log(s)
}

#[cfg(test)]
mod tests {
    use super::super::record::{AGENT_CLAUDE_CODE, PostCompactRecord, SCHEMA_VERSION, TraceRecord};
    use super::super::writer::append_record;
    use super::*;

    fn sample(session_id: &str) -> TraceRecord {
        TraceRecord::PostCompact(PostCompactRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:23:01.234Z".to_string(),
            session_id: session_id.to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            duration_ms: 5,
        })
    }

    #[test]
    fn collect_session_returns_all_records() {
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("t");
        append_record(&trace_dir, &sample("s1"));
        append_record(&trace_dir, &sample("s1"));
        let records = collect(&trace_dir, Some("s1"), None, None, None, None).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn collect_filters_by_event() {
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("t");
        append_record(&trace_dir, &sample("s1"));
        let records = collect(
            &trace_dir,
            Some("s1"),
            None,
            Some("SessionStart"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(records.len(), 0);
        let records = collect(
            &trace_dir,
            Some("s1"),
            None,
            Some("PostCompact"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn collect_missing_session_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("t");
        std::fs::create_dir_all(&trace_dir).unwrap();
        let records = collect(&trace_dir, Some("none"), None, None, None, None).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn collect_recent_caps_total() {
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("t");
        for id in ["a", "b", "c"] {
            append_record(&trace_dir, &sample(id));
        }
        let records = collect(&trace_dir, None, Some(2), None, None, None).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn pretty_print_emits_one_block_per_record() {
        let records = vec![sample("s1"), sample("s2")];
        let mut buf = Vec::new();
        pretty_print(&mut buf, &records).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("session=s1"));
        assert!(text.contains("session=s2"));
        assert!(text.contains("---"));
    }

    #[test]
    fn collect_reads_gzipped_session_file_transparently() {
        // A session whose `.jsonl` has been gzipped by the maintenance
        // pass must still be readable via `collect()` — gzip
        // transparency is the whole point of the `.gz` successor.
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces-gz");
        append_record(&trace_dir, &sample("s1"));
        append_record(&trace_dir, &sample("s1"));
        let raw_path = super::super::writer::trace_file_path(&trace_dir, "s1");
        let raw = std::fs::read(&raw_path).unwrap();
        // Gzip the file in place by writing the gz successor and
        // removing the source — mirrors what `maintenance::gzip_file`
        // does in production.
        let gz_path = raw_path.with_extension("jsonl.gz");
        let gz_file = std::fs::File::create(&gz_path).unwrap();
        let mut enc = flate2::write::GzEncoder::new(gz_file, flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &raw).unwrap();
        enc.finish().unwrap();
        std::fs::remove_file(&raw_path).unwrap();
        let records = collect(&trace_dir, Some("s1"), None, None, None, None).unwrap();
        assert_eq!(
            records.len(),
            2,
            "gzipped session must read back as two records, got: {records:?}"
        );
    }

    #[test]
    fn collect_skips_malformed_lines_without_aborting() {
        // A torn / corrupt line in a trace file (NFS interleave, partial
        // write, manual edit gone wrong) must not abort the read. The
        // surrounding valid records still surface; the malformed line
        // is reported via stderr under the diagnostics flag.
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces-malformed");
        std::fs::create_dir_all(&trace_dir).unwrap();
        let path = super::super::writer::trace_file_path(&trace_dir, "s1");
        let valid = serde_json::to_string(&sample("s1")).unwrap();
        let content = format!("{valid}\n{{not valid json}}\n{valid}\n");
        std::fs::write(&path, content).unwrap();
        let records =
            collect_with_diagnostics(&trace_dir, Some("s1"), None, None, None, None, false)
                .unwrap();
        assert_eq!(
            records.len(),
            2,
            "expected the two valid records, malformed line should be skipped, got: {records:?}"
        );
    }

    #[test]
    fn pretty_print_sanitises_control_characters_in_session_id() {
        // Defence against terminal-control-sequence injection: a
        // tampered session_id containing ANSI escapes must render as
        // an escaped literal, not drive the operator's terminal.
        let rec = TraceRecord::PostCompact(PostCompactRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:23:01.234Z".to_string(),
            session_id: "\x1b[2Jevil".to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            duration_ms: 5,
        });
        let mut buf = Vec::new();
        pretty_print(&mut buf, &[rec]).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            !text.contains('\x1b'),
            "raw ESC byte must not reach the terminal, got: {text:?}"
        );
        assert!(
            text.contains("evil"),
            "the visible suffix must survive sanitisation, got: {text:?}"
        );
    }

    #[test]
    fn collect_combined_filters_compose() {
        // --event + --tool + --agent are AND-combined; a record must
        // pass all three filters to land in the output.
        use super::super::record::{CallContextSnapshot, ConfigSnapshot, Phases, PreToolUseRecord};
        let dir = tempfile::tempdir().unwrap();
        let trace_dir = dir.path().join("traces-combined");

        let pre_bash = TraceRecord::PreToolUse(PreToolUseRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:00:00.000Z".to_string(),
            session_id: "s1".to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            call_context: CallContextSnapshot {
                tool_name: "Bash".to_string(),
                command_head: Some("git".to_string()),
                command_full: None,
                file_path: None,
                description: None,
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
        append_record(&trace_dir, &pre_bash);
        // A PostCompact record in the same session that should NOT
        // match an --event PreToolUse filter.
        append_record(&trace_dir, &sample("s1"));

        // Event-only filter retains the Bash record.
        let event_only =
            collect(&trace_dir, Some("s1"), None, Some("PreToolUse"), None, None).unwrap();
        assert_eq!(event_only.len(), 1, "event filter alone keeps PreToolUse");

        // Event + tool match the same record.
        let event_tool = collect(
            &trace_dir,
            Some("s1"),
            None,
            Some("PreToolUse"),
            Some("Bash"),
            None,
        )
        .unwrap();
        assert_eq!(event_tool.len(), 1);

        // Event + tool + agent — composes positively.
        let event_tool_agent = collect(
            &trace_dir,
            Some("s1"),
            None,
            Some("PreToolUse"),
            Some("Bash"),
            Some(AGENT_CLAUDE_CODE),
        )
        .unwrap();
        assert_eq!(event_tool_agent.len(), 1);

        // Event + agent with a wrong tool — composes negatively.
        let mismatched_tool = collect(
            &trace_dir,
            Some("s1"),
            None,
            Some("PreToolUse"),
            Some("Edit"),
            Some(AGENT_CLAUDE_CODE),
        )
        .unwrap();
        assert_eq!(mismatched_tool.len(), 0);
    }
}
