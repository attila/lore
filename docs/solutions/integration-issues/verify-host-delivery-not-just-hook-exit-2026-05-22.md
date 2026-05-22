---
title: "Verify host delivery, not just hook exit — successful hook execution is not evidence the model received the payload"
date: 2026-05-22
category: integration-issues
module: integrations
problem_type: integration_issue
component: tooling
symptoms:
  - "Hook exits 0 and harness logs hook_success, but the model behaves as if the payload was never injected"
  - "Terminal renders a chip from the hook output, masking the absence of model-side delivery"
  - "The bug persists across every workspace and every session for the integration's lifetime"
root_cause: silent_channel_misroute
resolution_type: testing_discipline
severity: high
tags:
  - claude-code
  - sessionstart
  - postcompact
  - additional-context
  - system-message
  - hooks
  - integration-testing
  - silent-failure
---

# Verify host delivery, not just hook exit

## Problem

Lore's `SessionStart` hook returned `{"systemMessage": "..."}` since the integration shipped. Claude
Code's harness routes `systemMessage` only to the terminal UI as a transient chip — the model never
sees it. The hook process exited `0`, the harness logged `hook_success`, the terminal rendered the
chip (sometimes), and every signal upstream of the model said "working." The pinned-conventions
index and the meta-instruction were invisible to the agent in every workspace, on every
`SessionStart` source (`startup`, `resume`, `clear`, `compact`), for the lifetime of the
integration.

The bug was found accidentally: the user noticed the chip rendering looked inconsistent across
`SessionStart` sources, which surfaced the channel question, which led to a model-side test that
asked the agent to quote the payload — and it couldn't.

## Symptoms

- `lore hook` process exits `0`, `stdoutLen` is non-zero in the transcript jsonl, `hook_success` is
  logged at every event.
- Terminal renders a chip for the payload on at least some events (often the ones with a quieter
  screen — `/clear`, `/compact`).
- The model exhibits no awareness of the pinned conventions across sessions — agents repeatedly
  violate conventions that the user knows are configured.
- The transcript jsonl records `content: ""` on the hook attachment row even though the hook stdout
  was non-empty.

## What Didn't Work

- Reading the harness's printed hook-output schema dump on validation failure. The dump is
  incomplete — it omits the `hookSpecificOutput` variants that work in practice for `SessionStart`,
  so matching the dump literally produces a wrong envelope for some events.
- Treating `exitCode=0` plus terminal chip rendering as evidence of model- side delivery. The chip
  is a separate channel; its visibility tells you nothing about what the model received.
- Inspecting the hook's stdout for "correct-looking" JSON. Both `{"systemMessage": "..."}` and
  `{"hookSpecificOutput": {...}}` are syntactically valid; only the latter (and only on the right
  events) reaches the model.

## Solution

For every host-integration channel that targets a specific consumer (typically the model in agent
harnesses), the only authoritative test is asking the consumer "did you receive it?" before the
integration ships.

For Claude Code hooks specifically, paste this in a fresh session before any tool call:

```
Before doing anything else — no tool calls, no MCP, no file reads — tell me
verbatim: do you see, in your initial context, a system message that begins
with the words "<first words of the payload>"? Quote the first sentence back
if yes; say "no" if not.
```

Repeat across every event source the hook fires on (`startup`, `resume`, `clear`, `compact`). The
acceptance prompt costs one session per event and gives an unambiguous model-side answer.

For non-agent host integrations (webhooks, message queues, plugin APIs), the equivalent is a
synthetic consumer that records what it received and asserts the payload matches expectation. The
shape of the test changes per host; the discipline does not.

## Why This Matters

Host integrations have multiple plausible delivery channels, and "the host didn't reject my call" is
a weak proof of delivery when the channels look syntactically interchangeable. Claude Code's
`systemMessage` and `hookSpecificOutput.additionalContext` are both top-level JSON keys with similar
shapes; both produce a `hook_success` on the harness side; both render output that a user might see.
Only one reaches the model. The asymmetry is structural to the host and invisible to upstream
signals.

The lifetime of this bug — the integration's entire history — is the cost of trusting upstream
signals. The fix took an hour; the discovery took months of agent behaviour that was misattributed
to model capability rather than missing context.

## Prevention

- For any hook, plugin, or host integration where the payload targets a downstream consumer: write a
  model-side (or consumer-side) acceptance test before ship, and re-run it after any envelope,
  schema, or routing change in the host.
- When the host publishes multiple output channels (chip vs context, log vs reply, etc.), document
  explicitly which channel each event uses and why. A table in the integration doc beats per-event
  scattered comments.
- Be suspicious of host-published validator schemas. Reproduce the accepted shape empirically when
  the documented schema looks incomplete or self-contradictory.
- Distrust "everything looks fine" when the consumer's behaviour is the only thing that can actually
  confirm delivery.

## Related

- `agents/claude-code-hook-output-envelopes.md` in the lore-patterns repository captures the
  per-event envelope acceptance map for Claude Code hooks specifically, including the `PostCompact`
  rejection that followed from this same investigation.
- `integration-issues/additional-context-timing-in-pretooluse-hooks-2026-04-02.md` documents a
  different shape of "hook fires but effect is not what you expect" for the `PreToolUse` event.
