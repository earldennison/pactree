<div align="center">

# Pactree

### Your context is a tree, not a transcript.

[Docs](docs/README.md) · [Python](https://github.com/earldennison/pactree-py) · [GitHub](https://github.com/earldennison/pactree) · [Issues](https://github.com/earldennison/pactree/issues)

[![CI](https://github.com/earldennison/pactree/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/earldennison/pactree/actions/workflows/ci.yml)
[![Stars](https://img.shields.io/github/stars/earldennison/pactree?labelColor=black&style=flat-square)](https://github.com/earldennison/pactree)
[![License: MIT](https://img.shields.io/badge/license-MIT-white?labelColor=black&style=flat-square)](LICENSE)

</div>

***

## What is Pactree?

Pactree is a Rust engine for building, editing, and storing AI context. It keeps
messages, instructions, tool calls, and content in an addressable tree. You can
select individual components, change their placement, inspect earlier revisions,
and fork a conversation without reducing everything to an ever-growing string.

It runs inside your application, with Python bindings and a CLI. No server or
hosted service is required. Your application runs the agent and calls the model;
Pactree manages the context.

## What does PACT mean?

**PACT stands for Positioned Adaptive Context Tree.** Think of it as a DOM for
AI context:

- **Positioned** — components have explicit placement and ordering, rather than
  depending only on when text was appended.
- **Adaptive** — applications can insert, update, move, or remove components as
  the conversation changes.
- **Context Tree** — instructions, messages, tool exchanges, and media references
  remain structured, addressable parts of the same context.

Pactree is the implementation. A `.pact` file stores the tree's durable state,
content, and history; provider-facing messages are a projection of that state.

***

## Overview

### The problem

When context is just a message array or a concatenated prompt:

- **Edits become string surgery.** Replacing one instruction or removing a tool
  exchange means finding and rebuilding the right pieces by hand.
- **History and current context get mixed together.** What happened is not always
  what the model should see next.
- **Branches become copies.** Trying a different continuation means inventing
  another way to track snapshots and their relationship.
- **Storage becomes application plumbing.** Every project needs its own way to
  save, inspect, and reload context.

### The Pactree approach

- **Addressable components** → select context by ID, type, or structural
  relationship, then edit the parts you need.
- **Versioned changes** → retain transaction traces and revisions alongside the
  current tree.
- **Forks and rewind** → explore another continuation or return to an earlier
  revision.
- **Provider-neutral rendering** → build messages from the tree while checking
  tool-call/result pairing. Your adapter handles the provider's wire format.
- **Compact local files** → store context in `.pact` files with indexed reads,
  deduplicated content, and compression.
- **One engine, multiple interfaces** → use Rust directly, Python bindings, or
  the CLI without maintaining separate implementations.

***

## Quick Start

### 1. Build from source

Use **Rust 1.93 or newer**. For Python, also use **Python 3.11 or newer**.
Clone the two repositories beside each other:

```bash
git clone https://github.com/earldennison/pactree.git
git clone https://github.com/earldennison/pactree-py.git
cd pactree-py
python3 -m venv .venv
```

Activate the environment:

```bash
source .venv/bin/activate
```

On Windows PowerShell, use `.venv\Scripts\Activate.ps1` instead. Then build the
native Python extension:

```bash
python -m pip install 'maturin>=1.8,<2'
maturin develop --release --locked
```

These instructions use the current source. The matching Python package update
is not yet published on PyPI.

### 2. Create, render, and save context

```python
from pactree import ContextTree

tree = ContextTree()
tree.user("What should we work on next?")

messages = tree.send()
tree.receive("Let's finish the README.")

tree.save("session.pact")
reopened = ContextTree.from_file("session.pact")
print(reopened.project("json"))
```

`send()` prepares provider-neutral messages; it does **not** call a model.
The example supplies a response directly to `receive()`. In an application,
your provider adapter makes the request and passes the response back.

### 3. Inspect the file

Install the CLI from the Rust checkout:

```bash
cd ../pactree
cargo install --path . --locked --bin pactree
pactree --help
```

From the directory containing `session.pact`:

```bash
pactree info session.pact
pactree view head session.pact --format json
pactree check session.pact
```

If you only need the CLI, clone the Rust repository and run the install command
above; Python is not required. Rust applications can use the crate directly;
see the [public API contract](docs/reference/api-stability.md). For more Python
examples, see [pactree-py](https://github.com/earldennison/pactree-py).

***

## Core Concepts

- **Component** — an addressable piece of context: text, a message, a tool call,
  a result, or a media reference.
- **Tree** — the ordered working context you inspect and edit.
- **Transaction trace** — a record of changes and their revision boundaries.
- **Branch** — a context history with its own head, allowing another continuation.
- **Projection** — a view of context for a particular purpose, such as JSON,
  provider-neutral messages, or a window of history.
- **`.pact` file** — the binary storage form for context, content, and history.

See the [architecture guide](docs/architecture/substrate-model.md) for how these
fit together.

## FAQ

**Is this an agent framework?** No. Pactree manages context and storage. Agent
loops, tool execution, model requests, and application policy stay with your
runtime.

**Is it a vector database?** No. Its core model is a versioned context tree, not
an embedding index. It provides selectors and indexed reads over that context.

**Do I have to use Python?** No. Rust is the engine; Python is a binding to it.
The CLI also exposes inspection, queries, file operations, and projections.

**Does it preserve tool exchanges?** Tool calls and results are structured
components. Provider-thread rendering rejects orphan tool calls by default
rather than silently emitting an invalid exchange.

**How stable is it?** Pactree is pre-1.0. See the
[API stability policy](docs/reference/api-stability.md) and
[file-format reference](docs/reference/file-format.md) before integrating it.

## Documentation

- [Documentation index](docs/README.md)
- [Selectors](docs/reference/selectors.md) and [cheat sheet](docs/reference/selector-cheat-sheet.md)
- [Provider-thread rendering](docs/reference/provider-thread.md)
- [Content references and bundles](docs/reference/content-refs-and-bundles.md)
- [CLI reference](docs/reference/cli.md)
- [Application integration](docs/integrations/host-adapter-contract.md)
- [Contributing](CONTRIBUTING.md)

## License

MIT. See [LICENSE](LICENSE). Bundled third-party data and dependencies retain
their respective terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
