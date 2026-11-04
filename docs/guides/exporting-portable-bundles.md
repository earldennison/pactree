# Exporting Bundles And Slices

Bundle commands inspect and complete payload material for a `.pact` file:

```bash
cargo run --quiet -- bundle create /tmp/session.pact
cargo run --quiet -- bundle verify /tmp/session.pact
cargo run --quiet -- bundle export-full /tmp/session.pact /tmp/session.full.pact
cargo run --quiet -- bundle prune /tmp/session.pact
```

Slice commands produce exchange files:

```bash
cargo run --quiet -- slice 1..10 /tmp/session.pact /tmp/session.1-10.slice.pact
cat > /tmp/slice-spec.json <<'JSON'
{"mode":"branch-with-lineage","branch_id":"main"}
JSON
cargo run --quiet -- file-slice /tmp/session.pact /tmp/main.slice.pact < /tmp/slice-spec.json
```

Import and merge operate on target/source `.pact` files:

```bash
cargo run --quiet -- file-import-check /tmp/main.slice.pact
cargo run --quiet -- import /tmp/session.pact /tmp/main.slice.pact
cargo run --quiet -- merge /tmp/session.pact /tmp/main.slice.pact
```
