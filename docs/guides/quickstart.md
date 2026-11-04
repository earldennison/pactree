# Quickstart

Run examples from the `pactree` repo root.

Create a small PACT tree:

```bash
cat > /tmp/pact-tree.json <<'JSON'
{
  "id": "root",
  "componentType": "root",
  "children": [
    {
      "id": "turn_1",
      "componentType": "message_turn",
      "type": "message_turn",
      "role": "user",
      "pact_depth": 0,
      "children": [
        {"id": "text_1", "componentType": "text_content", "type": "text_content", "offset": 0, "content": "hello"}
      ]
    }
  ]
}
JSON
```

Inspect and select from it:

```bash
cargo run --quiet -- inspect < /tmp/pact-tree.json
cargo run --quiet -- select 'd0 > .text_content' < /tmp/pact-tree.json
```

Create a root store and `.pact` file:

```bash
cargo run --quiet -- root-new main < /tmp/pact-tree.json > /tmp/root-store.json
cargo run --quiet -- file-create /tmp/session.pact < /tmp/root-store.json
```

Read the file:

```bash
cargo run --quiet -- head /tmp/session.pact
cargo run --quiet -- project /tmp/session.pact
cargo run --quiet -- stream /tmp/session.pact current_surface --limit 20
```
