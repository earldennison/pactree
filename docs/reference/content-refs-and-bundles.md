# Content Refs And Bundles

Pactree separates node structure from larger payloads. Small text can remain
inline. Larger or reusable payloads are stored as content records, chunks,
manifests, or bundle items and referenced from nodes or component records.

## Node Content Refs

`PactContentRef` in `src/node.rs` has:

```json
{
  "id": "content_id",
  "field": "$.content",
  "chars": 1200,
  "preview": "first bytes...",
  "source": {"kind": "import"}
}
```

`field`, `chars`, `preview`, and `source` are optional. Built-in payload slot
policies cover paths such as `$.content`, `$.media_content`, `$.data`,
`$.parameters`, `$.result`, `$.surface`, `$.attrs`, `$.org`, and `$.aux`.

## Store Content Records

The JSON store model has:

```json
{
  "id": "content_id",
  "kind": "node.content",
  "hash": "blake3...",
  "bytes": 2048,
  "chars": 2048,
  "preview": "first chars",
  "storage": {
    "kind": "inline",
    "inline": "payload text"
  }
}
```

`ContentStorageRef.kind` may be `inline` or `external` in reconstructed stores.

## `.pact` Content Records

The binary file format uses:

- `content` records: id, kind, hash, bytes, chars, preview, codec, optional
  inline text.
- `content_chunk` records: content id, ordinal, hash, bytes, codec, encoded
  chunk text, and optional chunk-reference fields.
- `content_manifest` records: manifest id, kind, hash, total bytes/chars, and
  ordered chunk refs.

`file_append_content_manifest_json` appends missing content chunks and a
manifest record. `file_content_manifest_payload_json` projects a manifest
payload by reading referenced chunk content in ordinal order.

## CLI

```bash
pactree content-put provider.request.rows < payload.txt
pactree file-append-content-manifest session.pact < manifest.json
pactree file-content-manifest-payload session.pact <manifest_id>
pactree file-query session.pact content <content_id>
pactree file-query session.pact content-hash <hash>
pactree file-query session.pact content-manifest <manifest_id>
pactree file-query session.pact content-manifest-hash <hash>
```

Manifest input:

```json
{
  "manifest_id": "mf_docs",
  "kind": "text.bundle",
  "chunks": [
    {"key": "intro", "kind": "text/plain", "text": "hello"},
    {"key": "body", "kind": "text/plain", "text": "world"}
  ],
  "aux": {"purpose": "example"}
}
```

## Bundles

Bundle APIs operate on `.pact` files:

```bash
pactree bundle create session.pact
pactree bundle verify session.pact
pactree bundle export-full session.pact session.pact.full
pactree bundle prune session.pact
```

The bundle schema is `pact.bundle.v1`. Current hot `.pact` storage keeps large
component payload material out of inline records when policy requires spilling;
`export-full` writes a file with required bundle payloads included.

`bundle verify` reports whether referenced bundle items are present and how
many items are inline or bundled. `bundle prune` removes unneeded bundle
payloads when possible.

## Hashing And Compression

The file-format content hash scheme is `blake3-128`. Page and content
compression use zstd where enabled; the record page codec can use a Pact record
dictionary.

## Source Anchors

- `src/node.rs`
- `src/store/model.rs`
- `src/file_format.rs`
- `src/file_format/content.rs`
- `src/file_format/layout.rs`
