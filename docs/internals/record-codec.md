# Record Codec

The record codec compacts common record payloads before binary value encoding.

Important implementation points:

- Canonical records use ordered arrays for common fields.
- Optional fields use compact positions and nulls rather than verbose object
  keys.
- Trace refs are compacted through boundary-specific helpers.
- Record payload bytes are encoded with Pact value tags, including compact
  integers, symbol refs, object-symbol keys, and binary byte sentinels.

Source anchors:

- `src/file_format/record_codec.rs`
- `src/file_format/value_codec.rs`
