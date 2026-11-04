# Storage Lifecycle

Pactree has JSON store workflows and binary `.pact` file workflows.

## JSON Store

1. Normalize a PACT JSON tree.
2. Create a `pact.store.v1` or `pact.root_store.v1`.
3. Append traces or resolver operations.
4. Project a branch.
5. Fork/checkpoint as needed.

## `.pact` File

1. Create from a root store with `file-create`.
2. Append commits, traces, views, or content manifests through Pactree APIs.
3. Read with `head`, `project`, `stream`, `file-query`, or `load components`.
4. Validate with `check` / `doctor`.
5. Maintain with `repack`, `reanchor`, `reindex-head`, or `heal`.
6. Slice/import/merge for exchange workflows.

Hot `.pact` storage is binary and appendable; portable/full export is explicit.
