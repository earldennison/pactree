# Pactree 0.1 File Format

Pactree 0.1 writes `PACTBIN2`, format version 2, with public schema
`pact.file.v2`. Normal runtime readers reject pre-v0.1 Pact files with a typed
unsupported-format error. Conversion is an explicit, separate operation.

## One-shot comparison converter

The separately packaged `pact-v01-convert` utility creates a v0.1 comparison
file by decoding supported `PACTBIN1` files directly. An older executable can
be supplied as an optional fallback reader to export neutral record JSON if
direct conversion fails:

```text
pact-v01-convert old-session.pact converted-session.pact

pact-v01-convert --pre-v01-pactree /path/to/pactree-0.0.1 \
  old-session.pact converted-session.pact
```

The utility never overwrites either path. It publishes `PACTBIN2` only after a
reopen proves identical playback rows and byte-identical decoded content. Page
layout, compression bytes, indexes, and other physical storage details are not
part of that comparison. The normal `pactree` binary continues to reject the
old file directly.

## Envelope and publication

The file starts with one fixed 4096-byte header containing identity and two
checksummed root slots. A logical boundary appends one canonical transaction:
semantic trace operations, public trace metadata, new content manifests/bodies,
copy-on-write index pages, an index-root delta, optional semantic checkpoint,
and one commit marker.

Publication writes and syncs the append records, replaces the older root slot,
then syncs the file. Open validates both slots and selects the highest complete
generation. A torn append or slot therefore resolves to the exact previous or
next generation, never a partial transaction.

## One index and content spine

All derived indexes share one immutable 4096-byte-page B+tree and namespaced
keys. Updates replace only root paths; old roots remain valid for pinned
snapshots. Full validation rebuilds index expectations from canonical
transactions.

Raw captured payload bytes use BLAKE3-256 internally and are written once per
exact payload. Public content IDs and hashes retain the frozen BLAKE3-128
scheme. Components and transaction manifests retain refs, not duplicate bodies.

## Checkpoints and snapshots

Semantic checkpoints contain canonical component state plus first-seen and
lifecycle metadata. They are derived, content-ref based, and excluded from
observable identity. Their cadence changes replay cost only.

Read handles map only the committed watermark selected at open. A handle stays
pinned after later appends; refresh returns a newer generation. Same-file,
same-generation handles share the immutable mapping and byte-budgeted decoded
page cache.

## Validation

Fast validation checks the envelope, root slots, commit marker, and reachable
index pages. Full validation replays every canonical transaction, verifies
contiguous revisions and trace identity, hashes stored content bodies, checks
derived indexes, and compares checkpoints to replayed component state.
