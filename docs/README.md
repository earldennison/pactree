# Pactree Documentation

Pactree is a deterministic context-tree substrate implemented by the Rust crate
in this repository. It owns the PACT JSON node model, selector grammar,
branchable JSON stores, transaction traces, provider-thread rendering, and the
binary `.pact` file format.

The docs in this repository describe Pactree itself. Host applications may
adapt Pactree output, but host vocabulary is not part of the core substrate.

## Documentation Map

- [Architecture](architecture/substrate-model.md): how nodes, stores,
  branches, commits, heads, projections, streams, and views fit together.
- [Selectors](reference/selectors.md): the actual Rust selector grammar.
- [Selector Cheat Sheet](reference/selector-cheat-sheet.md): compact examples
  for everyday selector syntax.
- [Stream Contract](reference/stream-contract.md): `pact.stream.v1` rows,
  cursors, and scroll geometry.
- [CLI Reference](reference/cli.md): commands exposed by `src/main.rs`.
- [Provider Thread](reference/provider-thread.md): Pactree-owned rendering to
  provider-neutral message JSON.
- [Content Refs And Bundles](reference/content-refs-and-bundles.md): inline
  content, chunk records, manifests, and bundle commands.
- [Host Adapter Contract](integrations/host-adapter-contract.md): integration
  boundary for applications built on top of Pactree.
- [Native Adapter Boundaries](internals/adapter-boundaries.md): typed execution,
  serialized adapters, and staged versus durable state.

## Source Of Truth

When docs conflict with code, the Rust crate wins. The main anchors are:

- `src/node.rs` for PACT node fields and content refs.
- `src/selector.rs` for selector parsing and evaluation.
- `src/store/model.rs` and `src/store/api.rs` for JSON stores and branches.
- `src/file_format.rs` and `src/file_format/*` for `.pact` storage.
- `src/provider_thread.rs` for provider-thread rendering.
- `src/view.rs` for view refs.
- `src/main.rs` for CLI commands.
