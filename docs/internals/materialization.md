# Projection

Projection reads a requested tree/view/query surface for a branch/revision.

File projection validates the `.pact` scan, selects a branch/revision, replays
component-shaped commit/trace records, and emits the requested sink. Content
records/chunks are used for referenced payloads.

Source anchors:

- `src/file_format.rs`
- `src/file_format/projection_replay.rs`
- `src/file_format/history.rs`
- `src/store/branch_projection.rs`
