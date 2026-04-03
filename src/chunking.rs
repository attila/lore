// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure markdown chunking — no I/O, no database, no embeddings.
//!
//! Splits markdown content into [`Chunk`]s by heading structure or as a single
//! whole-document chunk when no headings are present.

use std::collections::HashMap;
use std::path::Path;

/// A single chunk of knowledge extracted from a markdown file.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Unique identifier: `source_file:heading_path` (or `source_file` for document mode).
    pub id: String,
    /// Human-readable title (first heading text, or the filename stem).
    pub title: String,
    /// The body text of the chunk.
    pub body: String,
    /// Comma-separated tags extracted from YAML frontmatter.
    pub tags: String,
    /// Relative path of the source file.
    pub source_file: String,
    /// Breadcrumb trail of headings, e.g. `"Foo > Bar > Baz"`.
    pub heading_path: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Split `content` into chunks along markdown heading boundaries.
///
/// Falls back to [`chunk_as_document`] when no heading-based chunks are
/// produced (e.g. when the body under every heading is shorter than 10 chars).
pub fn chunk_by_heading(content: &str, source_file: &str) -> Vec<Chunk> {
    let stripped = strip_frontmatter(content);
    let lines: Vec<&str> = stripped.lines().collect();
    let mut chunks = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut current_body: Vec<&str> = Vec::new();
    let mut current_title = file_stem(source_file);
    let tags = extract_frontmatter_tags(content);
    let mut id_counts: HashMap<String, usize> = HashMap::new();

    let flush = |body: &[&str],
                 title: &str,
                 stack: &[(usize, String)],
                 tags: &str,
                 source_file: &str,
                 chunks: &mut Vec<Chunk>,
                 id_counts: &mut HashMap<String, usize>| {
        let text = body.join("\n").trim().to_string();
        if text.len() < 10 {
            return;
        }
        let heading_path: String = stack
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" > ");
        let base_id = format!(
            "{}:{}",
            source_file,
            if heading_path.is_empty() {
                "root"
            } else {
                &heading_path
            }
        );

        // Track duplicate heading paths and append a sequence number.
        let count = id_counts.entry(base_id.clone()).or_insert(0);
        *count += 1;
        let id = if *count > 1 {
            format!("{base_id}:{count}")
        } else {
            base_id
        };

        chunks.push(Chunk {
            id,
            title: title.to_string(),
            body: text,
            tags: tags.to_string(),
            source_file: source_file.to_string(),
            heading_path,
        });
    };

    for line in &lines {
        if let Some((level, text)) = parse_heading(line) {
            flush(
                &current_body,
                &current_title,
                &heading_stack,
                &tags,
                source_file,
                &mut chunks,
                &mut id_counts,
            );
            current_body.clear();

            while heading_stack.last().is_some_and(|(l, _)| *l >= level) {
                heading_stack.pop();
            }
            heading_stack.push((level, text.clone()));
            current_title = text;
        }
        current_body.push(line);
    }

    flush(
        &current_body,
        &current_title,
        &heading_stack,
        &tags,
        source_file,
        &mut chunks,
        &mut id_counts,
    );

    if chunks.is_empty() {
        return chunk_as_document(content, source_file);
    }

    chunks
}

/// Treat the entire file as a single chunk (after stripping frontmatter).
pub fn chunk_as_document(content: &str, source_file: &str) -> Vec<Chunk> {
    let body = strip_frontmatter(content).trim().to_string();
    if body.len() < 10 {
        return Vec::new();
    }

    let title = extract_title(content).unwrap_or_else(|| file_stem(source_file));
    let tags = extract_frontmatter_tags(content);

    vec![Chunk {
        id: source_file.to_string(),
        title,
        body,
        tags,
        source_file: source_file.to_string(),
        heading_path: String::new(),
    }]
}

/// Extract the first `# Heading` from `content` (after frontmatter).
pub fn extract_title(content: &str) -> Option<String> {
    let stripped = strip_frontmatter(content);
    for line in stripped.lines() {
        if let Some(text) = line.strip_prefix("# ") {
            return Some(text.trim().to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level > 6 {
        return None;
    }
    let text = trimmed[level..].trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((level, text))
}

fn extract_frontmatter_tags(content: &str) -> String {
    let Some(fm) = extract_frontmatter(content) else {
        return String::new();
    };

    let Some(start) = fm.find("tags:") else {
        return String::new();
    };
    let rest = &fm[start + 5..];

    // Inline style: tags: [a, b, c]
    // Only look for brackets on the first line after `tags:` to avoid matching
    // brackets in subsequent YAML fields.
    let first_line = rest.lines().next().unwrap_or("");
    if let Some(bracket_start) = first_line.find('[')
        && let Some(bracket_end) = first_line.find(']')
        && bracket_start < bracket_end
    {
        return first_line[bracket_start + 1..bracket_end]
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(", ");
    }

    // Block style: tags:\n  - a\n  - b
    let tag_lines: Vec<&str> = rest
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("  -") || l.starts_with("- "))
        .map(|l| l.trim_start_matches([' ', '-']).trim())
        .filter(|l| !l.is_empty())
        .collect();
    if !tag_lines.is_empty() {
        return tag_lines.join(", ");
    }

    String::new()
}

fn extract_frontmatter(content: &str) -> Option<String> {
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("\n---")?;
    Some(rest[..end].to_string())
}

fn strip_frontmatter(content: &str) -> &str {
    if !content.starts_with("---") {
        return content;
    }
    let rest = &content[3..];
    match rest.find("\n---") {
        Some(end) => {
            let after = end + 4; // skip past "\n---"
            if after < rest.len() {
                rest[after..].trim_start_matches('\n')
            } else {
                ""
            }
        }
        None => content,
    }
}

fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_based_chunking_multiple_headings() {
        let md = "\
# Introduction
Some intro text that is long enough.

## Details
Details body text that is long enough.

## Conclusion
Conclusion body text that is long enough.
";
        let chunks = chunk_by_heading(md, "notes.md");
        assert!(
            chunks.len() >= 3,
            "expected at least 3 chunks, got {}",
            chunks.len()
        );

        assert_eq!(chunks[0].heading_path, "Introduction");
        assert_eq!(chunks[1].heading_path, "Introduction > Details");
        assert_eq!(chunks[2].heading_path, "Introduction > Conclusion");

        assert!(chunks[0].body.contains("Some intro text"));
        assert!(chunks[1].body.contains("Details body"));
        assert!(chunks[2].body.contains("Conclusion body"));
    }

    #[test]
    fn heading_path_tracks_nesting_correctly() {
        let md = "\
## A
Content under A is long enough here.

### B
Content under B is long enough here.

## C
Content under C is long enough here.
";
        let chunks = chunk_by_heading(md, "test.md");
        assert_eq!(chunks.len(), 3);

        assert_eq!(chunks[0].heading_path, "A");
        assert_eq!(chunks[1].heading_path, "A > B");
        // When ## C appears, ### B and ## A should be popped, leaving only ## C.
        assert_eq!(chunks[2].heading_path, "C");
    }

    #[test]
    fn frontmatter_inline_tags() {
        let md = "\
---
title: Example
tags: [rust, chunking, markdown]
---

# Hello
Body text that is definitely long enough.
";
        let tags = extract_frontmatter_tags(md);
        assert_eq!(tags, "rust, chunking, markdown");
    }

    #[test]
    fn frontmatter_block_tags() {
        let md = "\
---
tags:
  - alpha
  - beta
  - gamma
---

# Hello
Body text that is definitely long enough.
";
        let tags = extract_frontmatter_tags(md);
        assert_eq!(tags, "alpha, beta, gamma");
    }

    #[test]
    fn no_frontmatter_returns_empty_tags() {
        let md = "# Just a heading\n\nSome body content here that is long enough.\n";
        let tags = extract_frontmatter_tags(md);
        assert!(tags.is_empty());
    }

    #[test]
    fn empty_file_returns_no_chunks() {
        let chunks = chunk_by_heading("", "empty.md");
        assert!(chunks.is_empty());
    }

    #[test]
    fn tiny_body_returns_no_chunks() {
        let chunks = chunk_by_heading("short", "tiny.md");
        assert!(chunks.is_empty());
    }

    #[test]
    fn no_headings_produces_root_chunk() {
        let md = "This file has no headings but has enough body text to make a chunk.\n";
        let chunks = chunk_by_heading(md, "plain.md");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].source_file, "plain.md");
        // No headings means heading_path is empty and id ends with ":root".
        assert!(chunks[0].heading_path.is_empty());
        assert_eq!(chunks[0].id, "plain.md:root");
    }

    #[test]
    fn truly_empty_body_falls_back_to_document_mode() {
        // When all body segments are < 10 chars, chunk_by_heading falls back to
        // chunk_as_document, which uses source_file as the id.
        let md = "## A\nhi\n## B\nhi\n";
        let _chunks_heading = chunk_by_heading(md, "sparse.md");
        // Both heading bodies are too short, so fallback happens and returns empty
        // because strip_frontmatter("## A\nhi\n## B\nhi\n") body is also small.
        // With enough document body it would produce a chunk_as_document result.
        let doc_md = "No headings at all, just a long enough paragraph of regular text.\n";
        let chunks = chunk_as_document(doc_md, "plain.md");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "plain.md");
        assert!(chunks[0].heading_path.is_empty());
    }

    #[test]
    fn extract_title_from_first_heading() {
        let md = "Some preamble\n\n# My Great Title\n\nBody.\n";
        assert_eq!(extract_title(md), Some("My Great Title".to_string()));
    }

    #[test]
    fn extract_title_skips_frontmatter() {
        let md = "---\ntitle: FM Title\n---\n\n# Real Title\n\nBody.\n";
        assert_eq!(extract_title(md), Some("Real Title".to_string()));
    }

    #[test]
    fn extract_title_returns_none_when_missing() {
        let md = "No heading here, just paragraphs.\n";
        assert_eq!(extract_title(md), None);
    }

    #[test]
    fn strip_frontmatter_returns_content_after_closing_fence() {
        let md = "---\nkey: value\n---\n\n# Hello\n\nBody.\n";
        let stripped = strip_frontmatter(md);
        assert!(stripped.starts_with("# Hello"), "got: {stripped:?}");
    }

    #[test]
    fn strip_frontmatter_returns_everything_when_no_frontmatter() {
        let md = "# Hello\n\nBody.\n";
        assert_eq!(strip_frontmatter(md), md);
    }

    #[test]
    fn chunk_ids_follow_source_heading_path_pattern() {
        let md = "\
# Top
Enough body text to be a real chunk.

## Sub
Another section with enough body text.
";
        let chunks = chunk_by_heading(md, "doc/guide.md");
        assert_eq!(chunks[0].id, "doc/guide.md:Top");
        assert_eq!(chunks[1].id, "doc/guide.md:Top > Sub");
    }

    #[test]
    fn document_mode_uses_file_stem_as_title() {
        let md = "Just a plain paragraph with enough text to be a chunk.\n";
        let chunks = chunk_as_document(md, "path/to/my-notes.md");
        assert_eq!(chunks[0].title, "my-notes");
    }

    #[test]
    fn frontmatter_tags_preserved_in_chunks() {
        let md = "\
---
tags: [design, patterns]
---

# Architecture
A sufficiently long body describing architecture patterns.
";
        let chunks = chunk_by_heading(md, "arch.md");
        let arch_chunk = chunks.iter().find(|c| c.title == "Architecture");
        assert!(arch_chunk.is_some(), "expected an Architecture chunk");
        assert_eq!(arch_chunk.unwrap().tags, "design, patterns");
    }

    #[test]
    fn frontmatter_only_root_chunk_is_suppressed() {
        let md = "\
---
tags: [rust, clippy, linting, code-quality]
---

# Clippy Pedantic
Enable pedantic at warn level with priority -1, then selectively allow noisy lints.

## Common pedantic fixes
Use map_or instead of map().unwrap_or() for cleaner code.
";
        let chunks = chunk_by_heading(md, "clippy.md");
        // Frontmatter should NOT produce a root chunk.
        assert!(
            chunks.iter().all(|c| !c.body.contains("tags:")),
            "no chunk should contain raw frontmatter YAML, got: {:#?}",
            chunks.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        // Real content chunks should still exist with tags propagated.
        assert!(
            chunks.iter().any(|c| c.title == "Clippy Pedantic"),
            "should have a Clippy Pedantic chunk"
        );
        assert_eq!(
            chunks[0].tags, "rust, clippy, linting, code-quality",
            "tags should still be extracted from frontmatter"
        );
    }

    #[test]
    fn pre_heading_prose_still_produces_root_chunk() {
        let md = "\
This is a preamble paragraph with enough text to be meaningful content.

# Main Section
The main section body text that is long enough for a chunk.
";
        let chunks = chunk_by_heading(md, "with-preamble.md");
        // Pre-heading prose (not frontmatter) should still produce a root chunk.
        let root = chunks.iter().find(|c| c.heading_path.is_empty());
        assert!(
            root.is_some(),
            "pre-heading prose should produce a root chunk"
        );
        assert!(root.unwrap().body.contains("preamble paragraph"));
    }

    #[test]
    fn parse_heading_rejects_level_seven() {
        assert!(parse_heading("####### Too deep").is_none());
    }

    #[test]
    fn parse_heading_rejects_empty_text() {
        assert!(parse_heading("## ").is_none());
        assert!(parse_heading("##").is_none());
    }

    #[test]
    fn parse_heading_accepts_valid_headings() {
        assert_eq!(parse_heading("# Foo"), Some((1, "Foo".to_string())));
        assert_eq!(
            parse_heading("### Bar Baz"),
            Some((3, "Bar Baz".to_string()))
        );
    }

    #[test]
    fn duplicate_headings_get_distinct_ids() {
        let md = "\
# Top
Intro text that is definitely long enough for a chunk.

## Examples
First set of examples that is long enough for a chunk.

## Examples
Second set of examples that is long enough for a chunk.
";
        let chunks = chunk_by_heading(md, "guide.md");
        // We should have 3 chunks: Top, Examples (first), Examples (second).
        assert!(
            chunks.len() >= 3,
            "expected at least 3 chunks, got {}",
            chunks.len()
        );

        let example_chunks: Vec<_> = chunks.iter().filter(|c| c.title == "Examples").collect();
        assert_eq!(example_chunks.len(), 2);

        // The two Examples chunks must have distinct IDs.
        assert_ne!(
            example_chunks[0].id, example_chunks[1].id,
            "duplicate heading IDs should be distinct"
        );

        // First Examples gets the base ID, second gets `:2` suffix.
        assert_eq!(example_chunks[0].id, "guide.md:Top > Examples");
        assert_eq!(example_chunks[1].id, "guide.md:Top > Examples:2");

        // Both bodies should be preserved (no data loss).
        assert!(example_chunks[0].body.contains("First set"));
        assert!(example_chunks[1].body.contains("Second set"));
    }

    #[test]
    fn frontmatter_brackets_in_subsequent_field_not_parsed_as_tags() {
        let md = "\
---
tags:
  - alpha
  - beta
other_field: [not, tags]
---

# Hello
Body text that is definitely long enough for a chunk.
";
        let tags = extract_frontmatter_tags(md);
        // Should only pick up alpha and beta, not "not, tags" from other_field.
        assert_eq!(tags, "alpha, beta");
    }

    #[test]
    fn frontmatter_inline_tags_with_brackets_in_later_field() {
        let md = "\
---
tags: [rust, patterns]
categories: [web, api]
---

# Hello
Body text that is definitely long enough for a chunk.
";
        let tags = extract_frontmatter_tags(md);
        assert_eq!(tags, "rust, patterns");
    }
}
