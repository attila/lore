// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::doc_markdown, clippy::struct_excessive_bools)]

//! Trace-record types serialised one-per-line as JSONL.
//!
//! Each on-disk JSONL line is a [`TraceRecord`] tagged on `event` per the
//! canonical taxonomy (PreToolUse, PostToolUse, SessionStart, PostCompact).
//! Adapter-specific lifecycle names map onto these at the adapter boundary —
//! readers see a single shape regardless of which agent produced the trace.
//!
//! Schema versioning follows the silent-additive convention in
//! `docs/solutions/conventions/schema-migration-strategy-2026-05-14.md`:
//! readers tolerate unknown fields via `#[serde(default)]`; the integer is
//! bumped only when a reader genuinely cannot tolerate the older shape.

use serde::{Deserialize, Serialize};

/// Current JSONL record schema. Bumped only when a future reader cannot
/// tolerate the older shape.
pub const SCHEMA_VERSION: u32 = 1;

/// Canonical agent identifier for the Claude Code adapter. Future adapters
/// (Cursor, opencode, …) ride into the same trace directory under their own
/// agent string and are disambiguated by the `lore trace why --agent` filter.
pub const AGENT_CLAUDE_CODE: &str = "claude-code";

/// A single per-event trace record. Tagged on `event` so the canonical
/// taxonomy lives in the JSON shape, not in caller-side discrimination.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum TraceRecord {
    PreToolUse(PreToolUseRecord),
    PostToolUse(PostToolUseRecord),
    SessionStart(SessionStartRecord),
    PostCompact(PostCompactRecord),
}

impl TraceRecord {
    /// Session id associated with this record. Records without a session id
    /// are dropped at the writer layer; this accessor is for downstream
    /// filters and pretty-print.
    pub fn session_id(&self) -> &str {
        match self {
            Self::PreToolUse(r) => &r.session_id,
            Self::PostToolUse(r) => &r.session_id,
            Self::SessionStart(r) => &r.session_id,
            Self::PostCompact(r) => &r.session_id,
        }
    }

    /// Canonical event name as it appears in the JSON `event` tag.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::PreToolUse(_) => "PreToolUse",
            Self::PostToolUse(_) => "PostToolUse",
            Self::SessionStart(_) => "SessionStart",
            Self::PostCompact(_) => "PostCompact",
        }
    }

    /// Agent identifier captured at write time. Today every record carries
    /// `claude-code`; the field is forward-compat for future adapters.
    pub fn agent(&self) -> &str {
        match self {
            Self::PreToolUse(r) => &r.agent,
            Self::PostToolUse(r) => &r.agent,
            Self::SessionStart(r) => &r.agent,
            Self::PostCompact(r) => &r.agent,
        }
    }

    /// Tool name from the call context, when the record is bound to a tool
    /// (PreToolUse / PostToolUse). Returns `None` for SessionStart and
    /// PostCompact records, which have no tool context.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::PreToolUse(r) => Some(&r.call_context.tool_name),
            Self::PostToolUse(r) => Some(&r.call_context.tool_name),
            _ => None,
        }
    }
}

/// PreToolUse trace record — captured after dedup, before
/// `format_imperative` emits the injected context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolUseRecord {
    pub schema_version: u32,
    pub ts: String,
    pub session_id: String,
    pub agent: String,
    pub call_context: CallContextSnapshot,
    pub query: Option<String>,
    pub candidates: Vec<CandidateRecord>,
    pub injected: Vec<String>,
    pub config: ConfigSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaState>,
    pub duration_ms: u64,
    pub phases: Phases,
}

/// PostToolUse trace record — captured after the post-tool search runs (or
/// no-ops on success).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolUseRecord {
    pub schema_version: u32,
    pub ts: String,
    pub session_id: String,
    pub agent: String,
    pub call_context: CallContextSnapshot,
    pub query: Option<String>,
    pub candidates: Vec<CandidateRecord>,
    pub injected: Vec<String>,
    pub config: ConfigSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaState>,
    pub duration_ms: u64,
    pub phases: Phases,
}

/// SessionStart trace record — captured after `format_session_context`,
/// before the hook returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartRecord {
    pub schema_version: u32,
    pub ts: String,
    pub session_id: String,
    pub agent: String,
    /// Full configuration snapshot — written once per session.
    pub config: FullConfigSnapshot,
    pub duration_ms: u64,
}

/// PostCompact trace record — captured after the compact-handler's dedup
/// truncation but before return.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostCompactRecord {
    pub schema_version: u32,
    pub ts: String,
    pub session_id: String,
    pub agent: String,
    pub duration_ms: u64,
}

/// Snapshot of the [`crate::engine::CallContext`] captured at trace time.
/// Verbatim: sanitisation is a presentation concern in `lore trace why`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallContextSnapshot {
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_head: Option<String>,
    /// Full Bash command body — only populated when
    /// `[trace] include_full_command = true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_full: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inferred_languages: Vec<String>,
    /// Last user message tail — only populated when
    /// `[trace] include_transcript_tail = true`. Already bounded by the
    /// hook adapter's existing 32 KB `TRANSCRIPT_TAIL_BYTES` cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_tail: Option<String>,
}

/// One per-candidate row inside [`PreToolUseRecord::candidates`] etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub chunk_id: String,
    pub source_file: String,
    pub is_universal: bool,
    pub has_predicate: bool,
    pub has_language_declaration: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_fts_fallback: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_fts_structural: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_vector: Option<f64>,
    pub score_combined: f64,
    pub predicate_outcome: PredicateOutcome,
    pub above_threshold: bool,
    pub deduped: bool,
}

/// Per-record search and embedder configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub hybrid: bool,
    pub top_k: usize,
    pub min_relevance: f64,
    pub min_relevance_universal: f64,
    pub embedder_model: String,
}

/// Full configuration snapshot — written once per session at SessionStart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullConfigSnapshot {
    pub knowledge_dir: String,
    pub database: String,
    pub bind: String,
    pub ollama_host: String,
    pub ollama_model: String,
    pub search: ConfigSnapshot,
    pub chunking_strategy: String,
    pub chunking_max_tokens: usize,
    pub trace_enabled: bool,
    pub trace_retain_days: u32,
    pub trace_gzip_older_than_days: u32,
    pub trace_include_full_command: bool,
    pub trace_include_transcript_tail: bool,
}

/// Ollama embedding state — captured per-record when the search path
/// attempted to embed the query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaState {
    pub embedding_succeeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_dims: Option<usize>,
}

/// Per-phase wall-clock breakdown, populated as the pipeline progresses.
/// Fields are `None` when the phase did not execute for this record
/// (e.g. `embedding_ms` is `None` when the search took the FTS-only fallback
/// path).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Phases {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_extract_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_fts_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_vector_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate_filter_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_ms: Option<u64>,
}

/// Predicate evaluation outcome on a per-candidate basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredicateOutcome {
    /// Pattern has no `applies_when` declaration; the candidate is admitted
    /// by the predicate filter unconditionally.
    NoPredicate,
    /// Predicate evaluated truthy; the candidate is admitted.
    Matched,
    /// Predicate evaluated falsy; the candidate is dropped.
    Suppressed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pre_tool_use() -> TraceRecord {
        TraceRecord::PreToolUse(PreToolUseRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:23:01.234Z".to_string(),
            session_id: "abc-123".to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            call_context: CallContextSnapshot {
                tool_name: "Bash".to_string(),
                command_head: Some("git".to_string()),
                command_full: None,
                file_path: None,
                description: Some("push to remote".to_string()),
                inferred_languages: vec!["rust".to_string()],
                transcript_tail: None,
            },
            query: Some("rust AND (git OR push)".to_string()),
            candidates: vec![CandidateRecord {
                chunk_id: "chunk1".to_string(),
                source_file: "workflows/git.md".to_string(),
                is_universal: true,
                has_predicate: false,
                has_language_declaration: false,
                score_fts_fallback: Some(0.65),
                score_fts_structural: None,
                score_vector: Some(0.82),
                score_combined: 0.78,
                predicate_outcome: PredicateOutcome::NoPredicate,
                above_threshold: true,
                deduped: false,
            }],
            injected: vec!["chunk1".to_string()],
            config: ConfigSnapshot {
                hybrid: true,
                top_k: 5,
                min_relevance: 0.6,
                min_relevance_universal: 0.6,
                embedder_model: "nomic-embed-text".to_string(),
            },
            ollama: Some(OllamaState {
                embedding_succeeded: true,
                embedding_dims: Some(768),
            }),
            duration_ms: 47,
            phases: Phases {
                query_extract_ms: Some(1),
                search_fts_ms: Some(3),
                search_vector_ms: Some(28),
                embedding_ms: Some(12),
                predicate_filter_ms: Some(1),
                dedup_ms: Some(2),
            },
        })
    }

    #[test]
    fn pre_tool_use_round_trips() {
        let original = sample_pre_tool_use();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: TraceRecord = serde_json::from_str(&json).unwrap();
        let rejson = serde_json::to_string(&parsed).unwrap();
        assert_eq!(json, rejson);
    }

    #[test]
    fn event_tag_appears_in_json() {
        let json = serde_json::to_string(&sample_pre_tool_use()).unwrap();
        assert!(
            json.contains("\"event\":\"PreToolUse\""),
            "expected event tag in JSON, got: {json}"
        );
    }

    #[test]
    fn forward_compat_unknown_field_is_tolerated() {
        // Future schema may carry fields a v1 reader doesn't know about;
        // the reader must drop them rather than fail.
        let json = r#"{
            "event": "PostCompact",
            "schema_version": 1,
            "ts": "2026-05-15T14:23:01.234Z",
            "session_id": "abc-123",
            "agent": "claude-code",
            "duration_ms": 5,
            "future_field_added_in_v2": [1, 2, 3]
        }"#;
        let parsed: TraceRecord = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.event_name(), "PostCompact");
    }

    #[test]
    fn session_id_accessor_returns_per_variant_id() {
        assert_eq!(sample_pre_tool_use().session_id(), "abc-123");
    }

    #[test]
    fn agent_accessor_returns_per_variant_agent() {
        assert_eq!(sample_pre_tool_use().agent(), AGENT_CLAUDE_CODE);
    }

    #[test]
    fn tool_name_present_for_tool_events_absent_for_lifecycle_events() {
        assert_eq!(sample_pre_tool_use().tool_name(), Some("Bash"));
        let session_start = TraceRecord::SessionStart(SessionStartRecord {
            schema_version: SCHEMA_VERSION,
            ts: "2026-05-15T14:00:00.000Z".to_string(),
            session_id: "s1".to_string(),
            agent: AGENT_CLAUDE_CODE.to_string(),
            config: FullConfigSnapshot {
                knowledge_dir: "docs".to_string(),
                database: "lore.db".to_string(),
                bind: "localhost:3100".to_string(),
                ollama_host: "http://127.0.0.1:11434".to_string(),
                ollama_model: "nomic-embed-text".to_string(),
                search: ConfigSnapshot {
                    hybrid: true,
                    top_k: 5,
                    min_relevance: 0.6,
                    min_relevance_universal: 0.6,
                    embedder_model: "nomic-embed-text".to_string(),
                },
                chunking_strategy: "heading".to_string(),
                chunking_max_tokens: 1024,
                trace_enabled: true,
                trace_retain_days: 30,
                trace_gzip_older_than_days: 7,
                trace_include_full_command: false,
                trace_include_transcript_tail: false,
            },
            duration_ms: 3,
        });
        assert_eq!(session_start.tool_name(), None);
    }
}
