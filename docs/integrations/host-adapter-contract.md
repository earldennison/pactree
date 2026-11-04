# Host Adapter Contract

A host adapter connects an application to Pactree without making that
application part of the Pactree substrate. The adapter translates host events
into Pactree operations and translates Pactree reads back into host
presentation.

## Pactree-Owned Semantics

Adapters should treat these as substrate-owned:

- PACT node shape and deterministic child ordering.
- Selector grammar and selector result ordering.
- Branch ids, revisions, refs, traces, commits, and heads.
- Content refs, content records, manifests, and bundle payload policy.
- Provider-thread rendering and tool-call pairing rules.
- View refs and branch-following behavior.
- `.pact` validation, repair, reanchor, repack, slice, import, and merge.

## Adapter Responsibilities

Adapters may:

- map host objects to `PactNode` values;
- submit operations through `PactResolver`;
- bind a resolver to a `.pact` path for commit persistence;
- call `file_projection_json`, `file_stream_json`, or CLI equivalents for
  display;
- call provider-thread rendering when building provider requests;
- store host-specific metadata in `attrs`, `org`, `surface`, or namespaced
  extension records;
- translate Pactree errors into host-native error messages.

Adapters must not:

- mutate `.pact` bytes outside Pactree APIs;
- treat provider wire JSON as canonical Pactree storage;
- treat a stream window as the complete projection;
- require host vocabulary to interpret core Pactree records;
- bypass tool-pair validation for provider-bound rendering.

## Typical Mapping

```text
host item created       -> insert operation or InsertComponent trace op
host item edited        -> update/replace operation or UpdateComponent/ReplaceComponent trace op
host item removed       -> delete operation, selector, or component id
host transaction sealed -> resolver commit boundary
host viewport opened    -> projection or stream read
host provider request   -> provider-thread rendering
host archive/export     -> slice, bundle, or export-full command
```

## Resolver Path

For an in-process Rust adapter:

1. Create `PactResolver::new(initial_tree_json, Some("main"))` or
   `PactResolver::open_file(path)`.
2. Submit `PactOperation` batches.
3. Resolve and commit staged operations.
4. Use projections, streams, provider-thread rendering, and views as read APIs.
5. Let Pactree persist commits if `bind_file` is set.

## File-Oriented Path

For a process boundary adapter:

1. Create or receive root-store JSON.
2. Write a `.pact` file with `pactree file-create`.
3. Append commits, traces, views, or content manifests through CLI/API commands.
4. Read with `head`, `project`, `stream`, `file-query`, and `load components`.
5. Use `doctor`, `heal`, `reanchor`, and `repack` for maintenance.

## Extension Data

Use namespaced keys for host-specific metadata. Pactree preserves generic node
metadata and selected extension records, but the domain meaning belongs to the
adapter.

## Source Anchors

- `src/resolver.rs`
- `src/resolver/api.rs`
- `src/file_format.rs`
- `src/provider_thread.rs`
- `src/view.rs`
