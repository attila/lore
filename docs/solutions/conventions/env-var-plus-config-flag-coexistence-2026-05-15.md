---
date: 2026-05-15
topic: env-var-plus-config-flag-coexistence
status: convention
---

# Env-var-plus-config-flag coexistence

Ground rules for boolean toggles in lore that warrant both a persistent `lore.toml` flag and a
per-session environment-variable override. Derived from `LORE_DEBUG` (debug logging) and
`LORE_TRACE` (Track 2 Observability).

## When to expose both surfaces

Expose a config flag and an env-var override together when the toggle benefits from:

- **A persistent default** — operators set the behaviour once and forget it (e.g. "tracing is always
  on for my dogfood install").
- **A per-session override** — operators flip the behaviour for one shell without editing
  `lore.toml` (e.g. "let me trace this one session to debug a bug").

If only one of these properties is meaningful, expose only one surface. Don't add an env-var
override "just in case" — every extra knob is one more shape an operator has to learn.

## Precedence

**Env var wins over config.** When the env var is recognised, it overrides the config value for the
lifetime of that process. When unset or unrecognised, the config value applies.

This precedence is the only legal direction. Inverting it (config wins over env) makes per-session
override impossible without editing `lore.toml`, which defeats the env-var's purpose.

## Parsing contract

Truthy and falsy values are case-sensitive ASCII tokens:

- Truthy: `1`, `true`, `yes`
- Falsy: `0`, `false`, `no`

Any other value, including the empty string and case-variant tokens like `TRUE` or `Yes`, is treated
as unset and silently falls through to the config value. No stderr warning, no debug log — the
convention is fail-soft and relies on the operator getting the syntax right. Documented as a fixed
set so shell users can rely on a stable contract.

Why no broader truthy detection? Locking the parser to a known three-by-three set keeps shell-script
behaviour predictable and matches the discipline already in place at `src/debug.rs:11-15`. Liberal
truthy parsing (e.g. accepting `on`, `enabled`, or non-zero integers) produces inconsistency across
shells, scripts, and CI runners that quote env vars differently.

## Force-off form

`LORE_X=0` (or `false` / `no`) is a first-class per-session force-off. An operator who has
`[trace] enabled = true` in their config but wants to disable tracing for one shell runs
`LORE_TRACE=0 lore …` and the override applies for that process only.

This dual-direction override is the whole point of having both surfaces. A truthy-only env var
(where unset == "use config" but no value means "off") breaks the symmetry and forces operators to
either remove the env var or edit the config.

## Naming

- Config key: lowercase, under a section matching the feature (e.g. `[trace] enabled = true`).
- Env var: `LORE_<FEATURE>` in screaming snake case, where `<FEATURE>` matches the config section
  name (e.g. `LORE_TRACE` for `[trace]`).

Don't suffix the env var with `_ENABLED` or `_ON` — the parsing contract already encodes the
boolean, and the section name carries the noun.

## Documented examples

- `LORE_DEBUG` overrides no config flag (debug has no persistent surface) — the parsing contract
  above is its established shape and the canonical reference for new toggles.
- `LORE_TRACE` overrides `[trace] enabled` (Track 2 Observability) — the first toggle that
  materially exercises this convention.

Future toggles that share the persistent-plus-override profile inherit the parsing contract and
precedence without re-deciding from scratch.
