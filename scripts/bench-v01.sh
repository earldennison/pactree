#!/usr/bin/env bash
set -euo pipefail

mode="${1:?usage: scripts/bench-v01.sh current|candidate}"
case "$mode" in
  current|candidate) ;;
  *) echo "mode must be current or candidate" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
evidence_dir="${PACTREE_BENCH_OUTPUT_DIR:-$root/target/benchmark-results}"
evidence="$evidence_dir/performance-$mode.json"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cd "$root"
cargo build --release --bin pactree
PACTREE_STORAGE_AUDIT_BIN="$root/target/release/pactree" \
  cargo bench --bench v01_storage -- "$tmp/workload" >"$tmp/raw.json"

jq \
  --arg mode "$mode" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg benchmark_inputs "$(git hash-object benches/v01_storage.rs scripts/bench-v01.sh src/pact_file/layout_metrics.rs src/pact_file/codec.rs | git hash-object --stdin)" \
  --arg rustc "$(rustc --version --verbose | tr '\n' ' ')" \
  --arg os "$(uname -srm)" \
  '. + {mode: $mode, commit: $commit, provenance: {benchmark_input_tree_git_hash: $benchmark_inputs}, rustc: $rustc, os: $os}' \
  "$tmp/raw.json" >"$tmp/evidence.json"

mkdir -p "$(dirname "$evidence")"
mv "$tmp/evidence.json" "$evidence"

if [[ "$mode" == "candidate" ]]; then
  baseline="${PACTREE_BENCH_BASELINE:-$evidence_dir/performance-current.json}"
  [[ -f "$baseline" ]] || {
    echo "missing baseline evidence: run scripts/bench-v01.sh current first" >&2
    exit 2
  }
  jq -e --slurpfile baseline "$baseline" '
    .workloads["34_turn"].semantic_blake3 == $baseline[0].workloads["34_turn"].semantic_blake3 and
    .workloads["300_turn"].semantic_blake3 == $baseline[0].workloads["300_turn"].semantic_blake3 and
    .workloads["34_turn"].physical_layout.file_bytes == .workloads["34_turn"].file_bytes and
    .workloads["300_turn"].physical_layout.file_bytes == .workloads["300_turn"].file_bytes and
    ([.workloads["34_turn"].physical_layout |
      .envelope_bytes, .canonical_bytes, .checkpoint_bytes, .content_bytes, .facet_bytes,
      .directory_bytes, .allocator_bytes, .commit_bytes] | add) ==
      .workloads["34_turn"].file_bytes and
    ([.workloads["300_turn"].physical_layout |
      .envelope_bytes, .canonical_bytes, .checkpoint_bytes, .content_bytes, .facet_bytes,
      .directory_bytes, .allocator_bytes, .commit_bytes] | add) ==
      .workloads["300_turn"].file_bytes and
    .workloads["34_turn"].file_bytes * 4 <= $baseline[0].workloads["34_turn"].file_bytes * 3 and
    .workloads["300_turn"].file_bytes * 4 <= $baseline[0].workloads["300_turn"].file_bytes * 3 and
    .workloads["300_turn"].file_bytes <= 1000000 and
    .workloads["300_turn"].cold_open_micros <= 60000 and
    .workloads["300_turn"].cold_open_rss_delta_bytes <= 4 * 1024 * 1024 and
    .workloads["300_turn"].write_rss_delta_bytes <= 48 * 1024 * 1024 and
    .workloads["300_turn"].process_peak_rss_bytes <= 64 * 1024 * 1024 and
    .workloads["300_turn"].same_file_20_handles_rss_delta_bytes <= 1024 * 1024 and
    .workloads["300_turn"].same_file_20_handles_rss_delta_bytes <=
      ([$baseline[0].workloads["300_turn"].same_file_20_handles_rss_delta_bytes / 4,
        64 * 1024] | max) and
    .workloads["300_turn"].physical_layout.derived_amplification * 100 <=
      $baseline[0].workloads["300_turn"].physical_layout.derived_amplification * 110 and
    .workloads["600_append"].turns == 600 and
    .workloads["600_append"].physical_layout.file_bytes == .workloads["600_append"].file_bytes and
    .workloads["600_append"].physical_layout.derived_amplification <= 1.25 and
    .workloads["600_append"].physical_layout.compaction_whole_ledger_hydrations == 0 and
    .workloads["600_append"].same_file_20_handles_rss_delta_bytes <= 8 * 1024 * 1024 and
    .workloads["300_turn"].component_by_id_median_micros * 100 <=
      $baseline[0].workloads["300_turn"].component_by_id_median_micros * 105 and
    .workloads["300_turn"].component_by_id_p95_micros * 100 <=
      $baseline[0].workloads["300_turn"].component_by_id_p95_micros * 110 and
    .workloads["300_turn"].content_by_id_median_micros * 100 <=
      ([$baseline[0].workloads["300_turn"].content_by_id_median_micros, 1] | max) * 105 and
    .workloads["300_turn"].content_by_id_p95_micros * 100 <=
      ([$baseline[0].workloads["300_turn"].content_by_id_p95_micros, 1] | max) * 110 and
    .workloads["300_turn"].current_selector_median_micros * 100 <=
      $baseline[0].workloads["300_turn"].current_selector_median_micros * 105 and
    .workloads["300_turn"].current_selector_p95_micros * 100 <=
      $baseline[0].workloads["300_turn"].current_selector_p95_micros * 110 and
    .workloads["300_turn"].stream_20_median_micros * 100 <=
      $baseline[0].workloads["300_turn"].stream_20_median_micros * 105 and
    .workloads["300_turn"].stream_20_p95_micros * 100 <=
      $baseline[0].workloads["300_turn"].stream_20_p95_micros * 110 and
    .workloads["300_turn"].open_median_micros * 100 <=
      $baseline[0].workloads["300_turn"].open_median_micros * 105 and
    .workloads["300_turn"].open_p95_micros * 100 <=
      $baseline[0].workloads["300_turn"].open_p95_micros * 110 and
    .workloads["300_turn"].historical_projection_median_micros * 100 <=
      $baseline[0].workloads["300_turn"].historical_projection_median_micros * 110 and
    .workloads["300_turn"].historical_projection_p95_micros * 100 <=
      $baseline[0].workloads["300_turn"].historical_projection_p95_micros * 110
  ' "$evidence" >/dev/null
fi

cat "$evidence"
