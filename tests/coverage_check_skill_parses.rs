//! Structural validation for the `coverage-check` Claude Code skill.
//!
//! This test does **not** exercise runtime behaviour — the skill is markdown
//! consumed by an LLM at invocation time and has no automated end-to-end
//! harness in v1. What this test catches is a much narrower set of failures
//! that would otherwise only be discovered during a manual smoke test:
//!
//! 1. The `SKILL.md` file exists at the expected path under
//!    `integrations/claude-code/skills/coverage-check/`.
//! 2. The YAML frontmatter parses cleanly with the four required fields:
//!    `name`, `description`, `disable-model-invocation`, `user-invocable`.
//! 3. The `name` field is exactly `coverage-check` (matches the directory
//!    name, which becomes the slash command).
//! 4. Every MCP tool the skill body references by name (`search_patterns`,
//!    `lore_status`) is a tool the lore MCP server actually exposes. This
//!    catches typos like `search_pattern` (singular) or `lore-status`
//!    (kebab-case) at build time, before the implementer wastes a smoke
//!    test slot diagnosing them.
//!
//! The expected MCP tool list is hardcoded against the canonical pin in
//! `src/server.rs::tests::tools_list_returns_all_five_tools`. If that test
//! ever needs updating because the tool set has changed, this constant
//! must be updated alongside it.
//!
//! Plan reference: `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md`
//! Unit 3, "Files" section.

use std::fs;
use std::path::PathBuf;

/// Tools the lore MCP server registers, mirrored from the canonical pin in
/// `src/server.rs::tests::tools_list_returns_all_five_tools`. The skill is
/// only allowed to reference tools from this list.
const KNOWN_MCP_TOOLS: &[&str] = &[
    "search_patterns",
    "add_pattern",
    "update_pattern",
    "append_to_pattern",
    "lore_status",
];

/// MCP tool names the coverage-check skill explicitly depends on. The test
/// asserts (a) these tools are in `KNOWN_MCP_TOOLS` (subset check) and (b)
/// they appear in the SKILL.md body (the skill actually mentions them).
const SKILL_REQUIRED_TOOLS: &[&str] = &["search_patterns", "lore_status"];

fn skill_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("integrations/claude-code/skills/coverage-check/SKILL.md")
}

#[test]
fn coverage_check_skill_file_exists() {
    let path = skill_path();
    assert!(
        path.exists(),
        "coverage-check SKILL.md must exist at {}",
        path.display()
    );
}

#[test]
fn coverage_check_skill_frontmatter_has_required_fields() {
    let contents = fs::read_to_string(skill_path()).expect("read SKILL.md");
    let (frontmatter, _body) = split_frontmatter(&contents);

    assert_eq!(
        frontmatter_field(&frontmatter, "name"),
        Some("coverage-check"),
        "frontmatter `name` must be exactly `coverage-check` (matches the directory name and slash command)"
    );

    assert!(
        frontmatter_field(&frontmatter, "description").is_some(),
        "frontmatter `description` must be present"
    );

    assert_eq!(
        frontmatter_field(&frontmatter, "disable-model-invocation"),
        Some("true"),
        "frontmatter `disable-model-invocation` must be `true` — this skill is human-invoked, not model-driven"
    );

    assert_eq!(
        frontmatter_field(&frontmatter, "user-invocable"),
        Some("true"),
        "frontmatter `user-invocable` must be `true` — the slash command is the entry point"
    );
}

#[test]
fn coverage_check_skill_only_references_known_mcp_tools() {
    let contents = fs::read_to_string(skill_path()).expect("read SKILL.md");
    let (_frontmatter, body) = split_frontmatter(&contents);

    for required in SKILL_REQUIRED_TOOLS {
        assert!(
            KNOWN_MCP_TOOLS.contains(required),
            "`{required}` is in SKILL_REQUIRED_TOOLS but not in KNOWN_MCP_TOOLS — fix this test, not the skill"
        );
        assert!(
            body.contains(required),
            "SKILL.md body must reference the `{required}` MCP tool — coverage-check depends on it"
        );
    }
}

#[test]
fn coverage_check_skill_does_not_reference_unknown_tools() {
    // Catch typos like `search_pattern` (singular), `lore-status`
    // (kebab-case), or `searchPatterns` (camelCase) by scanning the body
    // for plausible-looking MCP tool name patterns and asserting each
    // one matches a known tool. This is intentionally conservative —
    // any identifier that looks like an MCP tool name (snake_case ASCII
    // ending in `_patterns`, `_pattern`, `_status`, or `_file`) is
    // checked against KNOWN_MCP_TOOLS.
    let contents = fs::read_to_string(skill_path()).expect("read SKILL.md");
    let (_frontmatter, body) = split_frontmatter(&contents);

    let suspect_suffixes = ["_patterns", "_pattern", "_status", "_file"];
    for word in body.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        if suspect_suffixes.iter().any(|s| word.ends_with(s)) && word.contains('_') {
            // Allowlist non-tool identifiers that legitimately match the
            // pattern: `source_file`, `chunks_indexed_for_source` (a hypothetical
            // future field), etc. These are field names from MCP response
            // metadata, not tool names, and the skill body references them.
            const ALLOWLIST: &[&str] = &[
                "source_file",
                "chunks_indexed_for_source",
                "log_file",
                "reindex_file",
            ];
            if ALLOWLIST.contains(&word) {
                continue;
            }
            assert!(
                KNOWN_MCP_TOOLS.contains(&word),
                "SKILL.md mentions `{word}` which looks like an MCP tool name but is not in KNOWN_MCP_TOOLS. Either fix the typo, or add it to the ALLOWLIST in this test if it is actually a field name."
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Hand-rolled YAML frontmatter parsing
// ---------------------------------------------------------------------------
//
// SKILL.md frontmatter is shallow (four flat key-value pairs) and bounded by
// `---` lines. Adding a YAML crate dependency for one structural test would
// be disproportionate to the complexity of the parsing.

fn split_frontmatter(contents: &str) -> (String, String) {
    let mut lines = contents.lines();
    let first = lines.next().unwrap_or("");
    assert_eq!(
        first, "---",
        "SKILL.md must start with `---` to open the YAML frontmatter, got: {first}"
    );
    let mut frontmatter_lines = Vec::new();
    let mut found_close = false;
    for line in lines.by_ref() {
        if line == "---" {
            found_close = true;
            break;
        }
        frontmatter_lines.push(line);
    }
    assert!(found_close, "SKILL.md frontmatter must close with `---`");
    let body: Vec<&str> = lines.collect();
    (frontmatter_lines.join("\n"), body.join("\n"))
}

fn frontmatter_field<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    for line in frontmatter.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                return Some(v.trim());
            }
        }
    }
    None
}
