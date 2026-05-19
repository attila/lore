---
title: "Schema JSON and tool description prose are testable surface, not docs"
date: 2026-05-19
category: best-practices
module: mcp-server
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - "Shipping a feature whose deliverable is a schema field, an MCP tool description, or any public contract object"
  - "Scoping a plan unit that touches only documentation text on a contract surface (tool descriptions, JSON Schema, OpenAPI, GraphQL SDL)"
  - "Tempted to annotate `Test expectation: none -- documentation` on a unit whose output is a contract artefact rather than behaviour"
  - "Reviewing a PR that adds a field to a schema but no test that asserts the field's presence or shape"
tags:
  - testing
  - mcp
  - schema
  - contract-tests
  - drift-guard
  - regression-pin
---

# Schema JSON and tool description prose are testable surface, not docs

## Context

While planning PR #63 (`feat: accept language arg on add_pattern / update_pattern MCP tools`), the
final implementation unit (U4) was scoped as "tool descriptions + ROADMAP move + CHANGELOG bullet."
The plan annotated its test scenarios as `Test expectation: none -- documentation, manual UAT only`,
treating the schema descriptions as untestable docs.

The user pushed back: _"I want automated unit tests for the feature implemented. provided we have
unit tests for the mcp interface, this is not foreign idea, right?"_ — and on follow-up: _"I want
full test coverage on all meaningful angles. unit tests are cheap, run them."_

That reframing pointed at the actual testable surface in U4:

- The `language` property present on `add_pattern.inputSchema.properties` with the right `oneOf`
  shape.
- The `language` property present on `update_pattern.inputSchema.properties` with the same shape.
- The `language` property **absent** from `append_to_pattern.inputSchema.properties` — load-bearing,
  because a future "let's add it for symmetry" PR would silently undo a deliberate design decision.
- Each tool's `description` string mentions specific stable nouns (`language`, `update_pattern`,
  `preserve`/`clear`/`replace`) that document the contract agents read when listing tools.
- The top-level `tools/list` catalogue still contains every expected tool name.

Every one of these is a substring or structural assertion against `tool_definitions()`'s JSON
output. Cheap to write, fast to run, and they catch a specific regression class that nothing else in
the test suite covers.

## Guidance

When a unit's deliverable is a schema, a contract object, or a description block, the structural and
substring assertions on that artefact **are** the unit tests. Three concrete shapes:

### 1. Pin the presence (and absence) of fields on the schema

For an MCP tool catalogue exposed via `tool_definitions()`:

```rust
#[test]
fn tool_definitions_add_pattern_has_language_oneof_shape() {
    let tool = tool_schema("add_pattern");
    let lang = &tool["inputSchema"]["properties"]["language"];
    let one_of = lang["oneOf"].as_array().expect("language.oneOf array");
    assert_eq!(one_of.len(), 2);
    assert_eq!(one_of[0]["type"], "string");
    assert_eq!(one_of[1]["type"], "array");
    assert_eq!(one_of[1]["items"]["type"], "string");
    assert!(
        !required(&tool).contains(&"language"),
        "must remain optional"
    );
}

#[test]
fn tool_definitions_append_pattern_does_not_accept_language() {
    // Load-bearing pin against a future "add language for symmetry" PR.
    let tool = tool_schema("append_to_pattern");
    assert!(tool["inputSchema"]["properties"].get("language").is_none());
}
```

The presence test catches accidental deletions or renames. The absence test catches additions that
would undo a deliberate design choice. **Both are required for genuinely contract-pinning
coverage.**

### 2. Pin the description prose with substring matches on stable nouns

The text agents read when expanding a schema in their tool registry is the contract documentation.
Drift in that text is drift in the contract:

```rust
#[test]
fn update_pattern_description_mentions_language_and_three_way_semantics() {
    let desc = tool_description("update_pattern");
    assert!(desc.contains("language"), "must mention `language`");
    for keyword in ["preserve", "clear", "replace"] {
        assert!(
            desc.contains(keyword),
            "three-way semantics keyword `{keyword}` missing from description: {desc}"
        );
    }
}
```

Key choice: assert on stable nouns the agent contract is keyed to (`language`, `update_pattern`,
`preserve`/`clear`/`replace`), not on the surrounding prose. Wording can drift; nouns can't drift
without changing the contract.

### 3. Pin catalogue integrity with a names-only assertion

```rust
#[test]
fn tools_list_contains_all_expected_tools() {
    let names: Vec<&str> = tool_definitions()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "search_patterns",
            "add_pattern",
            "update_pattern",
            "append_to_pattern",
            "list_patterns",
            "lore_status"
        ]
    );
}
```

Cheap regression guard against accidentally dropping a tool from the catalogue.

## Why This Matters

The default mental model for "documentation deliverable" treats the artefact as inert text — read by
humans, untestable by machines. That mental model is wrong for schema JSON, tool descriptions,
OpenAPI spec text, GraphQL SDL comments, and any contract artefact agents (or downstream code) read.

Three concrete failure modes the substring/structural tests catch:

- **Decision 3 regression.** This PR's `append_to_pattern` deliberately does not accept `language`
  (Decision 3 — schema honesty over surface symmetry). Without
  `tool_definitions_append_pattern_does_not_accept_language`, a future symmetry-driven PR titled
  "add language to append for consistency" would land green and silently undo the design.
- **De-language footgun via doc drift.** `update_pattern`'s `language` description documents the
  three-way semantics (omit preserves, `[]` clears, non-empty replaces). If a future edit drops the
  word `preserve`, agents reading the schema no longer learn the contract, and the de-language
  footgun reappears in agent code rather than in the implementation. The substring-on-stable-nouns
  pattern catches this.
- **Accidental property removal.** A careless schema edit could drop `tags` while adding `language`.
  The sibling-property regression guard catches it before it ships.

The cost is small: five substring or structural assertions per touched schema. The benefit is a
durable regression guard against the failure modes that don't have any other test surface.

## When to Apply

Apply when the unit's deliverable is **the contract text or schema itself**:

- An MCP tool's `inputSchema`, `description`, or catalogue entry
- An OpenAPI / Swagger / GraphQL SDL contract field
- A JSON Schema or YAML schema file with downstream consumers
- A public type / interface signature with cross-package consumers
- A CLI `--help` string that downstream automation greps

Skip when:

- The text is internal-only prose (a `# Conventions` section in `AGENTS.md`, a comment block in a
  source file) with no programmatic consumer
- The contract is already pinned by an OpenAPI / JSON Schema generator that runs in CI — drift shows
  up at the generator boundary, not the consumer

## Examples

The U4 work in PR #63 ships eight unit tests against `tool_definitions()`:

| Test                                                                      | Pins                                                                            |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `tool_definitions_add_pattern_has_language_oneof_shape`                   | `language` present + `oneOf` shape on `add_pattern`                             |
| `tool_definitions_update_pattern_has_language_oneof_shape`                | Same on `update_pattern`                                                        |
| `tool_definitions_append_pattern_does_not_accept_language`                | **`language` absent** on `append_to_pattern` (load-bearing)                     |
| `add_pattern_description_mentions_language_and_warn_behaviour`            | Description mentions `language` + unknown-token policy                          |
| `update_pattern_description_mentions_language_and_three_way_semantics`    | Description mentions `language` + `preserve`/`clear`/`replace`                  |
| `update_pattern_language_field_description_documents_three_way_semantics` | Per-field description echoes the three-way vocabulary                           |
| `append_pattern_description_points_at_update_pattern_for_language`        | Append description redirects agents to `update_pattern` for frontmatter changes |
| `tools_list_returns_all_six_tools` (existing, extended)                   | Catalogue integrity                                                             |

Total cost: ~80 lines of test code, all single-file, all `cargo test`-fast. Coverage gap that would
otherwise exist: every regression listed under **Why This Matters**.

## Related

- [`slice-shape-tests-are-not-pipeline-tests-2026-05-19.md`](slice-shape-tests-are-not-pipeline-tests-2026-05-19.md)
  — adjacent lesson on the other end of the testing spectrum: shape tests on static data slices
  don't prove pipeline integration. This doc says "do write the shape tests for contract surfaces";
  the slice-shape doc says "and don't stop there for behavioural surfaces."
- [`mcp-metadata-via-fenced-content-block-2026-04-07.md`](mcp-metadata-via-fenced-content-block-2026-04-07.md)
  — the metadata fence design choice this PR extends with `language_warnings`. Same MCP-tool design
  area.
- `docs/plans/2026-05-19-001-feat-mcp-language-arg-plan.md` — the plan whose U4 carries the
  schema-shape and description-prose unit tests this lesson generalises from.
