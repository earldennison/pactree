# Stream Window Consumer

This integration note is for host UIs or runtimes that consume
`pact.stream.v1`.

Consumers should:

- key rows by `row_id`;
- preserve `seq` and row cursors when requesting adjacent windows;
- use `position.total_rows`, `start_index`, and `end_index` for scroll
  geometry;
- treat `rows` as a window, not the complete projection;
- refresh from `head` when branch or revision changes.

Chat/activity hosts should request `message_surface`, retain the returned
`cursors.before` and `cursors.after`, and pass both cursor `row_id` and `seq`
when paging. After refreshing a held read handle, requesting `after` from the
cached tail reads only newly indexed messages. Message identity is the
`row_id` resolved within the returned branch head; a cache may not detach that
id from `head.root_id`, branch, and mutation lineage. `seq` is only a
projection-order token. Append-only commits preserve the pair. An insertion,
deletion, or reorder before the anchor returns a stale
cursor error so the consumer cannot silently miss or alias a row. Seq-only
message anchors are unsupported. On a stale pair, issue one row-id-only
bounded request to find the anchor's current position, reconcile the lane's
bounded viewport cache, then resume paired cursors. A missing anchor requires
a fresh bounded head window. Neither case permits a full-ledger fallback.
