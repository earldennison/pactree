# Indexes

Pactree uses indexes for fast queries but keeps enough record data to rebuild
them.

Indexed/queryable surfaces include branch heads, component fields, view fields,
tool pairs, message order, depth/type/parent/id selector buckets, attrs, aux
namespaces, content ids, content hashes, and content manifests.

`reindex-head` appends rebuilt head/index material to an output file.

Source anchors:

- `src/file_format/query.rs`
- `src/file_format/history.rs`
- `src/file_format/storage.rs`
