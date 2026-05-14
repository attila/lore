---
date: 2026-05-13
topic: language-pack-follow-on-seed
status: seed
---

# Follow-on Language-Pack Task — Seed Content

This file holds pre-loaded vocabulary decisions and open questions for the follow-on task that adds
new languages to the shared detection table introduced by the `language-detection-architecture`
slice. It is not a full brainstorm — the actual scope, sequencing, and Acceptance Examples for the
language-pack work are authored when that task is opened.

## Origin

Extracted from the `language-detection-architecture` brainstorm review (round 1) so the vocabulary
discussion that happened during that brainstorm is not lost and is not treated as binding decisions
on the architecture slice. The architecture slice ships single-tuple-per-language contribution
surface; this file captures what the first round of contributions might use, subject to revision
when this task is actually planned.

## Pre-loaded canonical tokens

Tentative canonical FTS5 tokens for the next round of languages, all conforming to R4 of the
architecture slice (FTS5-safe, avoid stop-words):

- `ruby` — Ruby
- `java` — Java
- `cpp` — C++ (avoids the `+` FTS5-special character)
- `csharp` — C# (avoids the `#` FTS5-special character)
- `php` — PHP
- `swift` — Swift
- `kotlin` — Kotlin
- `bash` — shell scripts (with `sh`/`zsh` as command keywords mapping to the same `bash` token;
  multi-value `language: [bash, fish]` available for fish-specific patterns)
- `clang` — C (single-letter `c` token fails the 3-character minimum in `clean_terms`; `clang` is
  the de facto modern C toolchain shorthand). Command keywords for the C entry: `clang`, `gcc`.
  Command keywords for the C++ entry: `clang++`, `g++`.

## Open question

- Whether Objective-C ships in the first language-pack round (canonical token would be `objectivec`
  — no hyphen, FTS5-safe). The `.m` extension is ambiguous between Objective-C and MATLAB; needs the
  R5 contested-signal policy to apply.

## Out of scope for this seed

- Per-language marker-filename and directory-hint lists. These are authored when the task is
  planned, against R6's three-test policy and the post-architecture-slice codebase state.
- Sequencing of language additions (which languages first, which second). Maintainer's call at
  task-planning time.
- Scope and acceptance criteria for the task itself. The actual brainstorm authors those.
