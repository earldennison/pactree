# Stream Contract

`pact.stream.v1` is returned by `file_stream_json` and `ContextTree::stream`.
It is a cursor window over a Pactree surface.

PACTBIN2 reads use the message-order and revision indexes to fetch a bounded
window plus lookahead. Counts and cursors remain exact.

`message_surface` is the host-chat/read-model seam. It stays entirely on the
immutable PACTBIN2 snapshot: the message-order B+tree locates the requested
window, then only those messages and their canonical child subtrees/content
are hydrated. It never materializes the full ledger or component tree.

## Modes

Accepted mode inputs are normalized as follows:

- `current`, `current_surface`, `current-surface`, `context_head`,
  `context-head`, `head`, or empty string -> `current_surface`
- `message`, `messages`, `message_surface`, `message-surface`, `chat`,
  `chat_surface`, or `chat-surface` -> `message_surface`
- `activity`, `activity_surface`, `activity-surface`, `timeline` ->
  `activity_surface`

Unsupported modes fail with an error.

`current_surface` streams rows from the branch projection's `message_order`.
`message_surface` streams the same canonical message identities through the
PACTBIN2 message-order index, but each returned message payload includes its
hydrated canonical child subtree. It reads the live head only; callers use a
refreshable read handle to advance to a newly committed head.
`activity_surface` starts with current-surface rows and appends activity rows
derived from transaction traces.

## Request Arguments

Function inputs:

- path
- mode
- optional branch id
- optional revision
- optional anchor row id
- optional anchor seq
- direction: `before`, `after`, `around`, plus aliases `up` and `down`
- limit, clamped to `1..=500`

## Response Shape

```json
{
  "schema": "pact.stream.v1",
  "path": "session.pact",
  "surface": "context",
  "mode": "current_surface",
  "direction": "before",
  "head": {"schema": "pact.head.v1"},
  "anchor": {
    "row_id": null,
    "seq": null
  },
  "rows": [
    {
      "row_id": "node:turn_1",
      "kind": "node",
      "node_id": "turn_1",
      "node_type": "message_turn",
      "role": "user",
      "cycle": 1,
      "revision": 1,
      "payload": {},
      "seq": 0,
      "cursor": {
        "row_id": "node:turn_1",
        "seq": 0,
        "revision": 1,
        "cycle": 1
      }
    }
  ],
  "cursors": {
    "before": {"row_id": "node:turn_1", "seq": 0, "revision": 1, "cycle": 1},
    "after": {"row_id": "node:turn_1", "seq": 0, "revision": 1, "cycle": 1}
  },
  "position": {
    "start_index": 0,
    "end_index": 0,
    "total_rows": 1,
    "estimated": false
  },
  "has_before": false,
  "has_after": false
}
```

## Row Types

Current-surface rows:

- `row_id`: `node:<node_id>`
- `kind`: `node`
- `node_id`
- `node_type`
- `role`
- `cycle`
- `revision`
- `payload`: the node record from the branch projection

Activity rows:

- `row_id`: `activity:<transaction_id>:<event_index>`
- `kind`: `activity`
- `transaction_id`
- `payload`: activity entry derived from trace records

All rows expose a zero-based projection-order `seq`. For `message_surface`,
the message-order index computes that token without hydrating the full ledger.
Message identity is the `row_id` resolved within the returned branch head
(`head.root_id`, `head.branch_id`, `head.revision`/transaction lineage), never
`seq` or an unqualified client cache entry. This v0.1 message-surface contract
addresses the loaded head; pinned historical-snapshot identity is a separate
API surface.

## Cursor Rules

- `--after <row_id>` starts after the anchor row.
- `--before <row_id>` returns rows before the anchor row.
- `--around <row_id>` centers the anchor within the requested limit where
  possible.
- `--anchor-seq <n>` can locate a current/activity anchor by sequence instead
  of row id. `message_surface` rejects seq-only anchors because positional
  tokens can alias after a projection reorder.
- When no anchor is provided, `after` starts at the first row and other
  directions return the last window.

For `message_surface`, an incremental cursor must be replayed with both
`row_id` and `seq` together with the returned head identity. `row_id` is
resolved through the current head's message index; `seq` is the row's
projection-order token and detects a mutation before the cached anchor:

- a missing `row_id` returns `not_found`;
- a seq-only message anchor returns `unsupported`;
- a valid `row_id` paired with a stale projection seq returns `invalid`
  with a stale-anchor diagnostic;
- `before` and `after` exclude the resolved anchor;
- append-only commits preserve existing cursor pairs;
- insertion, deletion, or reorder before an anchor changes its projection
  seq and explicitly stales the pair, so consumers can perform one bounded
  rebase instead of silently missing or aliasing rows;
- a row-id-only request resolves the row's current position and is suitable
  for that bounded rebase, but not as an incremental no-gap cursor;
- `after` a cached tail returns only newly indexed rows after a read-handle
  refresh.

`cursors.before` is the first returned row cursor. `cursors.after` is the last
returned row cursor. Either is `null` for an empty window.

## Scroll Geometry

`position.total_rows` is exact in the current implementation, and
`position.estimated` is always `false`.

`start_index` is inclusive. `end_index` is the inclusive index of the last row;
for an empty window it is computed with saturating subtraction and should not be
treated as a valid row index unless `rows` is non-empty.

## Source Anchors

- `src/file_format.rs`
- `src/pact_file/message_stream.rs`
- `src/read_handle.rs`
- `src/file_format/tests.rs`
- `src/main.rs`
