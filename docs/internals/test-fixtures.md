# Test Fixtures

Most current fixtures are built inline in Rust tests rather than checked in as
standalone files.

Useful fixture anchors:

- `tests/cli.rs` for CLI-level JSON examples.
- `src/selector.rs` test module for selector sample trees.
- `src/store/tests.rs` for JSON store and branch behavior.
- `src/file_format/tests.rs` for `.pact` commit, stream, bundle, slice, view,
  and repair behavior.
