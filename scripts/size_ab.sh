#!/usr/bin/env bash
# Usage: scripts/size_ab.sh <baseline-git-ref> [turns]
# Builds append_sim at the baseline ref and at HEAD (worktree), verifies each
# binary is byte-stable across repeat runs, then prints the exact size delta.
set -euo pipefail

ref="${1:?baseline git ref required}"
turns="${2:-300}"
repo="$(git rev-parse --show-toplevel)"
tmp="$(mktemp -d)"

cleanup() {
  git -C "$repo" worktree remove --force "$tmp/base" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

run_stable() {
  local label="$1" bin="$2"
  local first="$tmp/${label}_1.pact"
  local second="$tmp/${label}_2.pact"
  "$bin" "$first" "$turns" | tail -1
  "$bin" "$second" "$turns" | tail -1
  local first_size second_size sha
  first_size="$(stat -c%s "$first")"
  second_size="$(stat -c%s "$second")"
  sha="$(sha256sum "$first" | awk '{print $1}')"
  if ! cmp -s "$first" "$second"; then
    echo "${label}_first_size=$first_size ${label}_second_size=$second_size ${label}_repeat=byte-different" >&2
    return 1
  fi
  echo "${label}_size=$first_size ${label}_repeat=byte-identical ${label}_sha256=$sha"
  printf -v "${label}_size" '%s' "$first_size"
  printf -v "${label}_out" '%s' "$first"
}

git -C "$repo" worktree add --detach "$tmp/base" "$ref" >/dev/null
cp "$repo/examples/append_sim.rs" "$tmp/base/examples/append_sim.rs" 2>/dev/null || true
(cd "$tmp/base" && cargo build --release --example append_sim >/dev/null 2>&1)
(cd "$repo" && cargo build --release --example append_sim >/dev/null 2>&1)

run_stable baseline "$tmp/base/target/release/examples/append_sim"
run_stable head "$repo/target/release/examples/append_sim"

delta=$((head_size - baseline_size))
echo "baseline=$baseline_size head=$head_size delta=$delta ($(awk "BEGIN{printf \"%+.2f%%\", ($delta)*100/$baseline_size}"))"
if cmp -s "$baseline_out" "$head_out"; then
  echo "signal=byte-identical"
else
  echo "signal=changed"
fi
