# Streaming A Surface

Use streams when a projected surface is too large to consume as one response.

```bash
cargo run --quiet -- stream /tmp/session.pact current_surface --limit 50
cargo run --quiet -- stream /tmp/session.pact activity_surface --after node:turn_1 --limit 50
```

For a chat viewport, use the index-only message surface:

```bash
cargo run --quiet -- stream /tmp/session.pact message_surface --before node:turn_40 --limit 40
```

Persist both the returned cursor `row_id` and `seq`. After the file handle is
refreshed, `--after` the cached tail returns only newly appended messages.
For `message_surface`, identity is the `row_id` resolved within the returned
branch head while `seq` is only the current projection-order token. Never
detach a cached id from the head root/branch lineage. An insertion, deletion,
or reorder before the anchor returns a stale-anchor error; issue one
row-id-only bounded request to rebase.
Seq-only message anchors are unsupported because they can alias after a
reorder.

Consumers should preserve `row_id`, `seq`, and row `cursor` values for adjacent
window requests.

See [stream contract](../reference/stream-contract.md).
