# Provider Thread Reference

Provider-thread rendering is implemented in `src/provider_thread.rs`. It builds
provider-neutral JSON from a committed or staged PACT tree. It is a read over
Pactree state, not stored provider wire history.

The schema constant is `pact.provider_thread.v1`, but successful thread output
currently returns:

```json
{
  "messages": []
}
```

Errors include:

```json
{
  "schema": "pact.provider_thread.v1",
  "reason": "1 tool call(s) unpaired; provider surface refuses to emit",
  "offending_tool_call_ids": ["toolu_..."]
}
```

## Inputs

`build_provider_thread` takes:

- a serialized PACT projection adapter input;
- `ProviderThreadOptions`.

`build_provider_thread_with_content_store` first hydrates provider-visible
payload refs from the `ContentStore`, then calls `build_provider_thread`.

Resolver helpers:

- `PactResolver::build_provider_thread(branch_id, revision, options)`
- `PactResolver::seal_to_provider_thread(branch_id, boundary, options)`

Rendering alone is read-only. `seal_to_provider_thread` is the explicit
composition that commits staged state first and then renders.

## Options

```json
{
  "include_partial_tool_calls": false,
  "fail_on_orphan_tool_calls": true
}
```

Defaults reject orphan tool calls and omit in-flight tool calls.

## Ordering

Pactree ordering is substrate ordering:

1. System header from a `system_header` node.
2. Historical turns under `conversation_history`, sorted by descending
   `pact_depth` so older context is emitted first.
3. Active turn: `active_message` or `message_turn` with `pact_depth == 0`.

Wall-clock time is not used for provider-thread ordering.

## Message Mapping

System header:

```json
{
  "message_type": "system_header",
  "content": [{"content": "system text"}]
}
```

User or non-assistant turns become:

```json
{
  "message_type": "client_request",
  "content": [{"content": "hello"}]
}
```

Assistant turns are split:

- provider-safe text/media/tool-call blocks become `provider_response`;
- tool-result blocks become a following `client_request`.

```json
{
  "message_type": "provider_response",
  "content": [
    {"content": "I will call a tool."},
    {"tool_name": "search", "tool_call_id": "toolu_1", "parameters": {"q": "pactree"}}
  ]
}
```

```json
{
  "message_type": "client_request",
  "content": [
    {"tool_call_id": "toolu_1", "tool_name": "search", "success": true, "message": "ok"}
  ]
}
```

## Content Blocks

Text leaves emit `{"content": "<text>"}`.

Media leaves (`image_content`, `audio_content`, `video_content`,
`document_content`) emit media blocks only when there is a concrete media ref
in attrs or content. Otherwise they degrade to text instead of fabricating media
payloads.

Tool calls are identified by `tool_call_id` plus call-shaped fields such as
`tool_name` or `parameters`. Tool results are identified by `tool_call_id` plus
result-shaped fields such as `success`, `result`, or `error_message`.

## Tool Pair Invariant

Before rendering, Pactree checks unpaired tool calls from the native component
tree. If a call has no matching result and the options do not allow partials,
rendering fails with `ProviderThreadError`.

Tool results are emitted only if their matching call is pending from an earlier
provider response or present in the current turn.

## Metadata

Assistant messages may include `metadata.surface` collected from supported
surface escape-hatch locations such as `surface`, `attrs.surface`,
`org.surface`, and nested metadata/aux paths.

## Source Anchors

- `src/provider_thread.rs`
- `src/resolver/api.rs`
