#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo run --quiet --manifest-path "$root/tools/test-layout/Cargo.toml" -- "$root/src"
