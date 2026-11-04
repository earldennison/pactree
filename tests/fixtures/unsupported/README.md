# Unsupported pre-v0.1 Pact files

These fixtures exist only to verify typed rejection. They must never become
compatibility-reader inputs or semantic golden files.

## Published 0.0.1

- Source: crates.io `pactree` 0.0.1
- Crate archive SHA-256: `8a658e49374c6da18df88958abd2ad28b0b64136ad4841e7dd1002d95375c2ed`
- Fixture SHA-256: `2bde85d55555807dbbcbebf08ace9436dea5771bb3156090616f40e9348c173a`
- Fixture bytes: `1423`

Generation:

```bash
cargo run --manifest-path "$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/pactree-0.0.1/Cargo.toml" -- root-new < minimal-component.json > published-root.json
cargo run --manifest-path "$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/pactree-0.0.1/Cargo.toml" -- file-create published-0.0.1-minimal.pact < published-root.json
```

## Audited pre-v0.1 head

- Source commit: `26e3ec704297801c18801e3f834eeb1ec740dcb2`
- Source commit date: `2026-07-05T02:52:18+08:00`
- Fixture SHA-256: `ac3f9f1f9fa803d91e353c8ece4781b6ad12752d7f04c5de56435cf8508de920`
- Fixture bytes: `2933`

Generation:

```bash
cargo run -- root-new --no-base < minimal-component.json > current-root.json
cargo run -- file-create current-pre-v0.1-minimal.pact < current-root.json
```

Both fixtures were generated with:

```text
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

The shared `minimal-component.json` contained a root and one system-header
child with the literal content `pre-v0.1 rejection fixture`.
