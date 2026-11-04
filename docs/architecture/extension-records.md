# Extension Records

Pactree supports generic extension data through node metadata, view records,
and selected record kinds. The domain meaning belongs to the adapter or
runtime; Pactree owns persistence, ordering, refs, validation, and query
boundaries.

## Current Surfaces

- Node metadata: `attrs`, `org`, and `surface`.
- View records: `PactViewRecord`.
- Component record types remain in the file/query contract, but Pact v0 rejects
  component snapshot append APIs.
- Content manifests can carry `aux`.

Unknown domain fields should be namespaced and treated as adapter-owned.
