# Repacking, Reanchoring, And Repairing

Validate first:

```bash
cargo run --quiet -- doctor /tmp/session.pact
```

Write a repaired copy:

```bash
cargo run --quiet -- heal /tmp/session.pact --out /tmp/session.healed.pact
```

Repack or reanchor into a new file:

```bash
cargo run --quiet -- repack /tmp/session.pact /tmp/session.repacked.pact
cargo run --quiet -- reanchor /tmp/session.pact /tmp/session.reanchored.pact --every-cycles 1000
cargo run --quiet -- reindex-head /tmp/session.pact /tmp/session.reindexed.pact
```

These commands write output files instead of mutating the input in place.
