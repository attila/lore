// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons
)]

//! Lazy maintenance for the trace directory.
//!
//! Two entry points:
//!
//! - [`run_lazy`] — invoked from SessionStart, throttled to at most once
//!   per 24 hours, bounded by [`MAX_COMPRESS_PER_RUN`] +
//!   [`MAX_PRUNE_PER_RUN`]. Silent on success; failures degrade to
//!   `LORE_DEBUG`-gated stderr per the hook contract.
//! - [`run_manual`] — invoked from `lore trace prune`, unbounded with no
//!   throttle. Reports per-file errors to stderr but always returns
//!   summary stats.
//!
//! Both writers bump the `.last_pruned_at` state file in the trace
//! directory so the throttle is honest about which writer last ran.
//!
//! **Disk-state-with-in-memory-shadow.** The throttle decision consults
//! a process-local `LazyLock<Mutex<HashMap<PathBuf, SystemTime>>>` first
//! (cheap, no syscall), then falls back to reading `.last_pruned_at`
//! from disk. After every pass the in-memory map is updated
//! unconditionally; the disk write is best-effort. On hosts where the
//! state file is unwriteable (read-only mount, disk full, SELinux
//! denial) the cross-process throttle stops working but each process
//! still throttles itself, so a hot loop of SessionStarts in one
//! process can't trigger repeated full-directory walks. Pattern is the
//! standard one used by OpenTelemetry Collector's `file_storage`,
//! Datadog Agent, Vector, and Loki Promtail.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use crate::lore_debug;

/// Process-local shadow of the on-disk throttle state. Keyed on the
/// resolved trace directory path so tests using different temp dirs
/// don't share state. Updated unconditionally after every maintenance
/// pass; read before consulting disk.
static IN_MEMORY_THROTTLE: LazyLock<Mutex<HashMap<PathBuf, SystemTime>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// File name of the throttle state file inside the trace directory.
pub const LAST_PRUNED_AT_FILE: &str = ".last_pruned_at";

/// Maximum files compressed in a single lazy maintenance run.
pub const MAX_COMPRESS_PER_RUN: usize = 100;

/// Maximum files deleted in a single lazy maintenance run.
pub const MAX_PRUNE_PER_RUN: usize = 100;

/// Throttle window for lazy maintenance — at most one run per 24 hours.
pub const LAZY_THROTTLE: Duration = Duration::new(86_400, 0);

/// Summary of files affected by a maintenance run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceSummary {
    pub compressed: usize,
    pub deleted: usize,
    pub skipped: bool,
    pub errors: usize,
}

/// Run lazy maintenance against `trace_dir` with the configured
/// retention horizons. Honours the 24-hour throttle by reading the
/// `.last_pruned_at` state file. Silent on success; bumps the state
/// file regardless of whether any files were touched.
pub fn run_lazy(
    trace_dir: &Path,
    retain_days: u32,
    gzip_older_than_days: u32,
) -> MaintenanceSummary {
    if !trace_dir.exists() {
        return MaintenanceSummary::default();
    }
    let state_path = trace_dir.join(LAST_PRUNED_AT_FILE);
    // Consult the in-memory shadow first (cheap, no syscall). Falls
    // back to the on-disk state file when the process hasn't run a
    // pass yet (cold start).
    if let Some(last) = read_throttle_state(trace_dir, &state_path) {
        // `duration_since` returns `Err` when `last` is in the future
        // (clock skew + NTP correction, container snapshot, tampered
        // state file). Treat the future-timestamp case as "throttle is
        // not credible" and run the pass — otherwise a stale value
        // like `u64::MAX` would block maintenance until wall time
        // catches up, which can be years.
        match SystemTime::now().duration_since(last) {
            Ok(elapsed) if elapsed < LAZY_THROTTLE => {
                lore_debug!(
                    "trace maintenance: throttled (last run {}s ago)",
                    elapsed.as_secs()
                );
                return MaintenanceSummary {
                    skipped: true,
                    ..Default::default()
                };
            }
            Ok(_) => {} // elapsed >= throttle window — run.
            Err(_) => {
                lore_debug!("trace maintenance: state file in future, running");
            }
        }
    }
    let summary = run_pass(
        trace_dir,
        retain_days,
        gzip_older_than_days,
        Some(MAX_COMPRESS_PER_RUN),
        Some(MAX_PRUNE_PER_RUN),
        Verbosity::Silent,
    );
    record_throttle_state(trace_dir, &state_path, SystemTime::now());
    summary
}

/// Run unbounded maintenance with no throttle. Used by
/// `lore trace prune`. Always updates the `.last_pruned_at` state.
pub fn run_manual(
    trace_dir: &Path,
    retain_days: u32,
    gzip_older_than_days: u32,
) -> MaintenanceSummary {
    if !trace_dir.exists() {
        return MaintenanceSummary::default();
    }
    let summary = run_pass(
        trace_dir,
        retain_days,
        gzip_older_than_days,
        None,
        None,
        Verbosity::Verbose,
    );
    let state_path = trace_dir.join(LAST_PRUNED_AT_FILE);
    record_throttle_state(trace_dir, &state_path, SystemTime::now());
    summary
}

/// Diagnostic verbosity for `run_pass`. Lazy maintenance must stay
/// silent on per-file errors to honour the hook contract (R15); the
/// manual `lore trace prune` writer surfaces errors to operator stderr
/// per the CLI behaviour ladder's tier-2 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verbosity {
    Silent,
    Verbose,
}

/// Inner maintenance pass — compress first (so a freshly-gzipped file
/// can still be deleted on the same pass if it's old enough), then
/// prune. `None` caps mean unbounded.
fn run_pass(
    trace_dir: &Path,
    retain_days: u32,
    gzip_older_than_days: u32,
    compress_cap: Option<usize>,
    prune_cap: Option<usize>,
    verbosity: Verbosity,
) -> MaintenanceSummary {
    let mut summary = MaintenanceSummary::default();
    let now = SystemTime::now();
    let retain_horizon = duration_from_days(retain_days);
    let gzip_horizon = duration_from_days(gzip_older_than_days);

    let Some(files) = enumerate_trace_files(trace_dir, &mut summary) else {
        return summary;
    };

    // Compress phase. Only .jsonl files older than the gzip horizon.
    for (path, mtime) in &files {
        if compress_cap.is_some_and(|cap| summary.compressed >= cap) {
            break;
        }
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        if gzip_older_than_days == 0 {
            continue;
        }
        let age = now.duration_since(*mtime).unwrap_or_default();
        if age < gzip_horizon {
            continue;
        }
        match gzip_file(path) {
            Ok(()) => summary.compressed += 1,
            Err(e) => {
                summary.errors += 1;
                report_error(
                    verbosity,
                    format_args!("gzip {} failed: {e}", path.display()),
                );
            }
        }
    }

    // Re-read entries since the compress phase may have added .gz files
    // and removed .jsonl ones.
    let Some(all) = enumerate_trace_files(trace_dir, &mut summary) else {
        return summary;
    };
    for (path, mtime) in &all {
        if prune_cap.is_some_and(|cap| summary.deleted >= cap) {
            break;
        }
        if retain_days == 0 {
            continue;
        }
        let age = now.duration_since(*mtime).unwrap_or_default();
        if age < retain_horizon {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => summary.deleted += 1,
            Err(e) => {
                summary.errors += 1;
                report_error(
                    verbosity,
                    format_args!("delete {} failed: {e}", path.display()),
                );
            }
        }
    }
    summary
}

/// Enumerate trace files in `trace_dir`, skipping the throttle state
/// file and non-files. Returns `None` and bumps `summary.errors` when
/// the directory read itself fails; the caller short-circuits the pass.
fn enumerate_trace_files(
    trace_dir: &Path,
    summary: &mut MaintenanceSummary,
) -> Option<Vec<(PathBuf, SystemTime)>> {
    let entries = match std::fs::read_dir(trace_dir) {
        Ok(rd) => rd,
        Err(e) => {
            summary.errors += 1;
            lore_debug!("trace maintenance: read_dir failed: {e}");
            return None;
        }
    };
    let mut out: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|s| s.to_str()) == Some(LAST_PRUNED_AT_FILE) {
            continue;
        }
        // Use `symlink_metadata` (lstat) so a symlink looks like a
        // symlink rather than its target. The maintenance pass must
        // not gzip/delete files outside the trace directory; an
        // operator who symlinks `~/.bashrc` into the trace dir would
        // otherwise see lore eat the target after the retention
        // horizon. Skip both symlinks and non-files outright.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        out.push((path, mtime));
    }
    Some(out)
}

/// Route per-file maintenance errors to the right diagnostic surface.
/// Lazy runs stay silent unless `LORE_DEBUG=1`; manual runs surface
/// errors to operator stderr per the CLI behaviour ladder's tier-2
/// contract for `lore trace prune`.
fn report_error(verbosity: Verbosity, args: std::fmt::Arguments<'_>) {
    match verbosity {
        Verbosity::Silent => {
            lore_debug!("trace maintenance: {}", args);
        }
        Verbosity::Verbose => {
            eprintln!("lore trace prune: {args}");
        }
    }
}

/// Gzip a file in-place to `<filename>.gz` and remove the source on
/// success. Skips files that are already gzipped (`.gz` suffix).
fn gzip_file(path: &Path) -> anyhow::Result<()> {
    if path.extension().is_some_and(|e| e == "gz") {
        return Ok(());
    }
    let mut source = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    source.read_to_end(&mut buf)?;
    drop(source);

    let mut target_path = path.to_path_buf();
    let new_name = match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => format!("{name}.gz"),
        None => return Err(anyhow::anyhow!("invalid trace file name")),
    };
    target_path.set_file_name(new_name);

    // Mirror the writer's 0o600 discipline on the gzipped successor — the
    // default umask would yield 0o644 (world-readable), silently undoing
    // the privacy posture established by `src/trace/writer.rs`.
    // `create_new(true)` ensures a pre-existing `.gz` is not silently
    // overwritten; the caller increments `summary.errors` on conflict.
    let target = open_gzip_target(&target_path)?;
    let mut encoder = flate2::write::GzEncoder::new(target, flate2::Compression::default());
    encoder.write_all(&buf)?;
    encoder.finish()?;

    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(unix)]
fn open_gzip_target(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(super::writer::TRACE_FILE_MODE)
        .open(path)
}

#[cfg(not(unix))]
fn open_gzip_target(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Convert a u32 day count to a `Duration` saturated at u64 seconds.
fn duration_from_days(days: u32) -> Duration {
    Duration::from_secs(u64::from(days) * 86_400)
}

fn read_last_pruned_at(path: &Path) -> Option<SystemTime> {
    let s = std::fs::read_to_string(path).ok()?;
    let secs: u64 = s.trim().parse().ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

fn write_last_pruned_at(path: &Path, t: SystemTime) -> anyhow::Result<()> {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::write(path, secs.to_string())?;
    Ok(())
}

/// Read the throttle timestamp via the in-memory shadow with a
/// disk-state fallback. Returns `None` when neither source has a
/// value (first run after process start with no prior on-disk state).
fn read_throttle_state(trace_dir: &Path, state_path: &Path) -> Option<SystemTime> {
    if let Ok(map) = IN_MEMORY_THROTTLE.lock()
        && let Some(t) = map.get(trace_dir).copied()
    {
        return Some(t);
    }
    read_last_pruned_at(state_path)
}

/// Record the throttle timestamp in both the in-memory shadow and on
/// disk. The in-memory update always happens; the disk write is
/// best-effort and degrades to a `LORE_DEBUG`-gated diagnostic on
/// failure. This pairing keeps the process-local throttle working even
/// when the state file is unwriteable (read-only mount, disk full, etc.)
/// so a hot loop of SessionStarts can't repeatedly walk the directory.
fn record_throttle_state(trace_dir: &Path, state_path: &Path, t: SystemTime) {
    if let Ok(mut map) = IN_MEMORY_THROTTLE.lock() {
        map.insert(trace_dir.to_path_buf(), t);
    }
    if let Err(e) = write_last_pruned_at(state_path, t) {
        lore_debug!(
            "trace maintenance: failed to persist throttle state ({e}); \
             in-memory shadow will throttle this process"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch_with_age(path: &Path, days_old: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "{}\n").unwrap();
        let new_mtime = SystemTime::now() - Duration::from_secs(days_old * 86_400);
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(new_mtime).unwrap();
    }

    #[test]
    fn run_lazy_no_trace_dir_returns_default_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().join("missing");
        let summary = run_lazy(&trace_dir, 30, 7);
        assert_eq!(summary, MaintenanceSummary::default());
    }

    #[test]
    fn run_lazy_throttle_skips_within_window() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().to_path_buf();
        let state = trace_dir.join(LAST_PRUNED_AT_FILE);
        write_last_pruned_at(&state, SystemTime::now()).unwrap();
        // A stale file the lazy pass would normally delete.
        let stale = trace_dir.join("s1.jsonl");
        touch_with_age(&stale, 60);

        let summary = run_lazy(&trace_dir, 30, 7);
        assert!(summary.skipped);
        assert!(stale.exists(), "throttled run must not delete files");
    }

    #[test]
    fn run_manual_prunes_files_older_than_retain_days() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().to_path_buf();
        let old = trace_dir.join("old.jsonl");
        let new = trace_dir.join("new.jsonl");
        touch_with_age(&old, 60);
        touch_with_age(&new, 1);

        let summary = run_manual(&trace_dir, 30, 0);
        assert!(
            summary.deleted >= 1,
            "old file should be deleted: {summary:?}"
        );
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn run_manual_gzips_files_older_than_gzip_horizon() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().to_path_buf();
        let old = trace_dir.join("aged.jsonl");
        touch_with_age(&old, 14);

        let summary = run_manual(&trace_dir, 0, 7);
        assert!(summary.compressed >= 1, "{summary:?}");
        assert!(trace_dir.join("aged.jsonl.gz").exists());
        assert!(!old.exists());
    }

    #[test]
    fn run_manual_bumps_last_pruned_at() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().to_path_buf();
        let _ = fs::create_dir_all(&trace_dir);
        let _ = run_manual(&trace_dir, 30, 7);
        assert!(trace_dir.join(LAST_PRUNED_AT_FILE).exists());
    }

    #[test]
    fn cap_of_100_honoured_on_lazy_run() {
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().to_path_buf();
        for i in 0..120 {
            touch_with_age(&trace_dir.join(format!("s{i:03}.jsonl")), 60);
        }
        // No state file → lazy runs immediately, but the cap bounds it.
        let summary = run_lazy(&trace_dir, 30, 0);
        assert_eq!(summary.deleted, MAX_PRUNE_PER_RUN);
    }

    #[cfg(unix)]
    #[test]
    fn maintenance_skips_symlinked_entries() {
        // An operator-placed symlink in the trace directory must not be
        // chased by the maintenance pass: gzipping the target and
        // deleting it after the retention horizon would consume files
        // outside the lore-managed state tier.
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().join("traces");
        fs::create_dir_all(&trace_dir).unwrap();

        // A regular old file that SHOULD be pruned.
        let real = trace_dir.join("real.jsonl");
        touch_with_age(&real, 60);

        // An external file that the operator symlinked into the dir.
        let external = tmp.path().join("external.jsonl");
        touch_with_age(&external, 60);
        let symlink = trace_dir.join("via-symlink.jsonl");
        std::os::unix::fs::symlink(&external, &symlink).unwrap();

        let summary = run_manual(&trace_dir, 30, 0);
        assert_eq!(summary.deleted, 1, "only the real file should be deleted");
        assert!(!real.exists(), "real file should be deleted");
        assert!(symlink.exists(), "symlink itself should be left alone");
        assert!(
            external.exists(),
            "symlink target must not be touched outside the trace dir"
        );
    }

    #[test]
    fn in_memory_throttle_engages_when_disk_state_is_absent() {
        // Simulate a host where the on-disk state file fails to land
        // (read-only mount, disk full, etc.) by deleting the file
        // between the first and second lazy run. The second invocation
        // must still throttle off the in-memory shadow rather than
        // re-running the full maintenance pass.
        let tmp = tempfile::tempdir().unwrap();
        let trace_dir = tmp.path().join("traces-in-memory");
        fs::create_dir_all(&trace_dir).unwrap();
        touch_with_age(&trace_dir.join("aged.jsonl"), 60);

        // First run lays down state on disk AND in memory.
        let summary_first = run_lazy(&trace_dir, 30, 0);
        assert_eq!(
            summary_first.deleted, 1,
            "first run should prune the aged file"
        );
        assert!(
            trace_dir.join(LAST_PRUNED_AT_FILE).exists(),
            "first run should write the state file"
        );

        // Simulate the disk-state-loss case: someone removed the state
        // file out-of-band. The in-memory shadow should still throttle.
        fs::remove_file(trace_dir.join(LAST_PRUNED_AT_FILE)).unwrap();
        // Lay down another aged file that would be pruned if the
        // throttle didn't engage.
        touch_with_age(&trace_dir.join("aged-again.jsonl"), 60);

        let summary_second = run_lazy(&trace_dir, 30, 0);
        assert!(
            summary_second.skipped,
            "in-memory shadow should keep the throttle engaged when disk state is missing"
        );
        assert!(
            trace_dir.join("aged-again.jsonl").exists(),
            "throttled run must not prune"
        );
    }
}
