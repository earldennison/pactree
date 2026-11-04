# Querying A Tree Or File

Use `select` for PACT JSON trees:

```bash
cargo run --quiet -- select 'd0 > .text_content' < /tmp/pact-tree.json
cargo run --quiet -- select '#text_1, .message_turn' < /tmp/pact-tree.json
```

Use `file-query` for indexed `.pact` records:

```bash
cargo run --quiet -- file-query /tmp/session.pact node text_1
cargo run --quiet -- file-query /tmp/session.pact depth d0
cargo run --quiet -- file-query /tmp/session.pact type text_content
cargo run --quiet -- file-query /tmp/session.pact node-birth text_1
```

Selector syntax is documented in [selectors](../reference/selectors.md).
