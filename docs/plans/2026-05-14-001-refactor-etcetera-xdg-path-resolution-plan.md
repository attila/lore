---
title: "refactor: Replace hand-rolled XDG resolution with etcetera"
type: refactor
status: active
date: 2026-05-14
origin: docs/brainstorms/2026-05-14-track-2-observability-requirements.md
---

# Replace hand-rolled XDG resolution with `etcetera`

## Summary

Swap lore's hand-rolled XDG resolution (`resolve_xdg_base`, `default_config_path`,
`default_database_path` in `src/config.rs:107-148`) for the `etcetera` crate's `Xdg` strategy, and
add a new `default_trace_dir()` helper returning the XDG state tier (`$XDG_STATE_HOME/lore/traces`,
with `$HOME/.local/state/lore/traces` fallback). The macOS posture becomes explicit at the call site
(XDG-everywhere, not platform-native) rather than implicit in hand-rolled code. No observable Linux
or macOS behaviour change for the two existing helpers; `default_trace_dir()` ships as quiet
infrastructure for the forthcoming Track 2 Observability work.

Pre-1.0 is the right window for this refactor — deferring past 1.0 makes any future adjustment to
path resolution a breaking change for operators.

---

## Problem Frame

`src/config.rs:107-148` hand-rolls XDG base-directory resolution: `resolve_xdg_base` reads
`XDG_*_HOME` (treating empty strings as unset per XDG 0.8), falls back to `$HOME/<subpath>`, and
returns an `anyhow::Result` whose `$HOME`-not-set wording references the `--config` recovery flag.
Two helpers — `default_config_path` and `default_database_path` — join `lore/lore.toml` and
`lore/knowledge.db` onto the resolved base.

Track 2 Observability (origin) lands trace files in the XDG state tier
(`$XDG_STATE_HOME/lore/traces/`). Adding a third hand-rolled call site would duplicate the
resolution logic for an additional XDG variable. The origin's `Key
Decisions` section carves out
this refactor as the path-resolution prerequisite: replace `resolve_xdg_base` with `etcetera`'s
`Xdg` strategy, retrofit the existing helpers, and add the state-tier helper. The XDG-everywhere
posture on macOS — which lore's terminal-fluent user persona expects — becomes explicit through the
`Xdg` strategy choice rather than implicit in hand-rolled code.

---

## Requirements

Adapted from origin's `Dependencies / Assumptions` (path-resolution refactor prerequisite) and the
user's hard-constraint list at session start.

- **R-PR1.** Replace `resolve_xdg_base` (`src/config.rs:107-126`) and delete it. `etcetera`'s `Xdg`
  strategy is the single source of XDG resolution post-swap.
- **R-PR2.** `default_config_path()` and `default_database_path()` preserve their public signatures
  (`pub fn ... -> anyhow::Result<PathBuf>`) and resolve to the same paths as before for every
  operator-reachable input on Linux and macOS:
  - `default_config_path()` → `$XDG_CONFIG_HOME/lore/lore.toml` (set, non-empty) or
    `$HOME/.config/lore/lore.toml`.
  - `default_database_path()` → `$XDG_DATA_HOME/lore/knowledge.db` (set, non-empty) or
    `$HOME/.local/share/lore/knowledge.db`.
- **R-PR3.** Empty `XDG_CONFIG_HOME` or `XDG_DATA_HOME` must continue to fall back to the
  `$HOME`-based default (parity with the existing `xdg_var_empty_falls_back_to_home` contract called
  out in the user's hard constraints). XDG 0.8+ requires this; the swap must verify `etcetera`
  honours it.
- **R-PR4.** `$HOME`-not-set on either helper surfaces the existing wording verbatim:
  `Cannot determine <purpose> directory: $HOME is not set. Use
  --config to specify a path.` The
  `<purpose>` token is `config` and `data` respectively. The implementer maps `etcetera`'s
  missing-home error onto this anyhow context.
- **R-PR5.** A new `pub fn default_trace_dir() -> anyhow::Result<PathBuf>` resolves to
  `$XDG_STATE_HOME/lore/traces` (set, non-empty) or `$HOME/.local/state/lore/traces`. The
  `$HOME`-not-set path produces wording in the same shape as R-PR4, with `state` as the purpose
  token.
- **R-PR6.** No new entries in `deny.toml`. `etcetera` is licensed `MIT OR Apache-2.0`, already on
  the allowlist; `just deny` must pass unchanged. The implementer runs `just deny` after `cargo add`
  to confirm.
- **R-PR7.** `docs/configuration.md` Environment Variables table gains an `XDG_STATE_HOME` row. The
  CHANGELOG receives one assertive-voice `[Unreleased] / Changed` entry ending in `(#N)`.

---

## Scope Boundaries

- **No call-site changes outside `src/config.rs`.** `src/main.rs:8` keeps its existing import;
  `src/main.rs:144` and `:214` keep their call sites. The public signatures of `default_config_path`
  and `default_database_path` are preserved.
- **`default_trace_dir()` is not wired into any runtime code path.** It ships as infrastructure for
  Track 2 Observability; that separate plan wires it into the hook trace-write path. Pre-Track-2,
  the helper exists, has tests, and is `pub fn` so it's reachable, but no production branch calls
  it.
- **No new CLI flag, no new env var consumed by lore.** `XDG_STATE_HOME` is a standard XDG variable;
  documenting it in `docs/configuration.md` is a forward-compatibility courtesy, not a new contract
  lore introduces.
- **No change to `tests/init_output.rs` or `tests/smoke.rs` assertions.** They already exercise the
  XDG variant via child processes with `.env(...)` — the gold-standard pattern in this codebase for
  env-driven path tests — and the swap preserves the behaviour they pin (R-PR2).
- **No change to the existing `XDG_CONFIG_HOME` / `XDG_DATA_HOME` rows in `docs/configuration.md`.**
  They already describe the resolved behaviour accurately; the swap does not change it.

### Deferred to Follow-Up Work

- **Track 2 Observability** — uses `default_trace_dir()` to land trace files in the state tier.
  Separately scoped per origin; this refactor is the hard prerequisite.

---

## Key Technical Decisions

- **Three implementation units, smallest-blast-radius first.** U1 adds the crate and
  `default_trace_dir()` without touching the existing helpers — proves the `etcetera` integration on
  a fresh surface where the inline test suite at `src/config.rs:204-267` can't regress. U2 migrates
  `default_config_path` and `default_database_path` to `etcetera`, deletes `resolve_xdg_base`, and
  rewrites the affected inline tests. U3 lands the documentation update and the CHANGELOG entry.
  Each unit is independently committable.
- **No parallel implementation, no feature flag.** The swap is `Xdg::new()` one-shot. No deprecation
  comment on `resolve_xdg_base`, no transitional shim — U2 deletes it. Forward-compatibility is paid
  by behaviour preservation, not by leaving dead code in place.
- **Error wording for `$HOME`-not-set stays verbatim** (R-PR4). The phrase
  `Cannot determine <purpose> directory: $HOME is not set. Use --config to
  specify a path.` is
  load-bearing — it names the operator's recovery action. The implementer wraps `Xdg::new()`'s error
  in `anyhow::anyhow!` with this exact shape, using `config`, `data`, and `state` as the purpose
  tokens for the three helpers respectively.
- **Test rewrite uses public helpers, not internals.** The deleted tests at `src/config.rs:204-267`
  exercised `resolve_xdg_base` via direct parameter injection (`xdg_value: Option<String>`,
  `home_value: Option<String>`). After the swap, the helpers read `std::env::var` through
  `etcetera`, so test injection must happen through env vars. Two viable approaches, the implementer
  chooses:
  1. **`temp_env` dev-dependency** — provides `with_var(...)` and `with_var_unset(...)` closures
     around `std::env::set_var` that avoid the Rust 2024 `unsafe` ergonomics. Tests stay inline.
     License check via `just deny` after `cargo add --dev`.
  2. **Push XDG variant coverage to `tests/init_output.rs`** — spawn `lore` as a child process with
     controlled `.env(...)` and assert on the resulting on-disk path. Pattern already in use at
     `tests/init_output.rs:53-175`. No new dependency. Inline coverage shrinks to whatever can be
     exercised without env mutation (likely none, given `etcetera` reads env directly).

  **Recommended: option 1** — keeps inline-test density, avoids inflating integration-test runtime,
  and matches the existing inline pattern for `src/config.rs` tests. Fallback to option 2 if
  `temp_env` audits unfavourably or if the implementer prefers no new dev-dep for a single-helper
  module.
- **`default_trace_dir()` returns the directory, not a file path.** Trace files within it are named
  by session id (R1 in origin); the helper returns the parent. `PathBuf` does not carry a trailing
  slash — the origin's `$XDG_STATE_HOME/lore/traces/` notation is illustrative, not literal.
- **`pub fn` visibility for `default_trace_dir`.** Mirrors `default_config_path` and
  `default_database_path` so Track 2 can call it from `src/main.rs` or wherever the trace-write path
  lands. No reason to narrow to `pub(crate)`.
- **CHANGELOG entry under `Changed`, not `Added`.** Per the codified user-facing-only rule
  (`feedback_changelog_entries.md` in memory), the internal swap is invisible to operators; the only
  user-facing element is the `XDG_STATE_HOME` documentation row. The entry framing should lead with
  the forward-compatibility move (state-tier coverage), not the internal-swap detail — that belongs
  in the PR body.
- **Resolved planning-time questions:**
  - Does `etcetera`'s `Xdg` honour empty-string-as-unset? Assumed yes (XDG 0.8+ requires it;
    `etcetera` 0.10+ implements it). The R-PR3 test verifies before merge. If false, wrap the env
    reads in a tiny normaliser or pin a known-good version.
  - Keep `resolve_xdg_base` as a private helper? No — U2 deletes it entirely.
- **Deferred to implementation:**
  - Exact `etcetera` method names (`config_dir()`, `data_dir()`, `state_dir()` or equivalent) and
    the error variant for missing `$HOME` — read off docs.rs at U1.
  - `temp_env` version pin — latest stable on crates.io at U1 time.

---

## Implementation Units

### U1. Add `etcetera` dependency and `default_trace_dir()` helper

**Goal:** Land the `etcetera` crate in `Cargo.toml`, prove the integration on a fresh surface by
adding `default_trace_dir()`, and pin its behaviour with inline tests. The two existing helpers and
`resolve_xdg_base` are not touched in this unit — the transient state has two coexisting resolution
mechanisms for one commit, which is fine because R-PR2 only applies to the final state.

**Requirements:** R-PR5, R-PR6.

**Dependencies:** None.

**Files:**

- Modify: `Cargo.toml` — add `etcetera` to `[dependencies]` (bare version). If the recommended
  testing approach (option 1) is taken, also add `temp_env` to `[dev-dependencies]`.
- Modify: `Cargo.lock` — written by `cargo add`; commit alongside.
- Modify: `src/config.rs` — add `default_trace_dir()` plus its inline `#[cfg(test)]` unit tests in a
  new section. Do not interleave with the existing `resolve_xdg_base` tests at `:204-267` — those
  are slated for deletion in U2.

**Approach:**

- `cargo add etcetera` (no features needed — default features cover `Xdg`).
- If using `temp_env` for tests: `cargo add --dev temp_env`.
- Run `just deny` after the dependency add and **before** writing tests. Surface any audit failure
  as a U1 blocker (R-PR6); the user's pre-research suggests this won't happen, but the gate is
  `just deny`.
- Implement `default_trace_dir()`:
  - Construct an `Xdg` (mirror `etcetera`'s constructor; consult docs.rs for the exact entry point —
    likely `etcetera::base_strategy::Xdg::new()` or similar).
  - Resolve the state directory and join `lore/traces`.
  - Map any error from the constructor onto
    `anyhow::anyhow!("Cannot determine state directory: $HOME is not set.
    Use --config to specify a path.")`.

**Patterns to follow:**

- `src/config.rs::default_config_path` (`:128-137`) for the helper signature shape
  (`pub fn name() -> anyhow::Result<PathBuf>`) and the error context style. Match the existing
  module's vocabulary even though the body changes.

**Test scenarios:**

- `default_trace_dir_uses_xdg_state_home_when_set`: `XDG_STATE_HOME=/custom/state` → returns
  `/custom/state/lore/traces`.
- `default_trace_dir_falls_back_to_home_when_xdg_unset`: `XDG_STATE_HOME` unset, `HOME=/home/user` →
  returns `/home/user/.local/state/lore/traces`.
- `default_trace_dir_falls_back_to_home_when_xdg_empty`: `XDG_STATE_HOME=""`, `HOME=/home/user` →
  returns `/home/user/.local/state/lore/traces`. Pins XDG 0.8+ empty-string-as-unset semantics for
  the state tier; if `etcetera` does not honour this, U1 blocks until the normaliser fix lands.
- `default_trace_dir_home_unset_returns_error_mentioning_config`: `XDG_STATE_HOME` unset, `HOME`
  unset → error message contains `state` (purpose token), `$HOME is not set`, and `--config` (parity
  with R-PR4 wording shape).

**Verification:**

- `cargo test --features test-support config::tests` — four new scenarios pass.
- `just deny` — exits zero (R-PR6); no new `deny.toml` entries required.
- `just ci` — exits zero. (Required by repo convention; substitutes for running individual
  commands.)
- `grep -rn default_trace_dir src/` returns only `src/config.rs` matches — no premature caller wired
  up.

---

### U2. Swap `default_config_path` and `default_database_path` to `etcetera`; delete `resolve_xdg_base`

**Goal:** Migrate the two existing helpers to use `etcetera`'s `Xdg`, delete `resolve_xdg_base` and
its six inline tests at `src/config.rs:204-267`, and rewrite the deleted tests' coverage as fresh
scenarios exercising the public helpers via env-controlled inputs. The on-disk path layout for every
operator-reachable input is byte-identical before and after this unit (R-PR2 / R-PR3 / R-PR4).

**Requirements:** R-PR1, R-PR2, R-PR3, R-PR4.

**Dependencies:** U1 (the `etcetera` dependency and the implementation pattern proved by
`default_trace_dir`).

**Files:**

- Modify: `src/config.rs` — replace `default_config_path` and `default_database_path` bodies, delete
  `resolve_xdg_base` (lines `107-126`), delete the six inline tests at `:204-267`, write fresh
  inline tests covering the same behaviour through the public helpers.

**Approach:**

- Rewrite `default_config_path()`:
  1. Construct `Xdg` (mirror U1's pattern).
  2. Map any constructor error onto
     `anyhow::anyhow!("Cannot determine config directory: $HOME is not set.
     Use --config to specify a path.")`
     — verbatim from R-PR4.
  3. Return `<config_dir>.join("lore").join("lore.toml")`.
- Rewrite `default_database_path()` symmetrically with `data` purpose token and `lore/knowledge.db`
  suffix.
- Delete `resolve_xdg_base` (lines `107-126`) — no callers remain after the two helpers are rewired.
- Delete the six inline tests (`xdg_config_home_set`, `xdg_data_home_set`,
  `xdg_var_unset_falls_back_to_home`, `xdg_var_empty_falls_back_to_home`,
  `home_unset_returns_error`, `xdg_data_falls_back_to_home_local_share`) at `:204-267`.
- Write fresh inline tests using the chosen testing approach (see Key Technical Decisions). Coverage
  parity is the bar — every behaviour the deleted tests pinned must have an explicit assertion in
  the new suite.

**Patterns to follow:**

- U1's `default_trace_dir` for `etcetera` construction and error mapping.
- Coverage checklist (one-for-one with the deleted tests):
  1. `XDG_CONFIG_HOME` set → `<set>/lore/lore.toml`.
  2. `XDG_DATA_HOME` set → `<set>/lore/knowledge.db`.
  3. `XDG_CONFIG_HOME` unset → `$HOME/.config/lore/lore.toml`.
  4. `XDG_CONFIG_HOME` empty → `$HOME/.config/lore/lore.toml` (the named hard-constraint case from
     session start).
  5. `HOME` unset on `default_config_path()` → error contains `$HOME is
     not set` and
     `--config`.
  6. `XDG_DATA_HOME` unset → `$HOME/.local/share/lore/knowledge.db`.

**Test scenarios:**

Six scenarios, one per checklist item:

- `default_config_path_uses_xdg_config_home_when_set`: per checklist (1).
- `default_database_path_uses_xdg_data_home_when_set`: per checklist (2).
- `default_config_path_falls_back_to_home_when_xdg_unset`: per checklist (3).
- `default_config_path_falls_back_to_home_when_xdg_empty`: per checklist (4). **Critical edge case**
  — covers the `xdg_var_empty_falls_back_to_home` contract named in the user's hard constraints.
- `default_config_path_home_unset_returns_error_mentioning_config`: per checklist (5). Assertion
  shape: error contains `$HOME is not set` AND contains `--config` (literal substrings, matching the
  existing assertion style at `src/config.rs:253-259`).
- `default_database_path_falls_back_to_home_local_share_when_xdg_unset`: per checklist (6).

Optional symmetry test (`default_database_path_home_unset_returns_error_mentioning_config`) is
implementer's call — the deleted suite only covered the `config`-purpose-token case. Including it
adds half a test for a clearer contract; skipping mirrors prior coverage exactly.

**Verification:**

- `cargo test --features test-support config::tests` — all six new scenarios pass, plus the four
  from U1.
- `cargo test --features test-support --test init_output` — passes unchanged. This is the
  integration-level regression check for R-PR2 (the test spawns `lore init` with controlled
  `XDG_CONFIG_HOME` and `XDG_DATA_HOME` and asserts on the resulting on-disk file layout).
- `cargo test --features test-support --test smoke` — passes unchanged.
- `just ci` — exits zero.
- `grep -rn resolve_xdg_base src/ tests/` returns empty (no stragglers in source or tests).
  Historical mentions in `docs/plans/2026-04-05-001-doc-product-documentation-plan.md` and
  `docs/plans/2026-04-07-001-feat-coverage-check-skill-plan.md` are tolerated as time-snapshot
  context and do not need rewrites.

---

### U3. Documentation: `docs/configuration.md` recognises `XDG_STATE_HOME`; CHANGELOG entry under `Changed`

**Goal:** Add an `XDG_STATE_HOME` row to the Environment Variables table in `docs/configuration.md`,
and a single assertive-voice `[Unreleased] /
Changed` entry to `CHANGELOG.md`.

**Requirements:** R-PR7.

**Dependencies:** U2 (so the CHANGELOG entry reflects the as-shipped state).

**Files:**

- Modify: `docs/configuration.md` — add an `XDG_STATE_HOME` row in the Environment Variables table
  at `:202-208`, in the alphabetical-by-prefix position (after `XDG_DATA_HOME`).
- Modify: `CHANGELOG.md` — add one bullet under `[Unreleased] / Changed` (currently empty after the
  `[0.2.0]` cut at line 9).

**Approach:**

- New table row, mirroring the tone and structure of the existing three rows:
  - Variable: `XDG_STATE_HOME`
  - Purpose: `Override the state base directory`
  - Values: `Any absolute path`
  - Notes:
    `Defaults to $HOME/.local/state when unset or empty. Reserved
    for actions-history state under $XDG_STATE_HOME/lore/ — wired by Track
    2 Observability.`
    (Exact wording is implementer's call; must match the table's voice and end with a full stop.)
- Optional: add a brief sentence under `## File Paths` mentioning the state tier alongside the
  existing config and data tiers. Defer if it bloats the section — the env-var table row carries the
  load.
- CHANGELOG entry — one assertive sentence ending in `(#N)`. Draft framings (implementer picks one
  and refines):
  - State-tier first: XDG state tier (`$XDG_STATE_HOME`) is now reachable via a new
    `default_trace_dir()` helper, paving the way for Track 2 observability traces. (#N)
  - Internal-swap first: XDG path resolution moves to the `etcetera` crate; defaults and
    `$HOME`-fallback semantics are unchanged. (#N)
  - Hybrid: XDG state tier (`$XDG_STATE_HOME`) is now documented and reachable via a new
    `default_trace_dir()` helper; underlying resolution moves to the `etcetera` crate without
    behaviour change. (#N)

  **Bias toward the state-tier-first or hybrid framing** — the internal-swap detail alone is not
  user-facing per the CHANGELOG convention; the state-tier reachability is the forward-compatibility
  surface that warrants the entry.

**Patterns to follow:**

- `docs/configuration.md:202-208` — table shape, column alignment, prose tone of the existing three
  rows.
- `CHANGELOG.md` `[0.2.0] / Changed` precedent (lines `30-36`) — see the
  `lore ingest on a fresh git init...` and `Knowledge database schema
  bumped...` entries for the
  assertive-voice + `(#N)` shape.

**Execution note:** Run `dprint check` (or `just fmt`) after editing both files — markdown
formatting drift breaks the pre-commit hook and `just
ci`. `dprint` is pinned at `0.53.1` per the
project's tooling convention.

**Test scenarios:**

Test expectation: none — documentation-only unit, no behaviour change.

**Verification:**

- `dprint check` — exits zero on `docs/configuration.md` and `CHANGELOG.md`.
- `just ci` — exits zero.
- The `[Unreleased]` section now contains a single `Changed` entry; no `Added` / `Fixed` entries
  (none warranted).

---

## System-Wide Impact

- **Interaction graph:** `src/config.rs::default_*` helpers are called by `src/main.rs:144,214`
  only. No other in-tree callers (verified via `grep`). Internal-only swap; the public surface
  (`default_config_path`, `default_database_path` signatures, error wording, on-disk paths) is
  preserved.
- **Cross-platform parity:** macOS resolves XDG identically to Linux post-swap — the `Xdg` strategy
  explicitly opts out of platform-native paths. This matches the existing hand-rolled behaviour, so
  macOS operators with pre-existing `~/.config/lore/lore.toml` and
  `~/.local/share/lore/knowledge.db` continue to use them.
- **Binary size:** `etcetera` adds ~30-50 KB to the release binary, acceptable per the parked
  binary-size investigation memory and the user's hard-constraint list.
- **Integration coverage:** `tests/init_output.rs` and `tests/smoke.rs` already pin the end-to-end
  on-disk path contract via child-process env-control. They pass unchanged post-swap, serving as the
  regression check that R-PR2 holds through the full `lore init` flow.
- **Forward-compatibility:** Post-swap, adding a fourth XDG-resolved path (cache, runtime, anything
  else lore needs at 1.x or later) is a single helper-function addition, not a per-variable
  hand-roll. The pattern set by `default_trace_dir()` is the template.

---

## Sources & References

- **Origin:** `docs/brainstorms/2026-05-14-track-2-observability-requirements.md` —
  `Dependencies / Assumptions` (path-resolution refactor prerequisite) and `Key Decisions`
  (XDG-everywhere posture on macOS).
- **Target code:** `src/config.rs:107-148` (`resolve_xdg_base`, `default_config_path`,
  `default_database_path`); `src/config.rs:204-267` (tests being rewritten); `src/main.rs:8,144,214`
  (callers, unchanged).
- **Integration tests preserved:** `tests/init_output.rs:53-175`; `tests/smoke.rs:213`.
- **Tooling gates:** `deny.toml:11-22` (license allowlist); `justfile` `deny` and `ci` recipes;
  `dprint.json` (pinned at 0.53.1 per the project's tooling convention).
- **Conventions in memory:** `feedback_changelog_entries.md` (CHANGELOG voice + scope rule);
  `project_binary_size_investigation.md` (30-50 KB acceptable).
