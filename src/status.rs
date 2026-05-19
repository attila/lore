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

    let mut entries: Vec<(String, usize)> = counts
        .declared
        .iter()
        .map(|c| (sanitise_for_line(display_name_for(&c.token)), c.count))
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut parts: Vec<String> = entries
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect();
    if counts.undeclared > 0 {
        parts.push(format!("undeclared {}", counts.undeclared));
    }
    Some(parts.join(", "))
}

/// Replace characters that would break the `Rust 5, TypeScript 5, ...`
/// single-line format — commas (the field separator), whitespace
/// (would visually merge with the `, ` separator or create new lines),
/// and ASCII control characters. Maps each offending character to `_`.
///
/// Known tokens in [`crate::engine::languages::LANGUAGES`] are vetted
/// at codegen time and pass through unchanged. The fallback path of
/// [`display_name_for`] returns the raw token, which is the only
/// realistic source of disallowed characters — and even there, the
/// ingest validator filters them out. This is the render-side
/// defence-in-depth backstop.
fn sanitise_for_line(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == ',' || c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
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
        // a newer pack than the binary covers) renders as-is. `matlab`
        // is the still-unknown canary now that `kotlin` is a known
        // token (MATLAB is the deferred `.m` contestation owner).
        let line = format_languages_line(&counts(&[("matlab", 2)], 0)).unwrap();
        assert_eq!(line, "matlab 2");
    }

    #[test]
    fn format_languages_line_sanitises_unknown_tokens_with_separator_chars() {
        // Defence-in-depth render-side: an unknown token containing
        // commas, whitespace, or control characters would corrupt the
        // single-line `, `-separated format. Sanitiser replaces them
        // with `_`. The ingest validator filters these out, so this
        // is only reachable via manual SQL edit or a future bug.
        let line = format_languages_line(&counts(&[("foo,bar", 1), ("baz\nqux", 1)], 0)).unwrap();
        // Ordering: alphabetical tiebreak on sanitised display name
        // (`_` is ASCII 0x5F, `b` is 0x62 — so `baz_qux` < `foo_bar`).
        assert_eq!(line, "baz_qux 1, foo_bar 1");
    }

    #[test]
    fn format_languages_line_leaves_known_display_names_unchanged() {
        // The sanitiser must not corrupt vetted display names. The
        // table now ships entries whose display names contain `+` and
        // `#` (`C++`, `C#`); both must pass through unchanged. ASCII
        // ordering on the leading character (`#` 0x23 < `+` 0x2B < `R`
        // 0x52 < `T` 0x54) drives the alphabetical tiebreak.
        let line = format_languages_line(&counts(
            &[("csharp", 1), ("cpp", 1), ("rust", 1), ("typescript", 1)],
            0,
        ))
        .unwrap();
        assert_eq!(line, "C# 1, C++ 1, Rust 1, TypeScript 1");
    }
}
