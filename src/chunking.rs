// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure markdown chunking — no I/O, no database, no embeddings.
//!
//! Splits markdown content into [`Chunk`]s by heading structure or as a single
//! whole-document chunk when no headings are present.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::hash::fnv1a;

/// Parsed `applies_when` predicate from a pattern's frontmatter.
///
/// Both fields are optional; the predicate semantics are OR within each list
/// and AND across keys (an unset key is "don't care"). An empty list parses
/// to `Some(vec![])` and is documented to never match (a zero-element
/// allowlist) — the evaluator (U3) enforces that contract; this parser
/// merely preserves the empty list verbatim.
///
/// Stored on each chunk and pattern row as a JSON-serialised string in
/// `applies_when_json`; `None` there means "no predicate, fire as today".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliesWhen {
    /// Tool-class allowlist; matches when the current call's tool name is
    /// in the list (case-sensitive against Claude Code tool names).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Bash-command-prefix allowlist; matches when the current call is
    /// `Bash` AND the command (after walking past one `sudo` and one
    /// `env KEY=VAL` wrapper, see U3) starts with one of the listed tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bash_command_starts_with: Option<Vec<String>>,
}

/// A per-file ingest advisory describing a malformed `applies_when` entry.
///
/// Emitted by [`parse_frontmatter_applies_when`] and surfaced at ingest by
/// U7 as a `Warning:` line via `on_progress` (CLI) or `eprintln!` (MCP write
/// tools). The pattern is ingested as if no predicate were set (R9
/// skip-with-warning), so authors keep the always-on visibility while the
/// signal points them at the typo.
#[derive(Debug, Clone, PartialEq)]
pub struct MalformedPredicateEntry {
    /// Source file path the malformed predicate was found in.
    pub file_path: String,
    /// The offending key (e.g. `appliess_when`, `applies_when.tools`,
    /// `applies_when.foo`). Stable shape so log lines and tests can match.
    pub key: String,
    /// Short human-readable reason ("unknown top-level key", "expected list,
    /// got scalar", "tabs not supported", ...).
    pub reason: String,
}

/// A single chunk of knowledge extracted from a markdown file.
///
/// No `Default` impl by design: every chunk-construction site must explicitly
/// set `is_universal` and `applies_when_json` so that future fixtures and
/// write paths cannot silently default them.
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
    /// `true` when the source pattern's frontmatter tags include `universal`,
    /// which opts the pattern into the always-on injection tier (always
    /// emitted at `SessionStart`, bypasses `PreToolUse` dedup).
    pub is_universal: bool,
    /// JSON-serialised `applies_when` predicate from the pattern's
    /// frontmatter, or `None` when no predicate is set. When `Some`, gates
    /// re-injection of universal chunks at the `PreToolUse` predicate filter
    /// (see U5 in `docs/plans/2026-05-07-001-feat-universal-pattern-predicate-plan.md`).
    /// U1 introduces the field with explicit `None` at every construction
    /// site; U7 plumbs real values from the frontmatter parser.
    pub applies_when_json: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Split `content` into chunks along markdown heading boundaries.
///
/// Falls back to [`chunk_as_document`] when no heading-based chunks are
/// produced (e.g. when the body under every heading is shorter than 10 chars).
///
/// Malformed `applies_when` entries are silently dropped on this entry point;
/// callers that need the per-file ingest advisory list (U7) should use
/// [`chunk_by_heading_with_malformed_predicates`] instead.
pub fn chunk_by_heading(content: &str, source_file: &str) -> Vec<Chunk> {
    chunk_by_heading_with_malformed_predicates(content, source_file).0
}

/// Like [`chunk_by_heading`] but also returns the per-file
/// [`MalformedPredicateEntry`] advisories produced by parsing the
/// `applies_when` block. The advisory list is empty when the frontmatter has
/// no `applies_when` block or when the block parses cleanly. Used by ingest
/// (U7) to surface per-file warnings via `on_progress` (CLI) and `eprintln!`
/// (MCP write tools).
pub fn chunk_by_heading_with_malformed_predicates(
    content: &str,
    source_file: &str,
) -> (Vec<Chunk>, Vec<MalformedPredicateEntry>) {
    let (applies_when, malformed) = parse_frontmatter_applies_when(content, source_file);
    let applies_when_json = serialise_applies_when(applies_when.as_ref());

    let stripped = strip_frontmatter(content);
    let lines: Vec<&str> = stripped.lines().collect();
    let mut chunks = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut current_body: Vec<&str> = Vec::new();
    let mut current_title = file_stem(source_file);
    let tags = extract_frontmatter_tags(content);
    let is_universal = frontmatter_has_tag(content, "universal");
    let mut id_counts: HashMap<String, usize> = HashMap::new();

    let flush = |body: &[&str],
                 title: &str,
                 stack: &[(usize, String)],
                 tags: &str,
                 is_universal: bool,
                 applies_when_json: &Option<String>,
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
            is_universal,
            applies_when_json: applies_when_json.clone(),
        });
    };

    for line in &lines {
        if let Some((level, text)) = parse_heading(line) {
            flush(
                &current_body,
                &current_title,
                &heading_stack,
                &tags,
                is_universal,
                &applies_when_json,
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
        is_universal,
        &applies_when_json,
        source_file,
        &mut chunks,
        &mut id_counts,
    );

    if chunks.is_empty() {
        return chunk_as_document_with_malformed_predicates(content, source_file);
    }

    (chunks, malformed)
}

/// Treat the entire file as a single chunk (after stripping frontmatter).
///
/// Malformed `applies_when` entries are silently dropped on this entry point;
/// callers that need the per-file ingest advisory list (U7) should use
/// [`chunk_as_document_with_malformed_predicates`] instead.
pub fn chunk_as_document(content: &str, source_file: &str) -> Vec<Chunk> {
    chunk_as_document_with_malformed_predicates(content, source_file).0
}

/// Like [`chunk_as_document`] but also returns the per-file
/// [`MalformedPredicateEntry`] advisories from parsing `applies_when`.
pub fn chunk_as_document_with_malformed_predicates(
    content: &str,
    source_file: &str,
) -> (Vec<Chunk>, Vec<MalformedPredicateEntry>) {
    let (applies_when, malformed) = parse_frontmatter_applies_when(content, source_file);
    let applies_when_json = serialise_applies_when(applies_when.as_ref());

    let body = strip_frontmatter(content).trim().to_string();
    if body.len() < 10 {
        return (Vec::new(), malformed);
    }

    let title = extract_title(content).unwrap_or_else(|| file_stem(source_file));
    let tags = extract_frontmatter_tags(content);
    let is_universal = frontmatter_has_tag(content, "universal");

    let chunks = vec![Chunk {
        id: source_file.to_string(),
        title,
        body,
        tags,
        source_file: source_file.to_string(),
        heading_path: String::new(),
        is_universal,
        applies_when_json,
    }];
    (chunks, malformed)
}

/// Serialise an [`AppliesWhen`] to its JSON representation, returning `None`
/// when the predicate is absent. Centralised so chunk and pattern rows
/// always see the same JSON shape.
fn serialise_applies_when(aw: Option<&AppliesWhen>) -> Option<String> {
    aw.map(|aw| {
        // `serde_json::to_string` can only fail on non-string-keyed maps;
        // `AppliesWhen` has only `Vec<String>` fields, so this is infallible
        // in practice.
        serde_json::to_string(aw).expect("AppliesWhen serialises to JSON")
    })
}

/// One row per pattern file for the `patterns` table — the authorial view
/// of an indexed document.
///
/// Built from the file's full raw contents plus the chunks already produced
/// for it (see [`pattern_row_from`]), then handed to
/// [`crate::database::upsert_pattern_in_tx`] as part of single-file ingest.
///
/// `raw_body` is the frontmatter-stripped document — what agents see
/// rendered into `## Pinned conventions`. `content_hash` is over the whole
/// file (frontmatter included) so tag-only edits still invalidate the hash
/// for future delta-ingest short-circuiting.
#[derive(Debug, Clone)]
pub struct PatternRow {
    pub source_file: String,
    pub title: String,
    pub tags: String,
    pub is_universal: bool,
    pub raw_body: String,
    pub content_hash: String,
    /// JSON-serialised `applies_when` predicate from the pattern's
    /// frontmatter; mirrors `Chunk::applies_when_json`. `None` means no
    /// predicate is set and the pattern fires as today (whole-file
    /// semantics — the predicate is shared across every chunk).
    pub applies_when_json: Option<String>,
}

/// Build a [`PatternRow`] from the file's raw contents and its produced
/// chunks. Pure — no I/O.
///
/// Caller is expected to have chunked the file first and verified that at
/// least one chunk was produced; callers that see an empty chunk set should
/// skip upserting a pattern row and let `delete_pattern_and_chunks_in_tx`
/// remove any stale row.
pub fn pattern_row_from(content: &str, source_file: &str, chunks: &[Chunk]) -> PatternRow {
    // All chunks from a single file share the frontmatter-derived
    // `is_universal` flag — we read it from the first chunk instead of
    // re-parsing the frontmatter, keeping the two paths in sync by
    // construction. `applies_when_json` follows the same whole-file mirror
    // (every chunk of a pattern carries the same predicate JSON).
    let is_universal = chunks.first().is_some_and(|c| c.is_universal);
    let tags = chunks.first().map_or_else(String::new, |c| c.tags.clone());
    let applies_when_json = chunks.first().and_then(|c| c.applies_when_json.clone());
    let title = extract_title(content).unwrap_or_else(|| file_stem(source_file));
    let raw_body = strip_frontmatter(content).trim().to_string();
    let content_hash = format!("{:016x}", fnv1a(content.as_bytes()));
    PatternRow {
        source_file: source_file.to_string(),
        title,
        tags,
        is_universal,
        raw_body,
        content_hash,
        applies_when_json,
    }
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

/// Return `true` when the parsed frontmatter `tags:` list contains an exact
/// match for `tag` (case-sensitive). Used to detect the `universal` opt-in
/// without re-parsing the markdown body.
pub fn frontmatter_has_tag(content: &str, tag: &str) -> bool {
    parse_frontmatter_tag_list(content).iter().any(|t| t == tag)
}

/// Return frontmatter tag values that look like typos of `tag` but aren't
/// exact matches. Flags two classes at ingest time so authors notice
/// silent-inert tags:
///
/// 1. Case variants (`Universal`, `UNIVERSAL`) — lowercased form equals the
///    target.
/// 2. Homoglyph candidates — tags containing non-ASCII characters whose
///    character count equals the target's. Catches Cyrillic-i for ASCII-i
///    style substitutions (`unіversal` with Cyrillic `і` U+0456) without
///    pulling in a full Unicode confusables table. Legitimate non-ASCII
///    tags like `résumé` pass through silently because their character
///    count differs from the target's.
pub fn frontmatter_near_miss_tags(content: &str, tag: &str) -> Vec<String> {
    let target_lower = tag.to_lowercase();
    let target_char_count = target_lower.chars().count();

    parse_frontmatter_tag_list(content)
        .into_iter()
        .filter(|t| {
            if t == tag {
                return false;
            }
            let t_lower = t.to_lowercase();
            if t_lower == target_lower {
                return true;
            }
            !t_lower.is_ascii() && t_lower.chars().count() == target_char_count
        })
        .collect()
}

/// Parse the frontmatter `tags:` list into a `Vec<String>` (one entry per tag).
/// Reuses the existing inline / block style logic from `extract_frontmatter_tags`
/// so the two functions never drift.
pub fn parse_frontmatter_tag_list(content: &str) -> Vec<String> {
    let Some(fm) = extract_frontmatter(content) else {
        return Vec::new();
    };
    let Some(start) = fm.find("tags:") else {
        return Vec::new();
    };
    let rest = &fm[start + 5..];

    let first_line = rest.lines().next().unwrap_or("");
    if let Some(bracket_start) = first_line.find('[')
        && let Some(bracket_end) = first_line.find(']')
        && bracket_start < bracket_end
    {
        return first_line[bracket_start + 1..bracket_end]
            .split(',')
            .map(|t| strip_outer_quotes(t.trim()))
            .filter(|t| !t.is_empty())
            .collect();
    }

    rest.lines()
        .skip(1)
        .take_while(|l| l.starts_with("  -") || l.starts_with("- "))
        .map(|l| strip_outer_quotes(l.trim_start_matches([' ', '-']).trim()))
        .filter(|l| !l.is_empty())
        .collect()
}

/// Parse the frontmatter `applies_when:` block into an [`AppliesWhen`] plus a
/// list of [`MalformedPredicateEntry`] advisories.
///
/// Return shape:
///
/// - `(None, vec![])` — the frontmatter has no `applies_when` block (and no
///   near-miss top-level key). Pattern fires as if no predicate were set.
/// - `(Some(aw), vec![])` — clean parse of all known nested keys.
/// - `(Some(aw), vec![entry, ...])` — the block parsed but at least one
///   nested key was malformed; known keys are still populated, the offending
///   keys are listed in the advisory vec.
/// - `(None, vec![entry])` — a top-level near-miss key (e.g. `appliess_when`)
///   was detected, or the `applies_when` block is structurally unusable
///   (e.g. tab-indented children, scalar where a mapping is expected).
///
/// The parser enforces the indentation contract documented in U2 and U8:
/// top-level keys at column 0; nested keys under `applies_when:` at 2-space
/// indent; block-list items at 4-space indent. Tabs are not accepted under
/// `applies_when:`.
///
/// `source_file` is only used to populate the advisory entries' `file_path`
/// field; it does not affect parsing.
pub fn parse_frontmatter_applies_when(
    content: &str,
    source_file: &str,
) -> (Option<AppliesWhen>, Vec<MalformedPredicateEntry>) {
    let Some(fm) = extract_frontmatter(content) else {
        return (None, Vec::new());
    };

    let mut malformed = Vec::new();

    // Detect a typo'd top-level key whose name resembles `applies_when`. The
    // hand-rolled parser only knows the literal `applies_when:`, so this scan
    // has to run independently — we walk every column-0 key and flag any
    // near-miss before falling through to the actual block locator.
    if let Some(typo) = find_top_level_applies_when_typo(&fm) {
        malformed.push(MalformedPredicateEntry {
            file_path: source_file.to_string(),
            key: typo,
            reason: "unknown top-level key (did you mean applies_when?)".to_string(),
        });
        return (None, malformed);
    }

    // Locate `applies_when:` at column 0. We deliberately reject indented
    // occurrences (e.g. mistakenly nested under `tags:`) — those parse as
    // `None` with no advisory, structurally identical to a missing block.
    let Some(block) = extract_applies_when_block(&fm) else {
        return (None, Vec::new());
    };

    // Reject tab indentation up front. Tabs collide with our 2/4-space contract
    // and would silently misparse, so we surface the unsupported indentation
    // and bail with no predicate (R9 skip-with-warning).
    if block_has_tab_indent(&block) {
        malformed.push(MalformedPredicateEntry {
            file_path: source_file.to_string(),
            key: "applies_when".to_string(),
            reason: "tabs not supported in applies_when block (use spaces)".to_string(),
        });
        return (None, malformed);
    }

    let mut applies_when = AppliesWhen {
        tools: None,
        bash_command_starts_with: None,
    };
    let mut any_known_seen = false;

    for entry in iter_applies_when_children(&block) {
        match entry.key.as_str() {
            "tools" => match parse_applies_when_value(&entry) {
                Ok(values) => {
                    applies_when.tools = Some(values);
                    any_known_seen = true;
                }
                Err(reason) => {
                    malformed.push(MalformedPredicateEntry {
                        file_path: source_file.to_string(),
                        key: "applies_when.tools".to_string(),
                        reason,
                    });
                }
            },
            "bash_command_starts_with" => match parse_applies_when_value(&entry) {
                Ok(values) => {
                    applies_when.bash_command_starts_with = Some(values);
                    any_known_seen = true;
                }
                Err(reason) => {
                    malformed.push(MalformedPredicateEntry {
                        file_path: source_file.to_string(),
                        key: "applies_when.bash_command_starts_with".to_string(),
                        reason,
                    });
                }
            },
            other => {
                malformed.push(MalformedPredicateEntry {
                    file_path: source_file.to_string(),
                    key: format!("applies_when.{other}"),
                    reason: "unknown nested key (Track 1 supports tools, bash_command_starts_with)"
                        .to_string(),
                });
            }
        }
    }

    if any_known_seen {
        (Some(applies_when), malformed)
    } else {
        // No usable nested keys — return `None` so the pattern fires as if
        // the block weren't there. Advisories (e.g. type-mismatch on the only
        // key, or unknown-only nested keys) still flow up.
        (None, malformed)
    }
}

/// Find a column-0 frontmatter key whose name looks like a typo of
/// `applies_when` (case-insensitive equality, edit-distance ≤ 2, or a
/// `applies` prefix). Returns the offending key as it appears in the source.
fn find_top_level_applies_when_typo(fm: &str) -> Option<String> {
    const TARGET: &str = "applies_when";

    for line in fm.lines() {
        // Only column-0 keys. A leading space rules out nested children.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[..colon].trim();
        if key.is_empty() || key == TARGET {
            continue;
        }
        let lower = key.to_lowercase();
        if lower == TARGET {
            // Pure case variant (`Applies_When:`). Treat as typo so the
            // author gets a signal — the parser only matches the lowercase
            // literal.
            return Some(key.to_string());
        }
        if lower.starts_with("applies") && key != "applies" {
            return Some(key.to_string());
        }
        if levenshtein_at_most_two(&lower, TARGET) {
            return Some(key.to_string());
        }
    }
    None
}

/// Return the body of the `applies_when:` block from `fm` (frontmatter text
/// without the surrounding `---` fences). The body is the content following
/// the `applies_when:` line up to (but not including) the next column-0 key
/// or end of frontmatter. Inline-mapping form (`applies_when: { ... }`) is
/// NOT accepted — the contract is line-based.
fn extract_applies_when_block(fm: &str) -> Option<String> {
    let mut lines = fm.lines();
    let mut block: Vec<&str> = Vec::new();
    let mut in_block = false;

    for line in lines.by_ref() {
        if !in_block {
            // Strict: column-0 `applies_when:` only.
            if let Some(rest) = line.strip_prefix("applies_when:") {
                if !rest.trim().is_empty() {
                    // Inline scalar/mapping after the colon — Track 1 doesn't
                    // support inline mapping; treat as empty body so children
                    // (none here) yield `None` upstream.
                    return None;
                }
                in_block = true;
            }
            continue;
        }
        // Stop at the next column-0 key (any non-space-leading non-empty line
        // with a colon).
        if !line.starts_with(' ') && !line.starts_with('\t') && line.contains(':') {
            break;
        }
        block.push(line);
    }

    if !in_block {
        return None;
    }
    Some(block.join("\n"))
}

/// Detect tab characters in any leading-indent position of the block. We only
/// care about indentation tabs (mid-value tabs are fine for, e.g., a quoted
/// string with internal whitespace).
fn block_has_tab_indent(block: &str) -> bool {
    block.lines().any(|l| l.starts_with('\t'))
}

/// One nested entry harvested from the `applies_when` block.
struct AppliesWhenEntry {
    /// The nested key (`tools`, `bash_command_starts_with`, or anything else
    /// authors typed under `applies_when:`).
    key: String,
    /// Inline value, if the entry was written as `key: [a, b]` or
    /// `key: scalar`. Empty string when block-list form was used.
    inline_value: String,
    /// Block-list items collected from 4-space-indented `- value` lines that
    /// follow the key line. Empty when inline form was used.
    block_items: Vec<String>,
}

/// Walk the `applies_when:` block body and yield one entry per nested key.
/// Skips blank lines. Lines that don't match the 2-space-indented `key:` shape
/// are ignored — the U2 contract is intentionally narrow.
fn iter_applies_when_children(block: &str) -> Vec<AppliesWhenEntry> {
    let mut out = Vec::new();
    let mut lines = block.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Nested keys must be 2-space-indented (children of `applies_when:`).
        // Reject 0-space indent (a stray top-level key sneaked in) and >2
        // (deeper nesting we don't support in Track 1).
        let indent = leading_space_count(line);
        if indent != 2 {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[indent..colon].trim().to_string();
        if key.is_empty() {
            continue;
        }
        let inline_value = line[colon + 1..].trim().to_string();

        // Collect 4-space-indented block-list items that follow.
        let mut block_items = Vec::new();
        while let Some(next) = lines.peek() {
            let next_indent = leading_space_count(next);
            let next_trim = next.trim();
            if next_trim.is_empty() {
                lines.next();
                continue;
            }
            if next_indent == 4 && (next_trim.starts_with("- ") || next_trim == "-") {
                let item = next_trim[1..].trim();
                if !item.is_empty() {
                    block_items.push(strip_outer_quotes(item));
                }
                lines.next();
                continue;
            }
            // Anything else terminates the current key's items.
            break;
        }

        out.push(AppliesWhenEntry {
            key,
            inline_value,
            block_items,
        });
    }

    out
}

/// Resolve an entry's value into a `Vec<String>` (preserving empty lists)
/// or describe the type-mismatch reason when neither inline-list nor
/// block-list form was used cleanly.
fn parse_applies_when_value(entry: &AppliesWhenEntry) -> Result<Vec<String>, String> {
    if !entry.inline_value.is_empty() {
        let v = entry.inline_value.as_str();
        if let Some(rest) = v.strip_prefix('[') {
            if let Some(inner) = rest.strip_suffix(']') {
                let trimmed = inner.trim();
                if trimmed.is_empty() {
                    return Ok(Vec::new());
                }
                return Ok(trimmed
                    .split(',')
                    .map(|t| strip_outer_quotes(t.trim()))
                    .filter(|t| !t.is_empty())
                    .collect());
            }
            return Err("expected closing ']' in inline list".to_string());
        }
        // Scalar where list expected.
        return Err(format!(
            "expected list, got scalar `{}` (use `{}: [{}]` or a block list)",
            v, entry.key, v
        ));
    }

    // No inline value: must be a block list (possibly empty).
    Ok(entry.block_items.clone())
}

/// Count the leading ASCII spaces on a line (does not count tabs — tab
/// detection lives in [`block_has_tab_indent`]).
fn leading_space_count(line: &str) -> usize {
    line.bytes().take_while(|b| *b == b' ').count()
}

/// Approximate Levenshtein distance check returning `true` when the edit
/// distance between `a` and `b` is at most 2. Used for the `applies_when`
/// typo near-miss heuristic; we don't need a full distance, just a yes/no.
fn levenshtein_at_most_two(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let len_diff = a_bytes.len().abs_diff(b_bytes.len());
    if len_diff > 2 {
        return false;
    }

    let m = a_bytes.len();
    let n = b_bytes.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = usize::from(a_bytes[i - 1] != b_bytes[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n] <= 2
}

/// Strip a single matched pair of surrounding quotes from `s` (only when both
/// ends carry the same quote character). Avoids the half-quote bug where
/// `"universal` and `thing"` (the two halves of a comma-split quoted token)
/// would each get their stray quote stripped and falsely register as the
/// tag `universal`.
fn strip_outer_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Extract frontmatter tags as a comma-joined string for storage in
/// `chunks.tags` (and friends). Thin wrapper over
/// [`parse_frontmatter_tag_list`] so the two paths cannot drift.
fn extract_frontmatter_tags(content: &str) -> String {
    parse_frontmatter_tag_list(content).join(", ")
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
    }

    #[test]
    fn yaml_formatting_pattern_produces_chunks() {
        // Reproduces the exact content of lore-patterns/yaml/formatting.md
        // to verify the file produces at least one indexable chunk.
        let content = "---\n\
            tags: [yaml, yml, formatting, quotes]\n\
            ---\n\
            \n\
            # YAML Formatting\n\
            \n\
            ## Never quote strings unless required by the parser\n\
            \n\
            Do not quote string values in YAML files unless the value requires quotes for\n\
            correct YAML parsing (e.g., contains special characters, starts with `*`, `&`,\n\
            `!`, etc.).\n\
            \n\
            **Why:** Unnecessary quoting adds visual noise and is not idiomatic YAML.\n";
        let chunks = chunk_by_heading(content, "yaml/formatting.md");
        assert!(!chunks.is_empty(), "expected at least one chunk");
        for chunk in &chunks {
            assert_eq!(chunk.source_file, "yaml/formatting.md");
            eprintln!(
                "chunk id={} title={:?} body_len={}",
                chunk.id,
                chunk.title,
                chunk.body.len()
            );
        }
    }

    #[test]
    fn no_headings_root_chunk_has_empty_heading_path() {
        let md = "This file has no headings but has enough body text to make a chunk.\n";
        let chunks = chunk_by_heading(md, "plain.md");
        assert_eq!(chunks.len(), 1);
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

    // -- frontmatter_has_tag / universal flag -----------------------------

    #[test]
    fn frontmatter_has_tag_matches_exact_tag_in_inline_list() {
        let md = "---\ntags: [foo, universal, bar]\n---\n\n# Hello\nBody.\n";
        assert!(frontmatter_has_tag(md, "universal"));
    }

    #[test]
    fn frontmatter_has_tag_matches_exact_tag_in_block_list() {
        let md = "---\ntags:\n  - foo\n  - universal\n  - bar\n---\n\n# Hello\nBody.\n";
        assert!(frontmatter_has_tag(md, "universal"));
    }

    #[test]
    fn frontmatter_has_tag_rejects_substring_matches() {
        let md = "---\ntags: [foo, universally, bar]\n---\n\n# Hello\nBody.\n";
        assert!(!frontmatter_has_tag(md, "universal"));
    }

    #[test]
    fn frontmatter_has_tag_is_case_sensitive() {
        let md = "---\ntags: [Universal]\n---\n\n# Hello\nBody.\n";
        assert!(!frontmatter_has_tag(md, "universal"));
    }

    #[test]
    fn frontmatter_has_tag_returns_false_when_no_frontmatter() {
        let md = "# Hello\nBody.\n";
        assert!(!frontmatter_has_tag(md, "universal"));
    }

    #[test]
    fn frontmatter_has_tag_does_not_match_quoted_tag_with_internal_comma() {
        // Hand-rolled parser fragility check: a quoted tag with a comma
        // in the middle would split into two tokens, neither equal to
        // "universal" exactly.
        let md = "---\ntags: [\"universal,thing\"]\n---\n\n# Hello\nBody.\n";
        assert!(!frontmatter_has_tag(md, "universal"));
    }

    #[test]
    fn frontmatter_near_miss_tags_finds_capitalised_variants() {
        let md = "---\ntags: [Universal, foo, UNIVERSAL]\n---\n\n# Hello\nBody.\n";
        let near = frontmatter_near_miss_tags(md, "universal");
        assert_eq!(near, vec!["Universal".to_string(), "UNIVERSAL".to_string()]);
    }

    #[test]
    fn frontmatter_near_miss_tags_finds_pluralised_variants() {
        // `universally` is a different word, lowercased it's still "universally"
        // so it won't match. But `Universals` lowercased is "universals" — not
        // "universal" either. Only exact-lowercase-matches count as near-misses.
        let md = "---\ntags: [universally]\n---\n\n# Hello\nBody.\n";
        let near = frontmatter_near_miss_tags(md, "universal");
        assert!(near.is_empty(), "got: {near:?}");
    }

    #[test]
    fn frontmatter_near_miss_tags_excludes_exact_match() {
        let md = "---\ntags: [universal]\n---\n\n# Hello\nBody.\n";
        let near = frontmatter_near_miss_tags(md, "universal");
        assert!(near.is_empty());
    }

    #[test]
    fn frontmatter_near_miss_tags_flags_cyrillic_homoglyph() {
        // Cyrillic `і` (U+0456) lowercases to itself, not ASCII `i`, so a
        // pure `to_lowercase()` comparison would silently miss this typo.
        // The length-plus-non-ASCII heuristic catches it.
        let md = "---\ntags: [unіversal]\n---\n\n# Hello\nEnough body.\n";
        let near = frontmatter_near_miss_tags(md, "universal");
        assert_eq!(near, vec!["unіversal".to_string()]);
    }

    #[test]
    fn frontmatter_near_miss_tags_does_not_flag_unrelated_unicode_tags() {
        // Legitimate non-ASCII tags with different character counts must
        // pass through silently — we don't want spurious warnings on every
        // non-English-tag knowledge base.
        let md = "---\ntags: [résumé, emoji-🚀, universal]\n---\n\n# Hello\nBody.\n";
        let near = frontmatter_near_miss_tags(md, "universal");
        assert!(near.is_empty(), "got: {near:?}");
    }

    #[test]
    fn chunk_by_heading_propagates_universal_flag() {
        let md = "---\ntags: [universal, conventions]\n---\n\n# Top\nEnough body text here.\n\n## Sub\nAnother section with enough body text.\n";
        let chunks = chunk_by_heading(md, "uni.md");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.is_universal, "chunk {:?} missing flag", chunk.id);
        }
    }

    #[test]
    fn chunk_by_heading_does_not_set_universal_when_tag_absent() {
        let md = "---\ntags: [conventions]\n---\n\n# Top\nEnough body text here.\n";
        let chunks = chunk_by_heading(md, "uni.md");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(!chunk.is_universal);
        }
    }

    #[test]
    fn chunk_as_document_propagates_universal_flag() {
        let md = "---\ntags: [universal]\n---\n\nNo headings here, just a long enough paragraph.\n";
        let chunks = chunk_as_document(md, "uni.md");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_universal);
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

    // -- parse_frontmatter_applies_when ----------------------------------

    #[test]
    fn applies_when_full_block_inline_lists() {
        let md = "\
---
title: Git Branch and PR Workflow
tags:
  - workflow
  - universal
applies_when:
  tools: [Bash]
  bash_command_starts_with: [git, gh]
---

# Hello
Body text that is definitely long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty(), "got: {malformed:?}");
        let aw = aw.expect("predicate parsed");
        assert_eq!(aw.tools.as_deref(), Some(&["Bash".to_string()][..]));
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["git".to_string(), "gh".to_string()][..])
        );
    }

    #[test]
    fn applies_when_block_list_form() {
        let md = "\
---
tags: [universal]
applies_when:
  bash_command_starts_with:
    - git
    - gh
---

# Hello
Body text that is definitely long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty(), "got: {malformed:?}");
        let aw = aw.expect("predicate parsed");
        assert!(aw.tools.is_none());
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["git".to_string(), "gh".to_string()][..])
        );
    }

    #[test]
    fn applies_when_only_tools_set() {
        let md = "\
---
tags: [universal]
applies_when:
  tools: [Bash, Edit]
---

# Hello
Body text long enough to chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty());
        let aw = aw.unwrap();
        assert_eq!(
            aw.tools.as_deref(),
            Some(&["Bash".to_string(), "Edit".to_string()][..])
        );
        assert!(aw.bash_command_starts_with.is_none());
    }

    #[test]
    fn applies_when_only_bash_command_starts_with_set() {
        let md = "\
---
tags: [universal]
applies_when:
  bash_command_starts_with: [cargo]
---

# Hello
Body text long enough to chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty());
        let aw = aw.unwrap();
        assert!(aw.tools.is_none());
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["cargo".to_string()][..])
        );
    }

    #[test]
    fn applies_when_missing_block_returns_none_no_advisory() {
        let md = "---\ntags: [conventions]\n---\n\n# Hello\nBody long enough.\n";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(aw.is_none());
        assert!(malformed.is_empty());
    }

    #[test]
    fn applies_when_universal_tag_without_block_returns_none() {
        let md = "---\ntags: [universal]\n---\n\n# Hello\nBody long enough here.\n";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(aw.is_none());
        assert!(malformed.is_empty());
    }

    #[test]
    fn applies_when_empty_inline_list_parses_to_empty_vec() {
        let md = "\
---
tags: [universal]
applies_when:
  tools: []
---

# Hello
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty(), "got: {malformed:?}");
        let aw = aw.expect("predicate parsed");
        assert_eq!(aw.tools.as_deref(), Some(&[][..]));
        assert!(aw.bash_command_starts_with.is_none());
    }

    #[test]
    fn applies_when_unicode_value_parses() {
        let md = "\
---
tags: [universal]
applies_when:
  bash_command_starts_with: [gît]
---

# Hello
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty());
        let aw = aw.unwrap();
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["gît".to_string()][..])
        );
    }

    #[test]
    fn applies_when_typoed_top_level_key_returns_none_with_advisory() {
        // AE5: typo'd key -> skip-with-warning, pattern fires as if no
        // predicate. The parser surfaces the typo'd key for the operator.
        let md = "\
---
tags: [universal]
appliess_when:
  tools: [Bash]
---

# Hello
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "patterns/foo.md");
        assert!(aw.is_none(), "predicate must be inert on typo");
        assert_eq!(malformed.len(), 1, "got: {malformed:?}");
        assert_eq!(malformed[0].file_path, "patterns/foo.md");
        assert_eq!(malformed[0].key, "appliess_when");
        assert!(
            malformed[0].reason.contains("did you mean applies_when"),
            "reason: {}",
            malformed[0].reason
        );
    }

    #[test]
    fn applies_when_scalar_where_list_expected_emits_advisory() {
        let md = "\
---
tags: [universal]
applies_when:
  tools: Bash
---

# Hello
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        // No usable known keys parsed cleanly => predicate is None.
        assert!(aw.is_none());
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].key, "applies_when.tools");
        assert!(
            malformed[0].reason.contains("expected list"),
            "reason: {}",
            malformed[0].reason
        );
    }

    #[test]
    fn applies_when_unknown_nested_key_keeps_known_keys_and_warns() {
        let md = "\
---
tags: [universal]
applies_when:
  tools: [Bash]
  foo: bar
---

# Hello
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        let aw = aw.expect("predicate parsed (known keys retained)");
        assert_eq!(aw.tools.as_deref(), Some(&["Bash".to_string()][..]));
        assert!(aw.bash_command_starts_with.is_none());
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].key, "applies_when.foo");
        assert!(
            malformed[0].reason.contains("unknown nested key"),
            "reason: {}",
            malformed[0].reason
        );
    }

    #[test]
    fn applies_when_two_space_indent_with_inline_lists() {
        let md = "\
---
applies_when:
  tools: [Bash]
  bash_command_starts_with: [git]
---

# H
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty(), "got: {malformed:?}");
        let aw = aw.unwrap();
        assert_eq!(aw.tools.as_deref(), Some(&["Bash".to_string()][..]));
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["git".to_string()][..])
        );
    }

    #[test]
    fn applies_when_two_space_indent_with_four_space_block_items() {
        let md = "\
---
applies_when:
  tools:
    - Bash
    - Edit
  bash_command_starts_with:
    - cargo
---

# H
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(malformed.is_empty(), "got: {malformed:?}");
        let aw = aw.unwrap();
        assert_eq!(
            aw.tools.as_deref(),
            Some(&["Bash".to_string(), "Edit".to_string()][..])
        );
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["cargo".to_string()][..])
        );
    }

    #[test]
    fn applies_when_tab_indented_children_emit_advisory_no_predicate() {
        let md = "---\ntags: [universal]\napplies_when:\n\ttools: [Bash]\n---\n\n# H\nBody long enough for a chunk.\n";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(aw.is_none(), "tabs must inert the predicate");
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].key, "applies_when");
        assert!(
            malformed[0].reason.contains("tabs not supported"),
            "reason: {}",
            malformed[0].reason
        );
    }

    #[test]
    fn applies_when_indented_top_level_key_is_not_detected() {
        // `applies_when:` mistakenly indented under another key (column != 0)
        // is structurally identical to a missing block — no advisory fires.
        let md = "\
---
tags:
  - universal
  applies_when:
    tools: [Bash]
---

# H
Body long enough for a chunk.
";
        let (aw, malformed) = parse_frontmatter_applies_when(md, "p.md");
        assert!(aw.is_none());
        assert!(malformed.is_empty(), "got: {malformed:?}");
    }

    // -- chunk-time integration ------------------------------------------

    #[test]
    fn chunk_by_heading_propagates_applies_when_json_to_every_chunk() {
        let md = "\
---
tags: [universal]
applies_when:
  tools: [Bash]
  bash_command_starts_with: [git]
---

# Top
Enough body text for a real chunk here.

## Sub
Another section with enough body text.

## Other
Yet another section with enough body text.
";
        let chunks = chunk_by_heading(md, "uni.md");
        assert!(chunks.len() >= 3, "got {} chunks", chunks.len());
        let first = chunks[0]
            .applies_when_json
            .as_deref()
            .expect("predicate JSON populated");
        // Every chunk shares the same value (whole-file semantics).
        for c in &chunks {
            assert_eq!(
                c.applies_when_json.as_deref(),
                Some(first),
                "chunk {:?} drifted",
                c.id
            );
        }
        // Round-trip: the JSON deserialises back to the parsed predicate.
        let aw: AppliesWhen = serde_json::from_str(first).expect("deserialise");
        assert_eq!(aw.tools.as_deref(), Some(&["Bash".to_string()][..]));
        assert_eq!(
            aw.bash_command_starts_with.as_deref(),
            Some(&["git".to_string()][..])
        );
    }

    #[test]
    fn chunk_by_heading_no_applies_when_means_none_on_chunks() {
        let md = "---\ntags: [universal]\n---\n\n# Top\nEnough body text here.\n";
        let chunks = chunk_by_heading(md, "uni.md");
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(
                c.applies_when_json.is_none(),
                "chunk {:?} unexpectedly had predicate JSON",
                c.id
            );
        }
    }

    #[test]
    fn chunk_by_heading_with_malformed_predicates_surfaces_advisory() {
        let md = "\
---
tags: [universal]
appliess_when:
  tools: [Bash]
---

# Top
Enough body text for a real chunk.
";
        let (chunks, malformed) = chunk_by_heading_with_malformed_predicates(md, "patterns/foo.md");
        assert!(!chunks.is_empty());
        // Advisory survives the chunk wiring.
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].key, "appliess_when");
        // No predicate JSON written when the typo inerted the block.
        for c in &chunks {
            assert!(c.applies_when_json.is_none());
        }
    }

    #[test]
    fn chunk_as_document_propagates_applies_when_json() {
        let md = "\
---
tags: [universal]
applies_when:
  tools: [Bash]
---

No headings here, just a long enough paragraph for a single document chunk.
";
        let chunks = chunk_as_document(md, "uni.md");
        assert_eq!(chunks.len(), 1);
        let json = chunks[0]
            .applies_when_json
            .as_deref()
            .expect("predicate JSON populated");
        let aw: AppliesWhen = serde_json::from_str(json).unwrap();
        assert_eq!(aw.tools.as_deref(), Some(&["Bash".to_string()][..]));
    }

    #[test]
    fn pattern_row_from_mirrors_applies_when_json_from_chunks() {
        // U7 will plumb pattern_row_from end-to-end, but the mirror added in
        // U1 should already see the populated value once chunks carry it.
        let md = "\
---
tags: [universal]
applies_when:
  bash_command_starts_with: [git]
---

# Top
Enough body text for a chunk here.
";
        let chunks = chunk_by_heading(md, "uni.md");
        let row = pattern_row_from(md, "uni.md", &chunks);
        assert!(
            row.applies_when_json.is_some(),
            "pattern row should mirror chunks' applies_when_json"
        );
        assert_eq!(row.applies_when_json, chunks[0].applies_when_json);
    }
}
