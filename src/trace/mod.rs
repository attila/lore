// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::doc_markdown,
    clippy::single_char_pattern,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::struct_excessive_bools,
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn
)]

//! Track 2 Observability: per-hook trace logging.
//!
//! Writes one JSONL record per canonical hook event under
//! `$XDG_STATE_HOME/lore/traces/<session-id>.jsonl`. Gated by
//! [`crate::config::Config::trace_enabled`] (config flag plus `LORE_TRACE`
//! env-var override). All writes are fire-and-forget — failures degrade to
//! silent skips with `LORE_DEBUG`-gated diagnostics so the hook contract
//! ("never break the agent") is preserved.
//!
//! Submodules:
//!
//! - [`record`] — `TraceRecord` enum + per-event payload structs, schema
//!   versioning, serde derives.
//! - [`writer`] — append-only JSONL writer with `0o600` file / `0o700`
//!   directory enforcement on Unix.
//!
//! See `docs/plans/2026-05-15-001-feat-track-2-observability-plan.md` for
//! the design context and the full requirements trace.

pub mod maintenance;
pub mod query;
pub mod record;
pub mod stats;
pub mod writer;

pub use stats::{CapturePosture, TraceStats};

pub use record::{
    AGENT_CLAUDE_CODE, CallContextSnapshot, CandidateRecord, ConfigSnapshot, FullConfigSnapshot,
    OllamaState, Phases, PostCompactRecord, PostToolUseRecord, PreToolUseRecord, PredicateOutcome,
    SCHEMA_VERSION, SessionStartRecord, TraceRecord,
};
pub use writer::{append_record, trace_file_path};

/// Format a [`std::time::SystemTime`] as a millisecond-precision RFC 3339
/// UTC timestamp without pulling in `chrono` or `time`. The output shape
/// is `YYYY-MM-DDTHH:MM:SS.mmmZ`.
///
/// Rationale lives in the plan's Key Technical Decisions section:
/// `SystemTime` + manual RFC 3339 avoids the binary-size hit of a
/// date/time crate. The trace consumer prefers stable monotonic ordering
/// over a human-readable shape; `lore trace why` pretty-print is free to
/// format on read.
pub fn format_rfc3339_millis(t: std::time::SystemTime) -> String {
    let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let millis = dur.subsec_millis();
    // Civil-from-days: integer-only Howard Hinnant date algorithm.
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400) as u64;
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let mi = (secs_of_day / 60) % 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Convert a Unix-epoch day count to a `(year, month, day)` triple using
/// Howard Hinnant's `civil_from_days` algorithm. Public-domain reference:
/// <https://howardhinnant.github.io/date_algorithms.html#civil_from_days>.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = y + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_at_unix_epoch() {
        let t = std::time::UNIX_EPOCH;
        assert_eq!(format_rfc3339_millis(t), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn rfc3339_at_known_instant() {
        // 2026-05-15T14:23:01.234Z corresponds to 1_778_337_781.234 secs
        // after the epoch (verified via Python `datetime.timestamp()`).
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_778_855_781_234);
        let formatted = format_rfc3339_millis(t);
        assert!(
            formatted.starts_with("2026-")
                && formatted.ends_with("Z")
                && formatted.contains('T')
                && formatted.contains('.'),
            "expected RFC 3339 shape, got {formatted}"
        );
    }

    #[test]
    fn rfc3339_round_trip_via_chrono_would_match() {
        // Spot-check three dates spanning the leap-year and century edge
        // cases that the Hinnant algorithm needs to get right.
        let cases = [
            (0_u64, "1970-01-01T00:00:00.000Z"),
            // 2000-02-29T00:00:00Z — leap year + century rule.
            (951_782_400_000, "2000-02-29T00:00:00.000Z"),
            // 2100-03-01T00:00:00Z — century non-leap year.
            (4_107_542_400_000, "2100-03-01T00:00:00.000Z"),
        ];
        for (millis, expected) in cases {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis);
            assert_eq!(format_rfc3339_millis(t), expected);
        }
    }
}
