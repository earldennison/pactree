# Native execution and serialized adapters

## components.rs

The `Component` / `ComponentKind` model is the semantic interface.
JSON values carry auxiliary metadata, tool payloads, external references, and
explicit serialization or FFI inputs and outputs.

## tree.rs

Mutable native execution substrate: `ComponentTree` owns the component tree
used by mutation, pruning, lifecycle, selectors, and rendering. `PactView`
and other projected read models do not own execution.

## selector.rs

Serialized inputs are parsed into the native tree before selector semantics run.

## mutation.rs

Canonical mutation applies to `ComponentTree`. JSON remains at request/report
boundaries and in user payload fields, including content refs and auxiliary data.

## normalize.rs

Normalization produces a serialized inspection/rendering view, not an execution
tree. Its JSON fields are projected payloads.

## store/api.rs

Public JSON functions in this module are CLI/FFI/file-wire adapters. They decode
input, call native ledger operations, and encode output.

## resolver/api.rs

Native authoring accepts typed components. Public
`*_json` methods and `serde_json::Value` reports are FFI/CLI compatibility adapters; staged execution
uses `ComponentTree`.

- Serialized component adapter for insert: `insert_json`; native callers use `insert_component`.
- Serialized component adapter for replace: `replace_json`; native callers use `replace_component`.
- Serialized component adapter for append: `append_json`; native callers use `append_component`.
- Serialized component adapter for upsert: `upsert_json`; native callers use `upsert_component`.

Serialized provider-response adapter: `receive_json` stages canonical response
parts, verifies tool calls, and seals the `PostResponse` boundary. Callers should
not seal that boundary a second time.

## provider_thread.rs

`build_provider_thread` accepts a serialized projection. This is a compatibility adapter; provider-thread behavior
is implemented by `build_provider_thread_tree` over `ComponentTree`.

## trace.rs

Staged trace operations support inspection and selective discard.
These ops become durable replay data only when seal writes a `TransactionTrace`.

## resolver.rs

Staged trace handles are metadata, not a second durable replay ledger.
The Private staged operation journal records operations per branch. Selective
discard removes matching records and re-resolves the remaining operations from
the committed head, preserving selector, lifecycle, identity, and validation rules.

## file_format.rs

The file boundary is allowed to use `serde_json::Value` for file wire records,
pages, manifests, query results, and compatibility adapters. These are not the
semantic execution substrate. File APIs that change Pact state route through
native ledger and component-tree operations.

File commit-record projection hydrates `TransactionTrace` history. Patch-shaped
mutation logs are unsupported.

## file_format/projection_replay.rs

`CommitRecordOp` replay is a file-wire adapter. It replays concrete commit records
and does not call any patch-shaped import path.
