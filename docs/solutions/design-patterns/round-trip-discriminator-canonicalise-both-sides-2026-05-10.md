---
title: "Round-trip discriminator: canonicalise both sides, validate at the write boundary"
date: 2026-05-10
category: design-patterns
module: lore
problem_type: design_pattern
component: tooling
severity: medium
applies_when:
  - "Comparing an incoming value against a round-tripped (serialised then re-parsed) form of the same value"
  - "A discriminator selects between branches based on string equality of two representations"
  - "The serialisation layer silently transforms or truncates input (e.g. `# heading\\n` truncates at the first newline)"
  - "Unicode normalisation, whitespace, or control characters can differ between input and storage"
tags:
  - canonicalisation
  - round-trip
  - input-validation
  - write-boundary
  - serialisation
  - discriminator
  - cli-design
related_components:
  - documentation
---

# Round-trip discriminator: canonicalise both sides, validate at the write boundary

## Context

CLI write tools (and any agent-facing tool that ingests user input) frequently decide between two
branches — "this is a legitimate update" vs "this is a name collision" — by comparing an incoming
string against a canonical form recovered from previously-stored state. The discriminator typically
looks like: write input `X` to disk on first call; on second call, read the stored representation,
parse out the field, compare against the new `X'`. If equal, treat as re-use; if different, treat as
collision.

This pattern is fragile whenever the write-side serialisation is lossy (trims whitespace, truncates
at a delimiter, normalises Unicode) or whenever the read-side parse recovers a form that has been
silently transformed by the storage layer. The discriminator then misfires on inputs that are
semantically the same as the stored value but textually different after the round-trip.

The friction surfaced in lore as `add_pattern` in `src/ingest.rs`, which serialises a pattern file
as `# {title}\n\n{body}` and later reads it back via `extract_title` to decide whether a second call
is a re-use of the same slug or a collision between two distinct titles. The brainstorm specified
NFC normalisation on both sides — necessary, since `extract_title` returns the raw heading bytes,
which may be NFC on disk while incoming is NFD or vice versa. A multi-reviewer pass on the
implementation surfaced two further failure modes the brainstorm had missed: surrounding whitespace
and embedded newlines.

## Guidance

Two rules, applied together:

1. **Canonicalise symmetrically on both sides.** Whatever transformation the read-side parser
   applies (trim, NFC normalise, delimiter-bounded extraction), apply the same transformation to the
   incoming value before comparing. Comparing raw incoming bytes against parsed-and-trimmed stored
   bytes is a bug.

2. **Validate at the write boundary so the round-trip is lossless.** Reject inputs the serialisation
   cannot represent faithfully. If the format is `# {title}\n` then `title` containing `\n` or `\r`
   cannot survive the round-trip — bail at write time rather than truncate silently.

Sketch from `src/ingest.rs`:

```rust
fn add_pattern(title: &str, body: &str) -> Result<()> {
    // 1. Validate at the write boundary.
    let title = title.trim();
    if title.contains('\n') || title.contains('\r') {
        bail!("Title must not contain newline characters");
    }
    let slug = slugify(title); // slugify also NFC-normalises internally
    let path = patterns_dir().join(format!("{slug}.md"));

    if path.exists() {
        let stored = fs::read_to_string(&path)?;
        let stored_title = extract_title(&stored); // trims, returns first heading

        // 2. Canonicalise symmetrically.
        let lhs: String = title.nfc().collect();
        let rhs: Option<String> = stored_title.as_deref().map(|t| t.nfc().collect());

        if rhs.as_deref() == Some(lhs.as_str()) {
            bail!("Pattern '{slug}' already exists; use update_pattern");
        }
        bail!(
            "Slug collision: '{}' vs '{}'; choose a different title",
            stored_title.unwrap_or("<no heading>".into()),
            title
        );
    }

    fs::write(&path, format!("# {title}\n\n{body}"))?;
    Ok(())
}
```

Both sides go through `.nfc()`. Whitespace is trimmed at the write boundary so `extract_title`'s
trim cannot diverge from the input. Embedded newlines are rejected before they can desync the
heading from the body.

## Why This Matters

A misclassifying discriminator does not merely return a wrong boolean — it routes the user to a
recovery path that cannot succeed. In lore, a legitimate re-use misclassified as a collision tells
the user "choose a different title" when the right move was "call `update_pattern`". The user has no
observable way to discover the round-trip is lossy; from their seat the title looks identical. They
will rename, re-rename, and eventually file a bug or give up.

The symmetry also matters in the other direction. Without write-boundary validation, a title like
`"Foo\nBar"` writes a file whose first line is `# Foo`, whose second line is `Bar`, and whose body
silently absorbs `Bar` as prose. The next call with the same title appears to collide with itself
because the stored heading is now just `Foo`. The user has no surface to diagnose the corruption.

This pairs with the project's CLI behaviour ladder
([`docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md`](../conventions/cli-behaviour-ladder-2026-05-10.md)):
the ladder decides which **tier** an edge case belongs in (hard-fail vs warn vs silent); this rule
makes sure the tier-1 hard-fail check is actually correctly implemented. A tier-1 collision check
that asymmetrically canonicalises is a tier-1 check that false-fires on legitimate re-uses, which
defeats the ladder's intent.

## When to Apply

Apply whenever both of these are true:

- The write side serialises through a transformation that is not bijective: trimming,
  delimiter-bounded extraction (`# heading\n`, CSV, TSV, headers terminated by blank line), Unicode
  normalisation, case folding, percent-encoding.
- The read side parses the stored representation and the parsed value feeds into a control-flow
  decision (re-use vs collision, cache hit vs miss, idempotency check, dedup key).

Concrete surfaces: filesystem-backed CLI tools with human-readable formats; MCP / tool-call handlers
that store and re-read state; idempotency keys derived from user-supplied strings; any
"create-or-update" endpoint whose discriminator runs over a serialised round-trip.

If the write format is genuinely lossless (length-prefixed binary, JSON with the field stored
verbatim, a key-value store keyed by an opaque hash of the verbatim bytes), this pattern does not
apply — though most "human-readable" formats are lossy in at least one of the dimensions above.

## Examples

`lore` `add_pattern` discriminator, before vs after.

**Adversarial input 1 — whitespace.** `title = "  Foo  "`.

- Pre-fix: writes `# Foo  \n\n{body}`. Second call with `"Foo"` reads back, `extract_title` trims to
  `"Foo"`, incoming is `"Foo"` — accidentally classifies as re-use. Second call with `"  Foo  "`
  reads back trimmed `"Foo"`, incoming is `"  Foo  "`, mismatch, misclassifies legitimate re-use as
  collision.
- Post-fix: incoming is trimmed at the write boundary, so the stored heading is `# Foo` and any
  subsequent `"  Foo  "` or `"Foo"` canonicalises identically.

**Adversarial input 2 — embedded newline.** `title = "Foo\nBar"`.

- Pre-fix: writes `# Foo\nBar\n\n{body}`. The `Bar` line is silently absorbed into the body.
  `extract_title` returns `"Foo"`. Second call with `"Foo"` is misclassified as re-use of a pattern
  whose stored body is corrupt.
- Post-fix: `add_pattern` rejects the input with `"Title must not contain newline characters"`
  before any write, so the lossy state never exists.

**Adversarial input 3 — Unicode form.** `title = "café"` in NFD, stored heading later read as NFC
(or vice versa, depending on filesystem and editor).

- Pre-fix: bytewise comparison of NFD incoming against NFC stored fails; legitimate re-use
  misclassified as collision.
- Post-fix: both sides go through `.nfc()` before comparison; the discriminator is stable across
  encodings.

## Related

- [`docs/solutions/conventions/cli-behaviour-ladder-2026-05-10.md`](../conventions/cli-behaviour-ladder-2026-05-10.md)
  — sibling CLI design convention. Decides the **tier** for an edge case (hard-fail / warn /
  silent); this doc explains how to make sure the check that implements a tier is itself correct
  under round-tripping.
- [`docs/solutions/database-issues/fts5-query-sanitization-crashes-on-special-chars-2026-04-02.md`](../database-issues/fts5-query-sanitization-crashes-on-special-chars-2026-04-02.md)
  — adjacent input-sanitisation rule on the read side (FTS5 query parser). Same shape (canonicalise
  input at the boundary), different surface.
- [`docs/solutions/best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md`](../best-practices/composition-cascades-new-write-paths-can-be-silently-undone-2026-04-06.md)
  — write-path discipline in the same module (`src/ingest.rs`); concerns reconciliation cascades
  rather than input canonicalisation, but pairs in the broader category of "be careful what you
  write to disk and how it round-trips".
- Upstream commit landing the codified rule:
  `091afed fix: sanitise add_pattern titles, harden
  collision discriminator` on branch
  `feat/edge-case-handling` (PR #42 against `attila/lore`).
