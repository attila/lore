//! Rendering helpers for `lore status` output.
//!
//! Pure formatting logic that turns a [`LanguageCounts`] aggregate into
//! the value portion of the `Languages:` status line. Lives outside the
//! CLI entry point so library consumers — the MCP `lore_status` tool
//! handler, a future `lore list --by-lang` consumer, snapshot tests —
//! can render the same string without duplicating the sort policy.

use crate::database::LanguageCounts;
use crate::engine::languages::display_name_for;

/// Render the value portion of the `Languages:` status line, or `None`
/// when the line should be suppressed (no declared languages and no
/// undeclared sources — empty database).
///
/// Declared entries sort by count descending with an alphabetical
/// tiebreak on the rendered display name (per R4 — operators see the
/// display name, not the token, so ties must order against what they
/// read). The `undeclared` bucket, when non-zero, always renders last.
pub fn format_languages_line(counts: &LanguageCounts) -> Option<String> {
    if counts.declared.is_empty() && counts.undeclared == 0 {
        return None;
    }

    let mut entries: Vec<(&str, usize)> = counts
        .declared
        .iter()
        .map(|c| (display_name_for(&c.token), c.count))
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let mut parts: Vec<String> = entries
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect();
    if counts.undeclared > 0 {
        parts.push(format!("undeclared {}", counts.undeclared));
    }
    Some(parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::LanguageCount;

    fn counts(declared: &[(&str, usize)], undeclared: usize) -> LanguageCounts {
        LanguageCounts {
            declared: declared
                .iter()
                .map(|(t, n)| LanguageCount {
                    token: (*t).to_string(),
                    count: *n,
                })
                .collect(),
            undeclared,
        }
    }

    #[test]
    fn format_languages_line_returns_none_when_no_sources() {
        assert!(format_languages_line(&counts(&[], 0)).is_none());
    }

    #[test]
    fn format_languages_line_all_declared_no_undeclared() {
        let line = format_languages_line(&counts(&[("rust", 12), ("yaml", 3)], 0)).unwrap();
        assert_eq!(line, "Rust 12, YAML 3");
    }

    #[test]
    fn format_languages_line_all_undeclared_no_declared() {
        let line = format_languages_line(&counts(&[], 5)).unwrap();
        assert_eq!(line, "undeclared 5");
    }

    #[test]
    fn format_languages_line_mixed_sorts_count_desc_undeclared_last() {
        let line =
            format_languages_line(&counts(&[("yaml", 3), ("rust", 12), ("typescript", 5)], 5))
                .unwrap();
        assert_eq!(line, "Rust 12, TypeScript 5, YAML 3, undeclared 5");
    }

    #[test]
    fn format_languages_line_alphabetical_tiebreak_on_display_name() {
        // Equal counts: alphabetical tiebreak applies to the rendered
        // display name (`Rust` < `TypeScript`), not the raw token.
        let line = format_languages_line(&counts(&[("typescript", 5), ("rust", 5)], 0)).unwrap();
        assert_eq!(line, "Rust 5, TypeScript 5");
    }

    #[test]
    fn format_languages_line_resolves_display_names() {
        // `golang` -> `Go` is the only token whose display name differs
        // beyond casing, so it's the canary for display-name resolution.
        let line = format_languages_line(&counts(&[("golang", 4)], 0)).unwrap();
        assert_eq!(line, "Go 4");
    }

    #[test]
    fn format_languages_line_unknown_token_falls_back_to_raw_token() {
        // A token not in LANGUAGES (e.g. a knowledge base ingested with
        // a newer pack than the binary covers) renders as-is.
        let line = format_languages_line(&counts(&[("kotlin", 2)], 0)).unwrap();
        assert_eq!(line, "kotlin 2");
    }
}
