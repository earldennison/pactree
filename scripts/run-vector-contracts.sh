#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plugin="${PACT_VECTOR_SEARCH_DIR:?set PACT_VECTOR_SEARCH_DIR to a local vector plugin checkout}"
toolchain="${PACT_VECTOR_RUST_TOOLCHAIN:-1.95.0}"

if [[ ! -f "$plugin/Cargo.toml" ]]; then
  printf 'vector plugin repository not found: %s\n' "$plugin" >&2
  exit 2
fi

plugin_tmp="$(mktemp -d)"
manifest_before="$(sha256sum "$plugin/Cargo.toml")"
cleanup() {
  rm -rf "$plugin_tmp"
  if [[ "$(sha256sum "$plugin/Cargo.toml")" != "$manifest_before" ]]; then
    printf 'vector contract runner changed the plugin manifest\n' >&2
    exit 3
  fi
}
trap cleanup EXIT

mkdir -p "$plugin_tmp/plugin"
rsync -a --exclude .git --exclude target "$plugin/" "$plugin_tmp/plugin/"
perl -0pi -e \
  's#pactree = \{ path = "[^"]+" \}#pactree = { path = "'"$root"'" }#' \
  "$plugin_tmp/plugin/Cargo.toml"

resolved="$({
  cargo "+$toolchain" metadata --no-deps --format-version 1 \
    --manifest-path "$plugin_tmp/plugin/Cargo.toml"
} | jq -r '.packages[0].dependencies[] | select(.name == "pactree") | .path')"
if [[ "$(realpath "$resolved")" != "$(realpath "$root")" ]]; then
  printf 'vector plugin resolves %s, expected exact candidate %s\n' \
    "$resolved" "$root" >&2
  exit 2
fi

CARGO_TARGET_DIR="$plugin_tmp/target" \
  cargo "+$toolchain" test --all-targets --manifest-path "$plugin_tmp/plugin/Cargo.toml"
