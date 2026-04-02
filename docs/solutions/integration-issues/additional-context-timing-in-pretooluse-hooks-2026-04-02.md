---
title: "additionalContext in PreToolUse hooks is visible after tool composition, not before"
date: 2026-04-02
category: integration-issues
module: integrations
problem_type: integration_issue
component: tooling
symptoms:
  - "First Edit/Write does not follow injected conventions despite additionalContext being returned"
  - "Claude self-corrects on follow-up edit after seeing conventions in transcript"
root_cause: async_timing
resolution_type: workflow_improvement
severity: medium
tags:
  - claude-code
  - pretooluse
  - additional-context
  - hooks
  - timing
  - self-correction
---

# additionalContext in PreToolUse hooks is visible after tool composition, not before

## Problem

When building a PreToolUse hook that injects coding conventions via `additionalContext`, the
conventions are visible to Claude *after* the tool call has been composed, not before. Claude cannot
retroactively change the tool call it already constructed. This means the first edit in a new domain
may not follow the injected conventions.

## Symptoms

- First `Edit` or `Write` in a session does not follow injected conventions (e.g., uses `interface`
  instead of `type`, uses `function` instead of arrow functions)
- Claude self-corrects on the next edit after seeing the conventions in its transcript
- Conventions are reliably followed from the second edit onward

## What Didn't Work

- Expecting `additionalContext` to prevent the first suboptimal write (not possible given the timing
  model)
- Deny-first-touch (blocking the first edit with conventions as the deny reason) — works for
  compliance but creates infinite deny loops without session dedup

## Solution

Accept the timing model and leverage Bash reconnaissance as a natural workaround:

```
User: "Create a TypeScript error handler"

Claude's tool sequence:
  1. Bash(ls src/)           ← PreToolUse fires, injects TS conventions
  2. Edit(src/handler.ts)    ← PreToolUse fires again, but conventions are
                               already in transcript from step 1
```

The Bash reconnaissance call (e.g., `ls`, `cat`) that Claude typically makes before editing a file
fires PreToolUse first. The conventions injected during that Bash call enter the transcript. By the
time Claude composes the Edit call, the conventions are visible in its context from the Bash hook
response.

For the hook implementation, extract the last user message from the transcript file
(`transcript_path` field in hook input) and use it as enrichment terms for the search query. This
means even the first PreToolUse in a sequence benefits from the user's intent signal.

## Why This Works

Claude Code hooks fire synchronously before tool execution, but the `additionalContext` they return
becomes part of the tool call's *response context*, not its *composition context*. The agent sees
the injected content in its transcript *after* the tool runs, not while planning the next tool call.

The Bash-first pattern works because:
1. Agents naturally explore before editing (checking file existence, reading existing code)
2. Each tool call's `additionalContext` enters the transcript
3. By the time the Edit call is composed, prior hook responses are visible

## Prevention

- Design hook-based injection assuming a one-tool-call delay for first-time compliance
- If stronger first-write compliance is needed, implement deny-first-touch as a v2 feature (requires
  session dedup to avoid infinite loops — see the session dedup learning)
- Test injection effectiveness by prompting Claude to write code (not just "write a function" which
  may produce text, but "edit this file" or "create this file" which triggers tool calls)
