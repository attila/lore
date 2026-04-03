// SPDX-License-Identifier: MIT OR Apache-2.0

//! Conditional debug logging gated by the `LORE_DEBUG` environment variable.
//!
//! Set `LORE_DEBUG=1` (or `true`, `yes`) to enable verbose diagnostics on
//! stderr.  All output is prefixed with `[lore debug]` so it can be grepped
//! in noisy terminal sessions.

use std::sync::LazyLock;

static IS_DEBUG: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("LORE_DEBUG")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
});

/// Returns `true` when `LORE_DEBUG` is set to a truthy value.
///
/// The environment variable is read once on first call and cached for the
/// lifetime of the process.
pub fn is_debug() -> bool {
    *IS_DEBUG
}

/// Emit a debug line to stderr when `LORE_DEBUG` is enabled.
///
/// Usage mirrors `eprintln!`:
///
/// ```ignore
/// lore_debug!("query extracted: {query}");
/// lore_debug!("results: {count} hits above threshold {thresh:.4}");
/// ```
#[macro_export]
macro_rules! lore_debug {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug() {
            eprintln!("[lore debug] {}", format_args!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: is_debug() uses a process-wide LazyLock, so we cannot meaningfully
    // test different env var states within the same test binary.  These tests
    // verify the parsing logic indirectly and ensure the macro compiles.

    #[test]
    fn is_debug_returns_bool() {
        // In CI / normal test runs LORE_DEBUG is unset, so this should be false.
        // The important thing is that it doesn't panic.
        let _ = is_debug();
    }

    #[test]
    fn lore_debug_macro_compiles() {
        // Verify the macro expands without errors for various arg patterns.
        lore_debug!("plain message");
        lore_debug!("formatted: {}", 42);
        lore_debug!("multi: {} / {:.4}", "hello", 1.23);
    }
}
