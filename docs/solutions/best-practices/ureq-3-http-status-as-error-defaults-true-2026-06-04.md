---
title: "ureq 3.x http_status_as_error defaults to true — non-2xx responses lose their body"
date: 2026-06-04
category: best-practices
module: embeddings
problem_type: best_practice
component: tooling
severity: high
applies_when:
  - "Upgrading from ureq 2.x to ureq 3.x"
  - "Calling an HTTP endpoint that returns diagnostic information in non-2xx response bodies (5xx error pages, JSON `error` fields)"
  - "Replacing send_json + read_json with a code path that needs to read the body when the status is not 2xx"
  - "Auditing existing ureq callers after a major-version dependency bump"
tags:
  - ureq
  - http
  - error-handling
  - rust
  - dependency-upgrade
  - response-body
---

# ureq 3.x http_status_as_error defaults to true — non-2xx responses lose their body

## Context

While building the Ollama runtime probe in PR #67 we needed to read the body of an HTTP 500 response
so the diagnostic could carry Ollama's own error string
(`error starting llama-server: llama-server binary not found ...`). The plan and one feasibility
reviewer both stated that ureq 3.x had removed ureq 2's "treat 4xx/5xx as `Err`" default — so
`send_json` would return `Ok(Response)` for 5xx and the caller could read the body.

Manual verification against the broken Homebrew Ollama bottle showed the opposite. The runtime line
rendered as `Runtime: ✗ transport error — http status: 500` instead of Ollama's actual diagnostic.
ureq 3.x **kept** the status-as-error default and renamed the toggle to `http_status_as_error`, with
`true` as the default. A 5xx response surfaces as `Err(ureq::Error::StatusCode(u16))` — without the
response body, which was already consumed and discarded by the time the error reaches the caller.

`Error::StatusCode(u16)` carries only the status code. There is no API on the error variant for
recovering the body. The only way to see it is to opt out of the short-circuit before the request
runs.

## Guidance

When a ureq 3.x caller needs to read the response body on non-2xx status, configure the agent once
at construction time:

```rust
let config = ureq::Agent::config_builder()
    .timeout_global(Some(Duration::from_secs(30)))
    .http_status_as_error(false)
    .build();
let agent = ureq::Agent::new_with_config(config);
```

With `http_status_as_error(false)`, `send_json` returns `Ok(Response)` for every HTTP status. The
caller must then check `response.status()` explicitly before parsing the body:

```rust
let mut resp = self.agent.post(&url).send_json(&req)?;
let status = resp.status().as_u16();
let body = resp.body_mut().read_to_string().unwrap_or_default();

if (200..300).contains(&status) {
    Ok(body)
} else if (500..600).contains(&status) {
    Err(MyError::ServerFailed { status, body })
} else {
    Err(MyError::HttpStatus { status, body })
}
```

The reconfiguration is per-agent, not per-request. If the same agent is shared across multiple
callers (the common case), every caller must check status manually — the agent will no longer
fail-fast on 4xx/5xx for any of them. This is a sibling-code-paths hazard (see Related); audit every
callsite of the reconfigured agent and confirm each one either funnels through the new
status-checking helper or has its own status check.

## Why This Matters

The default behaviour is subtly wrong for any caller that needs the response body:

- **Diagnostics get swallowed.** The body usually contains the most actionable error message
  (`llama runner process has terminated: ...`, `no space left on device`, application-level
  validation messages). The default config discards it before the caller can read it.
- **`read_json` failure looks like a parser bug.** A caller that does
  `send_json(...)?.body_mut().read_json::<T>()?` and runs against a 5xx will see a serde error like
  `"missing field \`embeddings\`"` because the body was the error page, not the success shape. The
  actual diagnostic is invisible.
- **Migration hazard.** Code that worked on ureq 2.x (which also defaulted to status-as-error)
  continues to work on ureq 3.x — except the toggle was renamed, and any code written under the
  belief that ureq 3 had changed the default will silently lose the body.

The trap is that the surface API is unchanged: `send_json` exists, `read_json` exists, both behave
correctly on 2xx. The difference only shows up on the error path, which by definition is the path
where you most want the diagnostic.

## When to Apply

Configure `http_status_as_error(false)` when:

- The endpoint returns useful information in non-2xx bodies (most JSON APIs, anything that returns
  `{"error": "..."}` shapes, anything that returns multi-line stack traces)
- The caller needs to classify failure modes (5xx server-side vs 4xx client-side vs other)
- The body's content drives the rendered error message users see

Leave the default `true` when:

- The endpoint's non-2xx response body is genuinely uninformative (a generic HTML error page, empty
  body, opaque code)
- The caller only needs to know "did it succeed" and a status code is sufficient
- A surrounding retry/circuit-breaker wrapper consumes only the status code

When configuring `false`, audit every existing callsite of the agent. The semantic change is global
— silent in code review but visible at runtime as "this endpoint used to throw, now it returns
success with a 500-shaped body."

## Examples

### Before (silently swallows the diagnostic)

```rust
let resp: EmbedResponse = self
    .agent
    .post(&url)
    .send_json(&req)?           // succeeds for 2xx, returns Err(StatusCode) for 5xx without body
    .body_mut()
    .read_json()?;              // never runs on 5xx, body is gone
```

On a 5xx, the `?` short-circuits with `ureq::Error::StatusCode(500)` — no diagnostic body, no way to
recover it.

### After (caller sees the body and classifies the failure)

```rust
let mut resp = self.agent.post(&url).send_json(&req)?;
let status = resp.status().as_u16();
let body = resp.body_mut().read_to_string().unwrap_or_default();

if (200..300).contains(&status) {
    let parsed: EmbedResponse = serde_json::from_str(&body)?;
    Ok(parsed.embeddings)
} else if (500..600).contains(&status) {
    Err(ProbeError::RunnerFailed { status, body })  // body contains real diagnostic
} else {
    Err(ProbeError::HttpStatus { status, body })
}
```

The agent must be constructed with `http_status_as_error(false)` for this code path to fire. With
the default, `send_json?` errors on 5xx before the manual status check runs.

### Audit grid

When reconfiguring a shared agent, build the audit grid before merging:

| Callsite                   | Status check after `send_json?`                 | Body usage on non-2xx             |
| -------------------------- | ----------------------------------------------- | --------------------------------- |
| `OllamaClient::probe`      | yes, via `classify_embed_response`              | reads, parses JSON `error`        |
| `Embedder::embed`          | yes, via `classify_embed_response`              | reads, parses JSON `error`        |
| `OllamaClient::pull_model` | yes, inline `status.is_success()`               | reads, returns as `anyhow::bail!` |
| `OllamaClient::is_healthy` | yes, `is_ok_and(\|r\| r.status().is_success())` | n/a (only status matters)         |
| `OllamaClient::has_model`  | yes, `is_ok_and(\|r\| r.status().is_success())` | n/a (only status matters)         |

A "no" in the second column means the caller will now treat a 5xx as success — a silent regression
introduced by the agent reconfiguration.

## Related

- [`sibling-code-paths-can-reintroduce-fixed-failure-modes-2026-05-19.md`](sibling-code-paths-can-reintroduce-fixed-failure-modes-2026-05-19.md)
  — exact shape of the audit needed when reconfiguring a shared agent. Every callsite is a sibling
  code path that inherits the new contract.
- [ureq 3.x docs on `http_status_as_error`](https://docs.rs/ureq/3/ureq/config/struct.ConfigBuilder.html#method.http_status_as_error)
  — the toggle. Default is `true`.
- `src/embeddings.rs` in lore — `OllamaClient::new` configures `http_status_as_error(false)`;
  `classify_embed_response` is the shared status-checking helper that all callers route through.
