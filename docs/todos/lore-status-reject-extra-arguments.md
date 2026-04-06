---
title: "lore_status accepts arguments but silently ignores them"
priority: P2
category: api-contract
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - src/server.rs:284-296
  - src/server.rs:handle_lore_status
related_pr: feat/git-optional-knowledge-base
---

# lore_status accepts arguments but silently ignores them

## Context

The `lore_status` MCP tool definition uses an empty input schema:

```json
"inputSchema": {
  "type": "object",
  "properties": {},
  "required": []
}
```

JSON Schema's default for `additionalProperties` is `true`, so a client that sends
`{"name": "lore_status", "arguments": {"verbose": true}}` will pass schema validation. The handler
`handle_lore_status` ignores the `args` value entirely and produces the same response as if no
arguments were passed.

An agent making a typo (`{"check": "git"}` instead of `{"git_check": true}`) gets no feedback. There
is no way to tell from the response that the argument was unrecognised.

## Proposed fix

Add `"additionalProperties": false` to the lore_status `inputSchema`:

```json
"inputSchema": {
  "type": "object",
  "properties": {},
  "required": [],
  "additionalProperties": false
}
```

The MCP server's hand-rolled JSON-RPC dispatcher does not currently validate input schemas
client-side, so this change alone may not produce a hard error. Verify whether the schema is checked
at the protocol layer (it should be — MCP clients usually do JSON Schema validation before
dispatch).

If the schema is not enforced anywhere, also add an explicit check in `handle_lore_status`:

```rust
fn handle_lore_status(req: &JsonRpcRequest, ctx: &ServerContext<'_>) -> JsonRpcResponse {
    let params = req.params.as_ref();
    if let Some(args) = params.and_then(|p| p.get("arguments"))
        && let Some(obj) = args.as_object()
        && !obj.is_empty()
    {
        return error_response(req, "lore_status accepts no arguments");
    }
    // ... existing implementation
}
```

## Test surface

Add `lore_status_rejects_unknown_arguments` in `src/server.rs::tests` that sends `{"foo": "bar"}` as
arguments and expects an error response.

## References

- Adversarial finding (confidence 0.88): contract mismatch
