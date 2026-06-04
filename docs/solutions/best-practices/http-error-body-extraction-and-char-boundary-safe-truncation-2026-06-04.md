---
title: "HTTP error body extraction and char-boundary-safe truncation"
date: 2026-06-04
category: best-practices
module: http-clients
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Surfacing an HTTP error body to humans in CLI output, log lines, or UI strings"
  - "Truncating an arbitrary-length string to a display budget"
  - "Working with response bodies that may be JSON `{\"error\": \"...\"}` shapes, plain text, multi-line, or empty"
  - "Writing a Rust utility that slices `&str` by character count rather than byte count"
tags:
  - rust
  - http
  - error-rendering
  - utf-8
  - truncation
  - char-boundary
  - body-extraction
---

# HTTP error body extraction and char-boundary-safe truncation

## Context

PR #67 needed to render arbitrary HTTP error bodies on a single CLI line. Ollama's broken Homebrew
bottle returns 5xx responses with bodies like:

```json
{
  "error": "error starting llama-server: llama-server binary not found (checked: /opt/homebrew/Cellar/ollama/0.30.4/libexec/lib/ollama/llama-server, /opt/homebrew/Cellar/ollama/0.30.4/libexec/llama-server, ...)"
}
```

Other failure modes produce different shapes — plain text, multi-line stack traces, HTML error pages
from misconfigured proxies, empty bodies, non-UTF-8 byte sequences. Each one needs to render
usefully on a status line that has a budget of roughly one terminal width.

Three failure modes specifically need defending against:

- **Byte-slice panic on truncation.** `&body[..80]` panics when byte index 80 falls in the middle of
  a multi-byte UTF-8 codepoint. A body containing CJK characters, emoji, or even an unlucky stray
  non-ASCII glyph will trip this.
- **JSON envelope leakage.** A naive "take the first 80 chars" approach prints
  `{"error":"the actual diagnostic` and chops off at 80, leaking the JSON wrapper into the
  user-facing message instead of extracting just the error content.
- **Empty result on lossy decode.** `read_to_string().unwrap_or_default()` collapses a non-UTF-8
  body to the empty string, swallowing the diagnostic entirely.

The pattern that landed handles all three.

## Guidance

Layer the body extraction:

1. **Read the body as bytes, decode lossily.** `read_to_vec()` followed by `String::from_utf8_lossy`
   preserves whatever is decodable and replaces invalid bytes with the U+FFFD replacement character.
   Never collapses to empty on a partial-UTF-8 byte sequence.

   ```rust
   let body = resp
       .body_mut()
       .read_to_vec()
       .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
       .unwrap_or_default();
   ```

2. **Try JSON `error` field first.** Most JSON APIs put the human-actionable message in
   `{"error": "..."}` or a similar shape. Attempting the JSON parse with a strict
   `{ error: String }` struct lets the common case extract just the error string.

   ```rust
   #[derive(Deserialize)]
   struct ErrorBody { error: String }

   if let Ok(parsed) = serde_json::from_str::<ErrorBody>(trimmed)
       && !parsed.error.trim().is_empty()
   {
       return Some(clean_and_truncate(&parsed.error));
   }
   ```

3. **Fall back to first non-empty line for non-JSON.** Take the first line that contains a
   non-whitespace character. Multi-line bodies (stack traces, HTML pages) collapse to their most
   informative leading line.

   ```rust
   let line = trimmed.lines().find(|l| !l.trim().is_empty())?;
   ```

4. **Strip control characters.** Replace anything with `c.is_control()` (tabs, bells, raw newlines
   that snuck through, etc.) with a space. Prevents terminal misbehaviour and keeps the line
   readable.

   ```rust
   let cleaned: String = line.chars()
       .map(|c| if c.is_control() { ' ' } else { c })
       .collect();
   ```

5. **Truncate by character count, not byte count.** Use `chars().take(max)` instead of byte slicing.
   The byte-slice approach panics on multi-byte boundaries; the char approach is safe by
   construction.

   ```rust
   fn truncate_chars(s: &str, max: usize) -> String {
       if s.chars().count() <= max {
           s.to_string()
       } else {
           let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
           format!("{truncated}…")
       }
   }
   ```

6. **Pick a generous-but-bounded truncation budget.** 80 chars is too tight for paths and
   stack-trace snippets; pathological multi-paragraph bodies still need a cap. Around 240 chars fits
   Homebrew Cellar paths on typical terminals while bounding the worst case.

The combined helper returns `Option<String>`: `None` if the body is empty after trimming, so the
caller can fall back to a variant-specific string without an empty dash-body suffix.

## Why This Matters

Each layer addresses a real failure mode caught during manual verification or unit tests:

- **The lossy-decode layer** turns a non-UTF-8 5xx body from a silent diagnostic loss into a
  best-effort recovery. Ollama emits UTF-8 in practice, but a misbehaving corporate proxy or
  TLS-intercepting middleware might not, and the diagnostic should survive the round trip.
- **The JSON-envelope layer** is the single highest-value rendering step. Most modern APIs use the
  JSON `{"error": "..."}` shape; extracting just the error field is the difference between the user
  seeing `inference failed — runner binary not found` and seeing
  `inference failed — {"error":"runner binary not found...`. The latter is a leaky abstraction.
- **The first-line-and-control-strip layer** is the fallback for everything else: plain-text bodies,
  HTML proxy error pages, stack traces. The first line is usually the most actionable;
  control-stripping keeps the terminal usable.
- **The char-count truncation layer** is the safety net. `&str` slicing by byte index panics on
  non-ASCII; `chars().take(n)` is safe for any UTF-8. The cost is constant per character (O(n)
  total) instead of O(1), but the safety dominates the cost for display-budget truncation.

The `…` ellipsis is a single Unicode codepoint (3 bytes in UTF-8) chosen over three ASCII dots
because it's a single character — the truncation arithmetic stays clean.

## When to Apply

Apply this pattern when:

- A function takes an arbitrary HTTP response body and produces a one-line user-facing message
- Bodies may be JSON, plain text, multi-line, empty, or non-UTF-8
- The display surface has a width budget (terminal line, log line, JSON metadata field)
- Truncation is required and the caller can't pre-validate the body's character set

Skip when:

- The body is known to be JSON with a fixed schema — parse it directly and render specific fields
- The display surface has unlimited width (a full log file, a multi-line panel)
- The body is known to be ASCII (a generated diagnostic from your own code) — byte slicing is fine
  in that constrained case, though `chars().take()` is still safer with zero downside

The `truncate_chars` helper specifically (saturating-sub on the budget for the ellipsis) avoids a
subtle off-by-one panic when `max == 0`. Callers should still pass realistic budgets, but the
defensive arithmetic costs nothing.

## Examples

### Full extraction with all four layers

```rust
fn extract_body_message(body: &str) -> Option<String> {
    const MAX_BODY_CHARS: usize = 240;

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Layer 1: JSON {"error": "..."}
    if let Ok(parsed) = serde_json::from_str::<ErrorBody>(trimmed) {
        let cleaned = clean_for_one_line(&parsed.error);
        if !cleaned.is_empty() {
            return Some(truncate_chars(&cleaned, MAX_BODY_CHARS));
        }
    }

    // Layer 2: first non-empty line
    let line = trimmed.lines().find(|l| !l.trim().is_empty())?;
    let cleaned = clean_for_one_line(line);
    if cleaned.is_empty() {
        None
    } else {
        Some(truncate_chars(&cleaned, MAX_BODY_CHARS))
    }
}

fn clean_for_one_line(s: &str) -> String {
    let replaced: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    replaced.trim().to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}
```

### Tests that lock in the safety properties

```rust
// Char-boundary safety on multi-byte input — would panic with byte slicing.
#[test]
fn truncation_handles_multibyte_chars() {
    let body = "あ".repeat(300);
    let msg = extract_body_message(&body).unwrap();
    assert!(msg.chars().count() <= 240);
    // No panic. Byte length is ~720 (3 bytes per char); char count is bounded.
}

// JSON envelope unwrapping — extracts just the error field.
#[test]
fn extracts_json_error_field() {
    let body = r#"{"error":"llama runner process has terminated"}"#;
    assert_eq!(
        extract_body_message(body).as_deref(),
        Some("llama runner process has terminated"),
    );
}

// Control-char replacement keeps the terminal usable.
#[test]
fn replaces_control_chars_with_spaces() {
    let body = "before\x07after";
    assert_eq!(extract_body_message(body).as_deref(), Some("before after"));
}

// Empty body returns None so the caller can render a variant-specific string.
#[test]
fn returns_none_for_empty_input() {
    assert_eq!(extract_body_message(""), None);
    assert_eq!(extract_body_message("   \n\t"), None);
}
```

The multi-byte test in particular is the one that would have failed with a naive `&body[..80]`
truncation; pin it now so a future refactor that switches to byte slicing surfaces immediately.

## Related

- [`ureq-3-http-status-as-error-defaults-true-2026-06-04.md`](ureq-3-http-status-as-error-defaults-true-2026-06-04.md)
  — companion to this doc. The ureq learning is about getting the body in the first place; this
  learning is about turning that body into a human-readable line.
- [`multi-surface-render-function-pattern-2026-06-04.md`](../design-patterns/multi-surface-render-function-pattern-2026-06-04.md)
  — `extract_body_message` is consumed by `render_failure` and feeds multiple surfaces. The
  centralisation pattern means body extraction logic lives in one place rather than per surface.
- [Rust documentation — `str::chars`](https://doc.rust-lang.org/std/primitive.str.html#method.chars)
  — the iterator that makes char-boundary-safe truncation possible.
- `src/embeddings.rs` in lore — `extract_body_message`, `clean_for_one_line`, `truncate_chars` are
  the reference implementations; unit tests in `mod tests` pin the safety properties.
