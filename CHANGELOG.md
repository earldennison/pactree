# Changelog

## 0.1.0 - Unreleased

- Freeze the existing Rust, CLI, serialized-schema, and consumer contracts.
- Introduce the `PACTBIN2` / `pact.file.v2` storage identity.
- Reject every pre-v0.1 Pact file without a decoder or migration path.
- Add crash-consistent transactions, one copy-on-write index family,
  content-body deduplication, semantic checkpoints, lazy snapshots, and
  bounded current/activity stream primitives.
- Add package, API, behavior, schema, lint, panic-surface, and test-layout gates.
