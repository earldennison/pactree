# PACT Projection Shape Contract (component era)

Authoritative consumer contract for the JSON produced by the engine's projection
surfaces (`file_project_head_json`, `_pact_file_open` / `pactree.file.open`,
`ContextTree.project`). Sourced from `Component::project_component_value()` and
`ComponentKind::project_into()` (`src/components.rs`) at engine `main`
(2026-07-04, trace_compact:v2 era). If this document and the code disagree, the
code wins — update this file.

## Tree structure

The projection of a branch head IS the root component object. Recursion:

- `children`: array of component objects, in canonical order. Recurse here.
- `child_ids`: array of the direct children's ids, same order as `children`
  (derived from `children`; use for order without recursing).
- `parent_id`: string, or `null` on the root.

There is no wrapper envelope around the root; `id` of the root is the tree/root
id (e.g. `root` or a minted `root_*`/session id).

## Fields on EVERY component

| key | type | notes |
|---|---|---|
| `id` | string | globally unique within the tree; format `<tree_id>:<local>` for authored nodes |
| `componentType` | string | canonical discriminant — see vocabulary below |
| `parent_id` | string \| null | |
| `offset` | number | sibling position lane |
| `priority` | number | |
| `transaction_index` | number | |
| `created_at_ns` | number | u64 nanoseconds |
| `creation_index` | number | |
| `ttl` | number \| null | always present |
| `child_ids` | string[] | always present (may be empty) |
| `children` | object[] | always present (may be empty) |

## Optional fields on any component (present only when set)

`type` (subtype string — NOT the discriminant; do not switch on it),
`role` (message_turn carries `user`/`assistant`/`system`/`tool` here),
`pact_depth` (number; system lane is negative, e.g. system_header = -1),
`created_at_iso` (string), `cad` (number), `key` (string), `tags` (string[]),
`aux` (object — app-data lane; lifecycle flags like `archived`/`deleted` ride here),
`external_refs` (array), `lifecycle` (object; state+revision round-trip — policy
`ttl`/`cad` are projected flat, `internal_refs` are inspect-only and never appear),
`provenance` (object).

## componentType vocabulary (canonical discriminants)

- `root`
- `system_header`
- `message_turn`
- `text_content`
- `reasoning_content`
- `tool_call`
- `tool_result`
- `media_content` — media FAMILY (image/audio/...) lives in `media_kind`, never
  in the discriminant
- container components: `componentType` is the container's own type string
  (dynamic — treat any non-listed value as a container/unknown and recurse into
  `children`; do NOT drop unknown types)

Legacy vocabulary that NO LONGER EXISTS anywhere in projections: `nodeType`,
`node_ref`, `node_id`, `NodeMeta`/`NodeRecord` shapes. A consumer matching those
gets zero rows on every current file.

## Per-kind payload fields (flat on the component object)

- `text_content` / `reasoning_content`: `content` (string, may be absent when
  externalized), `content_ref` (object), `source_ref` (object),
  `content_capture` (string), `content_status` (string).
- `tool_call`: `tool_call_id`, `tool_name` (both omitted when empty),
  `parameters` (any JSON), `parameters_ref`.
- `tool_result`: `tool_call_id`, `tool_name`, `success` (bool), `content`,
  `content_ref`, `result` (any JSON), `result_ref`, `error_message`,
  `error_ref`, `source_ref`, `content_capture`, `content_status`.
- `media_content`: `media_kind` (string family) + body lanes: `content`
  (inline text), `bytes`+`encoding` (inline bytes), `url`, `content_ref`,
  `external_ref` — exactly one body lane populated.
- `message_turn`: no payload keys of its own; `role` + children carry the turn.

## Content lanes (hydration rule)

A component's text is `content` when inline. When externalized, `content_ref`
points into the content store and `content_preview` may carry a bounded
preview. Projection surfaces hydrate content by default (a hydrated projection
has `content` filled); if you see `content_ref` with no `content`, treat as
not-hydrated, never as empty.

## Chat-row extraction recipe (for UI row builders)

1. Walk `children` from the root recursively.
2. A conversation turn = `componentType == "message_turn"`; its `role` is on
   the turn; its renderable parts are its descendant `text_content` /
   `reasoning_content` / `tool_call` / `tool_result` / `media_content` nodes
   in `child_ids` order.
3. System surface = `pact_depth < 0` subtree (`system_header` lane).
4. Skip components whose `aux` carries `archived: true` / `deleted: true`
   unless building an archive view.
5. Tool pairing: match `tool_result.tool_call_id` to `tool_call.tool_call_id`.

## Overlay children (base-provenance files) — CAVEAT

A `fork(mode=delta/overlay)` child file contains only LOCAL components; its
full view is composed at read time (base + local). `file_project_head_json` on
the RAW child projects only the local tree. Compose-on-read currently lives on
the `ContextTree` read path. If a consumer needs composed overlay projections
through the file-level entrypoints, that is an engine work item
— do not paper over it consumer-side.
