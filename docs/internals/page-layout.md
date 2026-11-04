# Page Layout

Pactree 0.1 files start with the eight-byte `PACTBIN2` magic inside a fixed
4096-byte header. The header carries file identity and two 128-byte root slots.

Append records use a 56-byte checksummed header containing record kind, schema
version, transaction generation, bounded payload length, and BLAKE3 digest.
The persistent index uses immutable 4096-byte B+tree pages stored inside index
page records. Leaf keys are prefix-compressed; internal pages contain separator
keys and child record addresses.

Root slots point to the commit marker, index root, and committed file-length
watermark for one complete generation. See [file format](../reference/file-format.md).

Source anchors:

- `src/pact_file/layout.rs`
- `src/pact_file/codec.rs`
- `src/pact_file/btree.rs`
- `src/pact_file/root_slot.rs`
