---
title: "Expensive CLI diagnostics go behind an opt-in flag, with discoverability rescue mechanisms"
date: 2026-06-04
category: conventions
module: cli
problem_type: convention
component: tooling
severity: medium
applies_when:
  - "Adding a diagnostic check that takes longer than ~1 second to a sub-second CLI command"
  - "The check catches a failure mode that fails silently — degraded fallback, ignored warning, hidden incorrectness"
  - "Designing a CLI status, health, or doctor command where users may have multiple intents (quick check vs deep verification)"
  - "Reviewing a design that proposes making a slow check default-on for discoverability reasons"
tags:
  - cli
  - opt-in
  - diagnostics
  - discoverability
  - default-behaviour
  - unix-conventions
---

# Expensive CLI diagnostics go behind an opt-in flag, with discoverability rescue mechanisms

## Context

PR #67 added a runtime probe to `lore status` that catches a silent failure mode — Ollama reports
installed + running + model-pulled, but inference is broken because the bundled runner binary is
missing. The probe takes 3-15 seconds on a cold model load. The initial design made the probe
default-on, reasoning that users wouldn't know to opt into a check they didn't know existed.

A product-level review pushed back: turning a sub-second command into a multi-second one penalises
every routine status invocation (config sanity, daemon-running check, "is the database still there")
in service of the much rarer silent-degradation case. Unix convention keeps surface checks fast and
gates deeper checks behind explicit flags — `ls` vs `ls -l`, `git status` vs
`git status --branch -v`, `nmap` vs `nmap -A`.

The default-on objection is real but the alternative seemed worse: users who never opt in get
nothing, so the silent-degradation problem stays unsolved.

The shape that landed has the opt-in flag plus three rescue mechanisms that route users to the
deeper check at the moments they actually need it — without paying the cost on routine calls.

## Guidance

When adding an expensive diagnostic to a CLI command, structure the design as:

1. **Default mode stays fast.** The base command preserves its existing latency budget. No new
   blocking I/O, no new network calls, no new disk reads beyond what was already there.

2. **Deeper check is opt-in via an explicit flag.** `--full`, `--deep`, `--verify`, or similar. The
   flag name should read naturally in the sentence "run X --full when Y." Avoid exposing
   implementation vocabulary (`--probe`, `--check-inference`).

3. **Default output advertises the flag.** A single discoverability hint line in the default output
   tells the curious user the deeper check exists. The line costs nothing — no probing — and reads
   naturally next to the other status lines.

   ```text
   Runtime:      —  (run 'lore status --full' to verify inference)
   ```

4. **Failure paths nudge toward the flag.** When the silent-degradation case fires in real use,
   surface a one-line warning that names the failure class and points at the deeper check. This is
   the push-side mechanism that catches users who never read the default status output but do see
   warnings during real work.

   ```text
   Warning: Ollama embed failed (inference error); falling back to text search.
   Run 'lore status --full' for details.
   ```

5. **Provisioning runs the check automatically.** Install or first-time-setup commands already wait
   on slow operations (model pull, schema migration, dependency check). A multi-second runtime
   verification is the right cost there, and it's the moment the user is most receptive to a slow
   check.

The four mechanisms together — fast default, hint line, push warning, auto-probe at install — serve
every realistic user path. Users who run the status check casually pay nothing. Users who actively
debug see the hint. Users who never opt in but use the tool through hook-driven or background paths
get nudged through the warning. Users who freshly install verify end-to-end at install time.

## Why This Matters

The default-on alternative looks user-friendly on paper but degrades the most common path — the
quick health check — in service of an edge case. The slow command becomes the only command and
routine users feel the cost continuously.

The opt-in alternative without rescue mechanisms looks lazy — "we shipped a flag, you're on your own
to find it" — and fails the silent-degradation case it was added to address.

The structured opt-in design splits the difference: every user path has a mechanism that catches
their failure mode at the moment they need it, and no user pays the cost they don't need to.

Three specific properties make this work:

- **The hint line is information, not interruption.** It uses a neutral em-dash, sits in the same
  column as other status lines, and reads as "this is something you could check" rather than "this
  is something you should check." Curious users follow it; uninterested users skim past it.
- **The push warning is rate-limited.** Without rate-limiting, hook-driven failures spam the warning
  hundreds of times per day and the user trains themselves to filter the pattern. With per-process
  dedup by failure class, the first warning of each class is the only one — enough to be visible,
  infrequent enough to not become wallpaper.
- **The install-time auto-probe is the safety net for fresh setups.** A user who never reads status
  output and never sees warnings still verifies inference end-to-end as part of `init`, because they
  explicitly waited on it.

The acknowledged residual: a user who provisioned six months ago, runs the tool through hook paths
only, ignores warnings (or routes stderr to `/dev/null`), and never invokes the status command will
not learn about the deeper check until something forces them to debug. This is acceptable collateral
— three rescue mechanisms cover the realistic paths; the residual user has already opted out of
every signal the design can surface.

## When to Apply

Apply this convention when:

- A CLI command currently completes in well under a second and a proposed addition would push it
  past several seconds
- The check is genuinely expensive enough to bother users (model load, network probe, filesystem
  walk over a large tree, schema verification)
- The failure mode the check catches is currently silent or only-loud-during-active-use
- The command is invoked routinely (as opposed to once-per-install)

Skip the opt-in structure when:

- The check is fast enough to fold into the default without anyone noticing (sub-100ms is the rough
  cutoff)
- The command is rarely invoked anyway (a once-per-installation `doctor` subcommand, an explicit
  `verify` command) — the check is already opt-in by virtue of the surrounding command being opt-in
- The failure mode is already loud during normal use and the deeper check is just confirming what
  the user already knows
- The user has indicated they want a thorough check (a `--strict` mode that's already expected to be
  slower)

## Examples

### `lore status` — the structure from PR #67

```text
$ lore status              # sub-second — default
  ...
  Model:        ✓ nomic-embed-text
  Runtime:      —  (run 'lore status --full' to verify inference)
  ...

$ lore status --full       # opt-in deeper check, 3-15s on cold load
  ...
  Model:        ✓ nomic-embed-text
  Verifying inference runtime (may take a few seconds)…
  Runtime:      ✗ inference failed — error starting llama-server: llama-server binary not found ...
  ...
```

The hint line in default mode advertises the flag without paying its cost. The header lines in
`--full` mode print before the probe blocks, so the multi-second wait is visible progress, not a
frozen terminal. The probe is invoked from `provision()` (during `lore init`) automatically
regardless of the flag.

The push-side rescue lives in the hook FTS-fallback warning:

```text
Warning: Ollama embed failed (inference error); falling back to text search.
Run 'lore status --full' for details.
```

A user who only ever runs `lore` through Claude Code hooks (never the status command directly) sees
this in their stderr the first time inference fails. The classification (`inference
error`,
`timed out`, `transport error`, `HTTP <status>`) tells them what kind of problem it is; the `--full`
pointer tells them where to look for the full diagnostic.

### Generic shape for other commands

| Default fast command                                         | Opt-in deeper check     | Rescue mechanism #1 (hint)     | Rescue mechanism #2 (push)                                                | Rescue mechanism #3 (install)                     |
| ------------------------------------------------------------ | ----------------------- | ------------------------------ | ------------------------------------------------------------------------- | ------------------------------------------------- |
| `myapp status`                                               | `myapp status --full`   | Hint line in default output    | Warning on background-task failure mentions `--full`                      | `myapp init` runs the check automatically         |
| `myapp doctor` (already opt-in via being a separate command) | `myapp doctor --strict` | Hint in `doctor` output        | Failure mode warnings throughout the app point at `myapp doctor --strict` | `myapp init` runs `doctor` checks at install time |
| `myapp lint`                                                 | `myapp lint --thorough` | Hint in default `lint` summary | Failed CI runs point at `--thorough`                                      | Pre-commit hook runs `lint` (already opt-in)      |

The shape transfers across commands. The hint-line wording and the push-warning text are where each
command differentiates.

### Anti-pattern: opt-in without rescue mechanisms

```text
$ myapp status            # fast, but no signal that --full exists
  ...all green...

$ myapp ingest            # works, silently produces broken output
  Done.
```

The user has no way to learn that `--full` exists until they actively go looking. The opt-in shape
is correct but the discoverability mechanisms are missing. Add at least the hint line and the
push-warning before considering this design done.

## Related

- [`cli-behaviour-ladder-2026-05-10.md`](cli-behaviour-ladder-2026-05-10.md) — the broader
  convention this fits inside: which tier of CLI feedback (silent, warn, error, abort) a given
  failure mode belongs in. The opt-in-with-rescues design is the codified shape for tier-2-style
  failures that need to stay loud without being default-blocking.
- [`cli-data-commands-should-output-to-stdout-2026-04-02.md`](../best-practices/cli-data-commands-should-output-to-stdout-2026-04-02.md)
  — adjacent CLI convention. The hint line and push warning both go to stderr (diagnostic),
  preserving the stdout-for-data discipline.
- `src/main.rs::cmd_status` and `src/hook.rs::emit_embed_failure_warning` in lore — the reference
  implementations of the hint-line and push-warning mechanisms.
- `src/provision.rs::provision` — the install-time auto-probe (mechanism #3).
