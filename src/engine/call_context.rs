//! Agent-agnostic call context for the engine module.
//!
//! [`CallContext`] is the input contract every engine function speaks. It
//! carries the pre-extracted strings an adapter (Claude Code's `src/hook.rs`,
//! a future Cursor adapter, ...) hands the engine: the tool name, the Bash
//! command (when applicable), the file path / description (for non-Bash
//! tools), and the eagerly-read transcript tail. All fields are owned
//! `String`s — one allocation per field, called once per `PreToolUse`, so
//! borrow lifetimes would buy nothing here. Simple beats clever.
//!
//! The engine never reads disk; the adapter is responsible for populating
//! `transcript_tail` (with `$HOME` validation and the existing 32 KB cap)
//! before invoking any engine function. This keeps the
//! `tests/invariants.rs` "no fs reads outside the allow-list" guard
//! satisfied for the engine module.
//!
//! See `docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`
//! unit U3 for the engine/adapter split rationale.

/// Pre-extracted call context the engine evaluates predicates against.
///
/// Every field is `Option<String>` so adapters can populate only what their
/// agent harness exposes. The Claude Code adapter (U5) will populate all
/// five from a single `HookInput` event.
#[derive(Debug, Clone)]
pub struct CallContext {
    /// Tool name as the adapter reports it (case-sensitive, matched against
    /// the predicate's allowlist verbatim — e.g. `"Bash"`, `"Edit"`,
    /// `"Read"`).
    pub tool_name: Option<String>,
    /// Raw Bash command string (`tool_input.command` in Claude Code's hook
    /// JSON). Only set when `tool_name == "Bash"`. The smart-prefix matcher
    /// operates on this string verbatim — no FTS-cleaning, no
    /// term-extraction.
    pub command: Option<String>,
    /// File path argument for tools that take one (Edit, Read, Write, ...).
    pub file_path: Option<String>,
    /// Free-form description argument (`Task`, `TodoWrite`, ...).
    pub description: Option<String>,
    /// Trailing snippet of the user's transcript, read eagerly by the
    /// adapter and capped at 32 KB. The engine treats this as opaque text
    /// — no I/O happens here.
    pub transcript_tail: Option<String>,
}

impl CallContext {
    /// All-`None` constructor for tests and adapters that want to fill
    /// fields one at a time. The engine treats this as "no information",
    /// which never fails the predicate evaluator (the evaluator returns
    /// `true` when the predicate has no set keys; non-predicate-relevant
    /// fields are simply ignored).
    pub fn empty() -> Self {
        Self {
            tool_name: None,
            command: None,
            file_path: None,
            description: None,
            transcript_tail: None,
        }
    }
}
