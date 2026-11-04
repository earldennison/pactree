# Contributing

Pactree uses Rust 1.93 and edition 2024. Install the pinned toolchain and run:

```bash
cargo test --no-default-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
bash scripts/check-test-layout.sh
python3 scripts/audit_panic_surface.py
bash scripts/check-package.sh
```

Tests and every test-only definition belong in separate test files. Production
modules may contain only an external `#[cfg(test)] mod tests;` declaration.

Changes to semantics or storage must preserve the frozen API, schema, and
behavior observations. The optional `scripts/run-consumer-contracts.sh` runner
checks a local Pactree Python checkout selected by `PACTREE_PY_DIR` without
modifying its environment. Performance changes require deterministic
I/O/allocation counters and reproducible benchmark results, not timing alone.

Pactree writes only `PACTBIN2`. Normal readers reject pre-v0.1 files; keep
explicit conversion in `pact-v01-convert` and do not add automatic reader
fallbacks, alternative backends, or production backend flags.

Five ignored `lenskeeper_*` tests accept a caller-supplied converted fixture
through `PACTREE_LENSKEEPER_ARTIFACT`. That fixture is not distributed here;
these checks require its original workload, not an arbitrary `.pact` file.
Run them only when you have the matching fixture:

```bash
PACTREE_LENSKEEPER_ARTIFACT=/path/to/fixture.pact cargo test lenskeeper_ -- --ignored
```
