---
title: "feat: probe Ollama runtime in `lore status`"
status: active
created: 2026-06-04
type: feat
depth: lightweight
---

# feat: probe Ollama runtime in `lore status`

## Summary

`lore status` currently reports Ollama as healthy when the server is reachable
and the model manifest is on disk, even when the bundled runner is missing or
unloadable. The current Homebrew bottle reproduces this: `/api/show` succeeds,
but any real inference call fails because Ollama can't spawn its runner
subprocess.

Crucially, `lore` does not treat inference failures as hard errors — the hook
path at `src/hook.rs:676` catches them, emits a one-line stderr warning, and
falls back to FTS. So a user can run with broken inference for weeks getting
silently degraded search results, never realising vector search is disabled.

Add a runtime probe — a minimal embed call against the configured model — and
expose it as an **opt-in deeper check** via `lore status --full` (U1 + U2),
not as a default-on regression of the existing fast status command. Default
`lore status` retains its current sub-second behaviour and gains a
discoverability hint line pointing at the deeper check. The probe also runs
automatically during provisioning (`lore init`), where the multi-second
cost is dwarfed by model pull time and a deep verification is exactly what
the user wants.

Separately, upgrade the FTS-fallback warning so the push-side surface is
informative and points at `lore status --full` for full diagnosis (U3).
Between the discoverability hint, the hook-warning nudge, and the
provisioning-time check, the silent-degradation case is still caught
without penalising routine `lore status` calls.

The probe returns a structured error so different failure modes
(runner-failed, timeout, transport) get accurate status messages, and uses
`keep_alive: 0` on the status path so the deeper check does not pin a
270 MB model in RAM after every invocation.

---

## Problem Frame

The existing status pipeline uses three checks, all of which are metadata-only:

- `check_ollama_binary()` — `ollama --version` exit status (CLI present on PATH).
- `OllamaClient::is_healthy()` — `GET <host>/` returns 200 (`ollama serve`
  responding).
- `OllamaClient::has_model()` — `POST /api/show` returns 200 (manifest exists
  on disk).

None of these load the model or invoke the runner subprocess. The runner is
shipped inside the Ollama app bundle, and recent Homebrew layouts ship a
broken runner-discovery path. The breakage only surfaces when something asks
for inference (e.g., `lore ingest`, `lore search`), at which point `lore`
fails noisily while `lore status` still reports a clean ✓✓✓.

We want `lore status` to be the diagnostic surface that flags this — that's
the whole reason the command exists.

---

## Requirements

- **R1.** `lore status` must distinguish "model manifest exists" from "model
  is loadable by the runner."
- **R2.** Runtime probe must use a real inference path (the embed endpoint) so
  it exercises the same code path ingest/search would later hit.
- **R3.** Probe must not depend on local patterns/state — it must run against
  whatever model is configured in `config.ollama.model`.
- **R4.** Probe failure must produce a CLI message specific enough that a user
  hitting the current Homebrew bottle bug recognises the diagnosis (mention
  the runner binary so the next search is productive).
- **R5.** Probe runs automatically in `provision()` (where it gates "ready
  for ingest"). In `check_status()` (the read-only diagnostic), the probe
  runs only when the caller passes the `--full` flag from
  `lore status --full`. Default `lore status` does not run the probe and
  retains its current sub-second latency; it displays a discoverability
  hint line pointing at `--full`.
- **R6.** Probe must time out within the existing 30 s global timeout — no
  separate timeout knob needed.
- **R7.** Hook-emitted FTS-fallback warnings (`src/hook.rs:676`) must
  satisfy two observable conditions: (1) name the failure class from the
  fixed five-element `ProbeError` set ("inference error", "timed out",
  "transport error", `HTTP <status>`, or a degenerate-fallback string),
  and (2) point the user at `lore status --full` for full diagnosis.

---

## Scope Boundaries

In scope:

- New `OllamaClient::probe(keep_alive_secs)` method invoking `/api/embed`
  with a minimal input and a structured `ProbeError` return type.
- New `runtime: ProbeOutcome` field on `ProvisionResult` (replaces the
  earlier draft's bare `runtime_ok: bool`).
- New `--full` flag on `lore status` that opts into the runtime probe.
- New status line in `lore status` output (stderr) — a discoverability
  hint in default mode, and the full `ProbeOutcome` rendering in `--full`
  mode.
- Updated provisioning failure messages that surface Ollama's actual error
  body rather than asserting a fixed runner-bundle diagnosis.
- Integration test exercising the probe against a real Ollama instance.
- Upgraded hook-path FTS-fallback warning at `src/hook.rs:676` that names
  the failure class and points at `lore status --full` (U3).

### Deferred to Follow-Up Work

- Auto-recovery hints specific to the Homebrew breakage (e.g., "try
  `brew reinstall ollama`"). The runner-bundle bug may be fixed upstream
  before we'd ship that advice; flagging the failure precisely is enough for
  now.
- Probing alternative endpoints (`/api/generate`) for non-embed models —
  `lore` only consumes embeddings, so the embed probe matches the actual
  usage.
- Persisting last-known probe state to the database so default `lore status`
  can show the runtime line without running a fresh probe. Considered as a
  third rescue mechanism but deferred — the discoverability hint plus the
  hook warning plus `lore init` cover the silent-degradation case
  adequately, and adding persistence introduces stale-cache concerns
  (when is the cache too old to trust?) that the current design avoids.

### Out of Scope

- Changing the existing `is_healthy` / `has_model` semantics. They keep their
  current meaning (reachable / manifest present); the probe is additive.

---

## Key Technical Decisions

- **Why the probe is opt-in (`--full`), not default-on.** Adding the
  probe to default `lore status` would turn a sub-second command into a
  3–15-second one, penalising every routine status check (config sanity,
  database presence, daemon reachable). Unix convention is to keep
  surface checks fast and gate deeper checks behind an explicit flag
  (`ls` vs `ls -l`, `git status` vs `git status --branch -v`). The
  default-on alternative was considered and rejected as a small product
  overreach: it privileges "catch silent degradation proactively" over
  the everyday-user experience, and the silent-degradation goal is
  already served by three rescue mechanisms that do not require
  default-on probing:
  1. **Discoverability hint.** Default `lore status` displays a single
     non-probing line: `Runtime:      —  (run 'lore status --full' to verify inference)`.
     Curious users see the deeper check exists without paying for it.
  2. **U3 hook warning.** When inference fails during real search, the
     warning fires automatically and routes users to `lore status --full`
     at the moment they need it.
  3. **Provisioning runs the probe automatically.** `lore init` already
     waits on a model pull (potentially minutes); a multi-second
     inference verification is the right cost there, and the user
     explicitly wants the deeper check.
- **Why `lore status` at all, not just better `embed` error messages.**
  Embed failures in the hook path (`src/hook.rs:676`) currently degrade
  silently: one stderr warning, then FTS fallback. They are not hard
  failures the user investigates — they are background noise the user
  tunes out. So "just improve `embed`'s error context" does not actually
  solve the problem: the user still ignores the warning. The combination
  of a discoverable opt-in deep check (`lore status --full`) plus a louder
  hook warning (U3) gives users both a proactive verification surface and
  a reactive nudge — neither alone is sufficient.
- **Probe via `/api/embed`, not `/api/generate`.** The configured model is an
  embedding model (`nomic-embed-text` by default, plus `mxbai-embed-large`,
  `snowflake-arctic-embed2`, `all-minilm` per `OllamaClient::dimensions`).
  Embedding models do not implement `/api/generate` and would fail the probe
  for the wrong reason. `/api/embed` is exactly the call ingest will make.
- **Probe input is a single short string (e.g., `"."`).** A short, well-formed
  token avoids tokenisation edge cases. The dominant cost of an embed call is
  model load and kernel warmup, not bytes on the wire — so this is the
  cheapest *embed*, not the cheapest *probe*. We accept the cold-load cost
  because the probe needs to exercise the same code path ingest would later
  hit (R2).
- **Probe returns a structured error, not a bare boolean.** Bare
  `runtime_ok: bool` collapses three distinct failure modes — runner
  failure (HTTP 5xx from the embed endpoint), timeout, and transport error —
  into one. The user-facing message and the status line need to distinguish
  them, otherwise a slow first-load (cold-disk page cache) gets misdiagnosed
  as a runner bundle bug. Probe returns `Result<(), ProbeError>` (a small
  enum: `RunnerFailed(String body)`, `Timeout`, `Transport(String)`,
  `HttpStatus(u16, String body)`), and `ProvisionResult` carries
  `runtime: ProbeOutcome` (an enum with `Ok`, `Skipped`, `Failed(ProbeError)`)
  rather than a bare `bool`. The status renderer and the
  `result.errors` / `result.actions` strings derive from the variant.
- **Probe gates `model_available`'s meaning, not its truth.** Keep
  `model_available` as "manifest present" so status output can distinguish
  "model not pulled" from "model pulled but runtime broken" — the latter
  shows `model_available=true, runtime=Failed(…)`, and the message points
  at the underlying Ollama error.
- **Status-path probe passes `keep_alive: 0`; provisioning probe does not.**
  Ollama loads the model on `/api/embed` and pins it in RAM for
  `OLLAMA_KEEP_ALIVE` (default 5 minutes). For `provision()`, that warm
  cache is a feature — the user is about to ingest, so the load amortises.
  For `check_status()` invoked via `lore status --full`, pinning ~270 MB
  of model in RAM after a single diagnostic call is wasteful (the user
  isn't about to do an ingest; they're checking health). `/api/embed`
  accepts `keep_alive: 0` in the request body to unload immediately after
  the call. `probe()` takes a `keep_alive_secs: Option<u64>` argument so
  the two call sites pick the right behaviour. `check_status()` passes
  `Some(0)`; `provision()` passes `None`.
- **Surface Ollama's actual error body, not a fixed diagnosis string.**
  Many failure modes look identical at the manifest level but differ
  underneath: missing runner binary, out-of-disk-space, GPU/Metal init
  failure, version mismatch, VRAM exhaustion, sandbox-blocked exec. The
  probe-failure message is built from the error body Ollama returns, with
  a Homebrew-bottle hint appended as one possibility. We do not hard-code
  `llama-server` (stale llama.cpp terminology); the modern Ollama runner
  appears in logs as `llama runner process`.
- **No mock HTTP layer for unit tests.** The crate has no `mockito`/`wiremock`
  dependency; existing Ollama tests are real-server integration tests gated
  by `just test-integration`. Stay consistent: cover the probe via
  integration test rather than introducing a mock framework for a one-method
  surface.

---

## Implementation Units

### U1. Add `OllamaClient::probe()` and `runtime` outcome on `ProvisionResult`

**Goal:** Introduce the runtime probe and surface its outcome on the result
struct so callers can branch on it.

**Requirements:** R1, R2, R3, R6.

**Dependencies:** none.

**Files:**

- `src/embeddings.rs` (add `probe` method and `ProbeError` enum on
  `OllamaClient`)
- `src/provision.rs` (add `runtime: ProbeOutcome` field on `ProvisionResult`;
  populate in `check_status` and `provision`)
- `tests/ollama_integration.rs` (new test exercising the probe)

**Approach:**

- `OllamaClient::probe(keep_alive_secs: Option<u64>)` issues
  `POST {host}/api/embed` with `model = self.model`,
  `input = vec![".".to_string()]`, and (when `keep_alive_secs` is `Some`)
  `keep_alive = <n>` in the request body. Extend `EmbedRequest` with an
  optional `keep_alive` field serialised with `#[serde(skip_serializing_if =
  "Option::is_none")]` so existing call sites (`Embedder::embed`) keep their
  current wire shape.
- Probe returns `Result<(), ProbeError>` where `ProbeError` is a small enum
  defined in `embeddings.rs`:
  - `RunnerFailed { status: u16, body: String }` — HTTP 5xx from Ollama.
    This is the **verified** failure shape for the broken-Homebrew runner
    case: Ollama returns 500 with a body like
    `{"error":"llama runner process has terminated: ..."}`. The body string
    is the human-facing diagnostic.
  - `Timeout` — ureq's timeout variant.
  - `Transport(String)` — catchall for all other ureq error variants
    (`Io`, `ConnectionFailed`, `HostNotFound`, `Decompress`, etc., plus any
    future `#[non_exhaustive]` additions). Implementer note: the mapping
    arm is `_ => ProbeError::Transport(e.to_string())` — explicit so
    future ureq variants compile without structural change.
  - `HttpStatus { status: u16, body: String }` — non-2xx, non-5xx (rare;
    catches misconfigured proxies returning 4xx).
  - `InferenceError { body: String }` — **hypothesised** shape: HTTP 200
    with a JSON body containing an `error` field. Some Ollama failure
    modes on `/api/chat` and `/api/generate` use this shape; whether the
    embed path ever does is not verified. Defensive coverage — keep the
    branch, but the broken-Homebrew acceptance test is expected to fire
    `RunnerFailed`, not this variant.
- **ureq 3.x mechanics (load-bearing for the implementer):** ureq 3.x does
  *not* short-circuit non-2xx responses through `?`. `send_json()` returns
  `Ok(Response)` for HTTP 5xx; the existing `Embedder::embed` (which calls
  `read_json::<EmbedResponse>` directly) currently fails with an opaque
  serde "missing field `embeddings`" error on the broken-runner 500
  response, which is why the hook warning is uninformative. Probe must:
  1. Call `send_json()` and bind the response.
  2. Check `response.status()` first.
  3. On non-2xx: read body as text via `body_mut().read_to_string()`, map
     to `RunnerFailed` (5xx) or `HttpStatus` (other), return.
  4. On 2xx: read body as text, try to parse as JSON and check for an
     `error` field — if present, return `InferenceError`. Otherwise return
     `Ok(())`. Do not deserialise into `EmbedResponse`; the embedding
     values are unused by the probe.
- **Single render function per variant** — covers four user-facing
  surfaces from one match. The naive shape has the variant matched in
  four places (status `--full` Runtime line, `provision()`'s
  `result.errors`/`result.actions`, U3 hook short-reason), which drifts
  the first time someone adds a variant. Instead, expose a single
  `render_failure(err: &ProbeError) -> RenderedFailure` function in
  `embeddings.rs` (alongside `classify_embed_response`) returning a
  small struct:
  ```
  struct RenderedFailure {
      short_reason: String,    // U3 hook warning, e.g. "inference error"
      status_line: String,     // U2 `lore status --full` Runtime line body
      error_line: String,      // U1 result.errors entry
      action_line: String,     // U1 result.actions entry
  }
  ```
  All four surfaces consume this struct; adding a `ProbeError` variant
  requires updating exactly one match arm and the variant becomes
  rendered everywhere. The body-extraction algorithm below is a private
  helper called from inside `render_failure` for the variants that carry
  a body.
- **Body extraction rule for human-facing strings:** When a `ProbeError`
  variant carries a `body: String`, the renderer (U2) must turn it into a
  one-line message via this algorithm:
  1. Attempt `serde_json::from_str::<{ error: String }>(&body)`. If it
     succeeds, use the extracted `error` field.
  2. Otherwise, take the first non-empty line of the trimmed body,
     replace control characters with spaces.
  3. Truncate to 80 characters with an ellipsis if longer.
  4. If the result is empty, fall back to `HTTP <status>` (or `<variant>`
     for the no-status variants) without a dash-body suffix.
  This rule lives in a small helper in `provision.rs` (or
  `embeddings.rs`) so both the status renderer and the
  `result.errors`/`result.actions` strings use the same extraction.
- Add `pub runtime: ProbeOutcome` to `ProvisionResult` (place after
  `model_available` to mirror the dependency order: installed → running →
  model → runtime), where `ProbeOutcome` is
  `Ok | NotChecked | Skipped | Failed(ProbeError)`.
  - `NotChecked` — caller did not request the probe (default
    `lore status` without `--full`). Renders as a discoverability hint.
  - `Skipped` — model not available, no point probing.
  - `Failed(err)` — carries the structured error for downstream rendering.
- Update `provision()` and `check_status()` to call `client.probe(...)`
  per the rules below. Change `check_status()`'s signature to take a
  `full: bool` parameter. **No default value** — every caller must pass
  the parameter explicitly. The single existing call site at
  `src/main.rs:782` becomes `provision::check_status(&host, &model, full)`,
  where `full` is forwarded from the new clap `--full` flag and is
  `false` when the flag is absent. This explicit-no-default rule prevents
  the failure mode where default `lore status` would silently probe
  because someone added a `Default` impl with `true`.
  - `check_status(host, model, full=true)` calls `probe(Some(0))` when
    `model_available` is true, to avoid pinning the model in RAM after a
    read-only diagnostic call. `runtime = Ok | Failed(err)`.
  - `check_status(host, model, full=false)` skips the probe entirely and
    sets `runtime = NotChecked`. The default status output renders this
    as a discoverability hint (see U2).
  - `provision()` always calls `probe(None)` when `model_available` is
    true, so the model stays warm for the ingest that typically follows
    `lore init`. Emit a progress callback
    (`on_progress("Verifying inference runtime…")`) before the probe so
    the post-pull pause is not silent.
  - When `model_available` is false, set `runtime = Skipped` and do not
    call the probe.
  - Note: `keep_alive: 0` schedules unload after the response is sent,
    not synchronously — a back-to-back `lore status --full` invocation
    may still find the model warm. The worst case in Risks (cold-load on
    every `--full` call) is the upper bound, not the only behaviour.
- Update the `check_status()` docstring: change "Quick read-only health
  check without side effects" to "Quick health check without filesystem or
  config side effects; may trigger model load/unload in Ollama via the
  runtime probe."
- In `provision()`, on `runtime = Failed(err)`, push the rendered failure
  message into `result.errors` and the remediation hint into
  `result.actions` (matching the existing split — `errors` is what went
  wrong, `actions` is what the user can do about it). U2 owns the exact
  strings; U1 only routes the variant into the two vectors.
- **`lore init` continues to completion on probe failure; does not
  abort.** The probe failure populates `result.errors` and
  `result.actions`, which `lore init` surfaces via its existing
  result-printing block. The user sees the failure inline with the rest
  of the init output and can act on it, but the database stays
  initialised and the manifest stays pulled — partial completion is
  better than a half-provisioned rollback. This matches existing
  `provision()` semantics, which already populates `errors` without
  short-circuiting. Exit code: `lore init` returns non-zero when
  `result.errors` is non-empty (also existing behaviour), so CI
  pipelines that gate on init success will see the probe failure as a
  failed init — which is correct: vector search would be silently
  degraded if they proceeded.
- **No `lore init --skip-probe` flag in this plan.** A user who needs to
  bootstrap with known-broken inference (e.g., reproducing a bug, CI
  with mocked Ollama) can set `OLLAMA_HOST` to an unreachable address,
  which will set `model_available = false` and skip the probe via the
  Skipped branch. If real users hit pain (e.g., scripted provisioning
  in environments where inference verification is genuinely undesired),
  add the flag as a follow-up.
- Update the `provision_result_defaults` unit test to initialise
  `runtime: ProbeOutcome::Skipped`.

**Patterns to follow:**

- Mirror `is_healthy` / `has_model` style: short, `Result`-returning probe
  on `OllamaClient`, boolean conversion at the call site.
- Reuse `EmbedRequest` rather than introducing a second request struct —
  one Rust struct per Ollama endpoint contract.

**Test scenarios:**

- `probe_succeeds_against_real_ollama` (integration, gated by
  `just test-integration`): construct `OllamaClient` against the local
  Ollama; assert `probe()` returns Ok. Reproduces the green path on a
  working install; on the currently-broken Homebrew bottle this test fails,
  which is exactly the signal we want before shipping.
- `provision_result_defaults` (unit, updated): verify that calling
  `check_status(host, model, full=false)` against any state returns
  `runtime = ProbeOutcome::NotChecked` (the opt-out path), and that
  calling `check_status(host, model, full=true)` against an unreachable
  Ollama returns `runtime = ProbeOutcome::Skipped` (the model-absent
  path). Tests assert assigned values, not Rust enum defaults — the
  struct has no `Default` impl.
- `check_status_skips_probe_when_model_absent` — covered indirectly by the
  integration suite (the existing `check_status` tests already exercise the
  no-model branch). No standalone unit test: writing one would require
  introducing a mock HTTP framework that this crate has deliberately
  avoided. The probe-skip branch is small enough that a code-review pass is
  the appropriate guard.

**Verification:** Build passes (`cargo build`), unit tests pass
(`cargo test`), integration test passes on a working Ollama
(`just test-integration`). On the current broken Homebrew bottle, the
probe integration test fails with a runner-related error — capture the
exact error string in the test output so U2's error message can reference
it accurately.

---

### U2. Render runtime state in `lore status`, plumb `--full` flag, update provisioning errors

**Goal:** Add the `--full` flag, surface the probe result in CLI output,
and route probe failures into actionable guidance.

**Requirements:** R4, R5.

**Dependencies:** U1.

**Files:**

- `src/main.rs` (add `--full` flag to the `status` subcommand; extend the
  status-rendering block around lines 796–817)
- `src/provision.rs` (probe-failure error/action messages used by both
  `provision` and `check_status` consumers; reuse the same message string
  in U1's call sites)

**Approach:**

- Add a `--full` boolean flag to the `status` subcommand definition in
  `main.rs`. Pass it through to `check_status(host, model, full)`.
- Add a new line to `lore status` output after `Model:`, rendered from
  the `runtime: ProbeOutcome` variant:
  - `ProbeOutcome::Ok` → `  Runtime:      ✓ inference OK`
  - `ProbeOutcome::NotChecked` →
    `  Runtime:      —  (run 'lore status --full' to verify inference)`.
    The em-dash signals "deliberately empty by design" rather than the
    ambiguous "we tried and couldn't" reading of "not checked". Wording
    surfaces the deeper check without paying its cost.
    **Policy:** the hint emits unconditionally in default mode, even
    when the preceding `Ollama svc:` or `Model:` lines are ✗. Running
    `--full` against unreachable Ollama will land on `Skipped` (no
    probe), which is mildly redundant with the preceding ✗ lines but
    acceptable noise versus the branching complexity of conditional
    suppression. The hint also reminds the user the deeper check exists
    once they fix the upstream problem.
  - `ProbeOutcome::Skipped` → omit the line entirely. When the model is
    absent, the preceding `Model: ✗` line is sufficient; a second line
    saying "no runtime to check" is noise. Status output keeps a fixed
    six-line baseline plus one optional Runtime line.
  - `ProbeOutcome::Failed(err)` → renders as
    `  Runtime:      ✗ <render_failure(&err).status_line>` where the
    exact `status_line` strings per variant are defined inside
    `render_failure` in U1. Reference shapes:
    - `RunnerFailed`/`InferenceError`: `inference failed — <body via extraction rule>`
    - `Timeout`: `inference timed out (>30s) — check 'ollama serve' is healthy`
    - `Transport(msg)`: `transport error — <msg>`
    - `HttpStatus { status, .. }`: `unexpected HTTP <status>`
  All `body` rendering goes through the helper defined in U1, so the same
  extraction rule applies everywhere a body is shown to the user.
- All status output remains on stderr per existing convention (`lore status`
  is a diagnostic command; stdout stays reserved for machine consumers).
- Provisioning failure path: when `runtime = Failed(err)`, push
  `render_failure(&err).error_line` to `result.errors` and
  `render_failure(&err).action_line` to `result.actions`. The exact
  strings per variant live in `render_failure`'s match in `embeddings.rs`
  (U1). Reference shapes:
  - `RunnerFailed`/`InferenceError`:
    - `error_line`: `"Ollama reached the model but the inference call failed: <body via extraction rule>"`
    - `action_line`: `"Check 'ollama serve' logs for the underlying cause. Common cases: (1) Homebrew-installed Ollama with a broken runner bundle — try 'brew reinstall ollama'; (2) disk space exhausted; (3) GPU/Metal initialisation failure in a headless or sandboxed environment. The Ollama log line usually starts with 'llama runner process'."`
  - `Timeout` / `Transport` / `HttpStatus`: variant-specific
    `error_line`/`action_line` that do *not* assert "runner bundle bug"
    — those would misdiagnose a slow disk or a network blip.
- Critical: do *not* hard-code `llama-server` in any message. The modern
  Ollama runner appears in logs as `llama runner process`; the standalone
  `llama-server` binary is stale llama.cpp terminology and would send a
  user searching for the wrong string.

**Patterns to follow:**

- Match the existing `eprintln!("  Label:        ...")` two-space-indent,
  fixed-column-label layout in the status-rendering block of `cmd_status()`
  (`src/main.rs:796–817`).
- Status checkmarks: ✓ for healthy, ✗ for failed, matching the existing
  rendering.

**Test scenarios:**

- Manual: run `lore status` (no flag) against any Ollama state; confirm
  Runtime line shows the discoverability hint, command returns in
  sub-second.
- Manual: run `lore status --full` against the current broken Homebrew
  Ollama; confirm Runtime line shows ✗ with Ollama's actual error body
  via U1's extraction rule.
- Manual: run `lore status --full` against a known-good Ollama; confirm
  Runtime line shows ✓ inference OK.
- (No automated test for stderr formatting exists for the surrounding
  lines either — staying consistent with the existing surface.)

**Verification:** Default `lore status` is still sub-second and shows the
discoverability hint. `lore status --full` against the broken Homebrew
bottle shows the new ✗ line surfacing Ollama's actual error body; against
a working install it shows ✓ inference OK. No regressions in the
preceding lines (Ollama, Ollama svc, Model).

---

### U3. Surface the underlying Ollama error in the hook FTS-fallback warning

**Goal:** Embed failures during hook-driven search currently degrade
silently to text search with a one-line warning at `src/hook.rs:676`:
`Warning: Ollama unreachable ({e}), falling back to text search.` The `{e}`
is a bare `ureq::Error`, which on the broken-runner case is something like
`http: server returned status 500` — opaque, no actionable detail. Upgrade
the warning so a user running with the broken Homebrew bottle sees enough
context in their hook output to recognise the same diagnosis
`lore status --full` would have given them.

This unit is the complement to U1/U2: U1/U2 add the *pull-style* opt-in
diagnostic surface (`lore status --full`); U3 fixes the *push-style*
surface (warnings emitted by hook calls the user didn't initiate).
Without U3, a user could run with broken Ollama for weeks, ignore the
scrolling warnings, and never realise vector search is silently disabled.
U3 is also what discovers the deeper check for users who never read
default `lore status` output.

**Requirements:** R4, R7.

**Dependencies:** U1 — U3 **reuses** `ProbeError` and the body-extraction
helper. The classification logic that turns a ureq response into a
`ProbeError` variant is factored out of `probe()` into a small free
function in `embeddings.rs` (e.g., `classify_embed_response`) so both
`probe()` and the hook path call it. This is what makes U3 properly
dependent on U1 rather than parallel work; without the shared
classification, U3's category list would drift from U1's enum (which is
exactly what Round 2 caught).

**Files:**

- `src/hook.rs` (around line 676 — the existing warning emission site)
- `src/embeddings.rs` — extend `Embedder::embed` so it returns the same
  classification (or carries enough context in its `anyhow::Error` for the
  caller to classify). Under ureq 3.x mechanics (see U1), this means
  reading the response body on non-2xx instead of letting `read_json` fail
  with an opaque serde error.

**Approach:**

- Refactor `probe()` so the response → `ProbeError` mapping lives in a
  shared `classify_embed_response(response_or_err) -> Result<(), ProbeError>`.
  Both `probe()` and `Embedder::embed`'s error path call it.
- Update `Embedder::embed` to follow the same ureq 3.x mechanics as
  `probe()` (check status before `read_json`); on failure, attach the
  classified `ProbeError` to the `anyhow::Error` it returns.
- At `src/hook.rs:676`, downcast the embed error to `ProbeError`, call
  `render_failure(&err)`, and use `.short_reason` in the warning:
  `Warning: Ollama embed failed (<short_reason>); falling back to text search. Run 'lore status --full' for details.`
  The variant→`short_reason` mapping lives inside `render_failure` in
  U1, so the hook path does not maintain its own mapping table and
  cannot drift from the enum.
- Pointing at `lore status --full` keeps the hook warning short and routes the
  user to the full diagnosis from U1/U2 rather than duplicating it inline.
  Hooks fire frequently; the warning should be one line.
- **Rate-limit the warning per process, keyed on failure class.** A
  hook-using session may fire dozens of hook invocations per day; if
  every failure prints the same warning, the user trains themselves to
  filter it as wallpaper — defeating the rescue purpose. Track the last
  emitted failure class in a process-local cell (e.g., `OnceLock` or
  `Mutex<Option<String>>`) and only emit when the class changes or on
  the first failure of the process. After the first suppression, append
  a counter to subsequent warnings so the user can see the failure is
  ongoing without each one demanding attention. Suggested shape:
  - First failure of a class: full warning as specified above.
  - Same class again within the process: silent.
  - Class changes (e.g., timeout → inference error): emit new full
    warning.
  This is a per-process state, not persisted — new hook process = fresh
  warning, which is the right behaviour (each process should be
  self-diagnosing).

**Patterns to follow:**

- Existing CLI output convention: warnings to stderr (already correct at
  `src/hook.rs:676`).
- Avoid duplicating U1/U2's full diagnostic prose in the hook path —
  point the user at `lore status --full` instead.

**Test scenarios:**

- Manual: trigger a hook call (e.g., via `lore hook session-start` against
  a session payload that produces a search query) while the broken
  Homebrew Ollama is running; confirm the warning now mentions
  "inference error" and pointers to `lore status --full` instead of a
  bare `ureq` error string.
- Existing hook tests (`tests/hook.rs`) should continue to pass; the
  warning text is not asserted on, so no test updates are required for
  the rewording itself. If `tests/hook.rs` happens to assert on the old
  warning string, update the expected text.

**Verification:** Manual run against the broken Homebrew install — hook
warning is informative and points at `lore status --full`. Against a
working install, no warning fires (the embed succeeds and the hook
proceeds with vector results as before).

---

## Risks & Mitigations

- **Discoverability of `--full`.** Because the probe is opt-in, a user who
  never reads the hint line or never hits a hook warning would never
  exercise the deeper check. Three rescue mechanisms reduce this risk to
  acceptable levels:
  1. Default `lore status` displays the hint line on every invocation —
     impossible to miss for anyone who reads the status output.
  2. The U3 hook warning fires automatically on inference failure during
     real use and names `lore status --full`.
  3. `lore init` runs the probe automatically as part of provisioning,
     so fresh setups verify inference at install time.
  Residual risk persona — be explicit about who we don't catch: a user
  who provisioned `lore` six months ago when Ollama worked, runs `lore`
  primarily via hooks where stderr is captured to a log file they don't
  tail, never explicitly runs `lore status`, and whose Ollama install
  breaks later (e.g., a `brew upgrade` regressing the runner bundle).
  None of the three rescue mechanisms fire for this user. They are
  *not* "actively ignoring signals" — they are using `lore` exactly as
  intended in a non-interactive flow. The opt-in design accepts this
  collateral knowingly; a persisted last-probe-state with a TTL would
  catch them (see Deferred to Follow-Up Work) but introduces stale-cache
  semantics we have chosen not to pay for in v1. If post-launch
  telemetry or user reports show this persona is non-negligible, the
  persistence option moves up.
- **`lore status --full` latency.** Cold-load of `nomic-embed-text` on a
  freshly-restarted Ollama is empirically 3–15 seconds (mmap + kernel
  warmup). With `keep_alive: 0` on the `--full` path we unload after
  every call, so each invocation pays the cold-load cost again.
  Acceptable: `--full` is an explicit opt-in for a deeper diagnostic; the
  user asked for the deeper check and is committed to waiting.
- **Default `lore status` sub-second claim assumes a responsive Ollama.**
  The existing `is_healthy()` and `has_model()` calls share a 30 s
  global timeout on `OllamaClient::agent`. Against a healthy local
  Ollama both calls are sub-100 ms; against an Ollama host that accepts
  TCP but stalls (hung process, firewall DROP, paused container) each
  call can hang up to 30 s, so the worst-case default-mode latency tail
  is ~60 s. This is pre-existing, not introduced by the plan, but the
  "sub-second" framing in the rationale for opt-in implicitly refers to
  the healthy-Ollama case (which is the comparison that motivates the
  opt-in design: sub-second vs 3–15 s probe).
- **Probe timeout under cold disk.** The existing 30 s global timeout on
  `OllamaClient::agent` covers normal cold-load but is tight on slow disks
  (spinning, network volumes, throttled SSDs). The `Timeout` `ProbeError`
  variant renders a distinct status message that does *not* assert "runner
  broken" — a slow-disk false positive shows up as "inference timed out"
  with a pointer to check Ollama's health, not a misleading runner-bundle
  diagnosis.
- **`/api/embed` semantics across Ollama versions.** The endpoint has been
  stable since v0.1.x. If we ever support a pre-`/api/embed` Ollama, the
  probe would false-negative; not a current concern.
- **False positives on partial runner failures.** Probe sends one short
  input; a runner that loads but fails on longer inputs would pass. Out of
  scope — we only need to catch the "runner missing / unloadable" failure
  mode, and the embed call exercises the loading path that fails today.
- **HTTP 200 + error body assumption.** Ollama sometimes returns 200 with
  an `error` field rather than HTTP 5xx. U1's probe parses 2xx bodies for
  that case. If the broken-Homebrew integration test reveals a different
  shape (e.g., 200 with empty `embeddings` array), the body-parsing branch
  is extended to recognise it — does not require structural plan changes.

---

## Verification Strategy

The currently-broken Homebrew Ollama install on the author's machine is the
acceptance test: before the change, `lore status` reports clean ✓✓✓ while
`lore ingest` silently falls back to FTS. After the change:

- Default `lore status` shows the discoverability hint line and remains
  sub-second.
- `lore status --full` shows Runtime ✗ with Ollama's actual error body via
  U1's extraction rule.
- The integration test `probe_succeeds_against_real_ollama` fails — both
  correctly reflecting reality.
- A hook-driven search emits an informative warning naming the failure
  class and pointing at `lore status --full`.

Once Ollama is repaired (reinstall, upstream fix, etc.), the same runs
should flip back to green without any code change.

---

## Sources & Research

- `src/embeddings.rs:52–109` — existing `OllamaClient` methods (`is_healthy`,
  `has_model`, `pull_model`, `Embedder::embed` flow to mirror in `probe`).
- `src/provision.rs:21–207` — `ProvisionResult` struct, `provision`, and
  `check_status` integration points.
- `src/main.rs:780–817` — `lore status` output rendering block.
- `tests/ollama_integration.rs:1–40` — existing integration-test pattern
  (real-server, gated by `just test-integration`, `OLLAMA_HOST` constant).
- `src/hook.rs:676` — existing FTS-fallback warning emission, the target
  of U3.
