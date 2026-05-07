//! Pure-string text utilities used by query extraction.
//!
//! Two helpers used across the engine:
//!
//! * [`split_into_words`] — split on non-alphabetic characters, lowercase,
//!   drop empty fragments. Used by both `extract_query` (description /
//!   command / transcript-tail term harvesting) and `handle_post_tool_use`
//!   (Bash stderr cleanup).
//! * [`truncate_str`] — byte-length cap with UTF-8 char-boundary safety.
//!   Used by `extract_query` to bound the transcript-tail snippet at 200
//!   bytes before splitting it into terms.
//!
//! Both functions are total: any `&str` in, deterministic output, no I/O.

/// Split a string on non-alphabetic boundaries and lowercase each fragment.
///
/// Empty fragments are dropped. Used to harvest candidate terms from
/// free-form text (Bash descriptions, commands, transcript tails, stderr).
pub fn split_into_words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Truncate a string to at most `max_bytes` bytes, snapping to a valid
/// UTF-8 character boundary.
///
/// When `s.len() <= max_bytes`, returns `s` unchanged. Otherwise walks
/// `max_bytes` down until the byte index lands on a char boundary, then
/// slices there. Multi-byte characters at the cap are dropped rather than
/// split.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the largest byte offset that is both <= max_bytes and a valid
    // char boundary.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- split_into_words ----------------------------------------------------

    #[test]
    fn split_into_words_simple_sentence() {
        let words = split_into_words("Run cargo test for error handling");
        assert_eq!(
            words,
            vec!["run", "cargo", "test", "for", "error", "handling"]
        );
    }

    #[test]
    fn split_into_words_drops_punctuation_and_digits() {
        // Non-alphabetic characters are treated as word separators.
        let words = split_into_words("foo-bar_baz, 123 quux");
        assert_eq!(words, vec!["foo", "bar", "baz", "quux"]);
    }

    #[test]
    fn split_into_words_empty_string() {
        let words = split_into_words("");
        assert!(words.is_empty());
    }

    #[test]
    fn split_into_words_only_separators() {
        let words = split_into_words("---///123");
        assert!(words.is_empty());
    }

    #[test]
    fn split_into_words_lowercases() {
        let words = split_into_words("Hello WORLD MixedCase");
        assert_eq!(words, vec!["hello", "world", "mixedcase"]);
    }

    // -- truncate_str --------------------------------------------------------

    #[test]
    fn truncate_str_no_op_when_within_cap() {
        assert_eq!(truncate_str("abc", 10), "abc");
    }

    #[test]
    fn truncate_str_caps_at_byte_boundary() {
        assert_eq!(truncate_str("abcdef", 3), "abc");
    }

    #[test]
    fn truncate_str_snaps_to_char_boundary() {
        // "é" is two bytes in UTF-8 — capping at byte 2 lands mid-char,
        // so the helper must walk back to byte 1 (just `a`).
        let s = "aé";
        // Sanity: byte length is 3.
        assert_eq!(s.len(), 3);
        assert_eq!(truncate_str(s, 2), "a");
    }

    #[test]
    fn truncate_str_zero_cap_returns_empty() {
        assert_eq!(truncate_str("hello", 0), "");
    }

    #[test]
    fn truncate_str_exact_cap_returns_full_input() {
        assert_eq!(truncate_str("abc", 3), "abc");
    }
}
