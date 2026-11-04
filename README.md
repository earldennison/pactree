# Pactree

Pactree is the deterministic context-tree engine used by agent runtimes to own
ordered components, canonical transaction traces, selectors, projections,
branch history, and compact `.pact` storage.

Version 0.1 writes `PACTBIN2` files with schema `pact.file.v2`. In normal
runtime readers, pre-v0.1 Pact files are unsupported and rejected with a typed
error. The separate `pact-v01-convert` utility converts supported `PACTBIN1`
files; an older executable can be supplied as an optional fallback reader.

The Rust crate is the semantic authority. `pactree-py` is a separately
versioned binding and consumer that preserves the same engine behavior rather
than reimplementing it.

```rust
use pactree::{ContextTree, PactResult, ProviderThreadOptions};

fn main() -> PactResult<()> {
    let mut tree = ContextTree::new()?;
    let _thread = tree.send(&ProviderThreadOptions::default())?;
    Ok(())
}
```

See [API stability](docs/reference/api-stability.md), the
[file-format contract](docs/reference/file-format.md), and the
[CLI reference](docs/reference/cli.md).
