//! Agent-agnostic engine for the lore hook pipeline.
//!
//! The engine is the half of `lore` that takes a [`CallContext`] (a
//! pre-extracted, agent-neutral view of a tool call) and computes the
//! boolean and string outputs the adapter needs: predicate evaluation,
//! query extraction, language inference, and pure-string helpers. The
//! Claude Code adapter at `src/hook.rs` is responsible for translating
//! `HookInput` → `CallContext` (including the eager transcript-tail read
//! with `$HOME` validation); future adapters (Cursor, opencode, …) plug
//! in by writing their own translation and calling the same engine
//! functions unchanged.
//!
//! **Invariant**: no disk I/O in this module. The
//! `tests/invariants.rs::no_unsanctioned_runtime_disk_reads_in_hook_server_main`
//! static-grep keeps adapter-only filesystem access pinned to its
//! existing allow-list; the engine simply has no `std::fs::*` references
//! and never grows any.
//!
//! See `docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`
//! for the full engine/adapter split rationale and the Track 1 boundary
//! diagram.

pub mod call_context;
pub mod predicate;

pub use call_context::CallContext;
pub use predicate::{command_matches_with_wrappers, evaluate_applies_when};

// `AppliesWhen` is parsed by `chunking.rs` (where the frontmatter parser
// lives) and evaluated by `engine::predicate`. Re-export from the engine
// so future agent adapters depend only on the engine module — they never
// need to reach into `chunking` directly.
pub use crate::chunking::AppliesWhen;
