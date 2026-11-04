#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

list_file="$(mktemp)"
archive_list="$(mktemp)"
crate_dir="$(mktemp -d)"
trap 'rm -f "$list_file" "$archive_list"; rm -rf "$crate_dir"' EXIT

cargo package --allow-dirty --list >"$list_file"

bad_path=0
while IFS= read -r path; do
    case "$path" in
        .cargo_vcs_info.json|Cargo.lock|Cargo.toml|Cargo.toml.orig|README.md|LICENSE|THIRD_PARTY_NOTICES.md|CHANGELOG.md|src/*|tests/*|benches/*|tools/pact-v01-convert/main.rs|examples/*|api/*|contracts/*|release/tool-versions.toml|docs/internals/adapter-boundaries.md|docs/reference/file-format.md)
            ;;
        *)
            printf 'forbidden package path: %s\n' "$path" >&2
            bad_path=1
            ;;
    esac
done <"$list_file"
test "$bad_path" -eq 0

if rg -i '(^|/)(nexus-pipe-|graphify-out|plans|\.git|\.env)|credential|secret' "$list_file"; then
    echo "sensitive or private path in cargo package input" >&2
    exit 1
fi

crate="$root/target/package/pactree-0.1.0.crate"
rm -f "$crate"
cargo package --allow-dirty --no-verify
test -f "$crate"
tar -tzf "$crate" >"$archive_list"

if rg -i '(^|/)(nexus-pipe-|graphify-out|plans|\.git|\.env)|credential|secret' "$archive_list"; then
    echo "sensitive or private path in crate archive" >&2
    exit 1
fi

tar -xzf "$crate" -C "$crate_dir"
if rg -l -I 'nexus_ck_[A-Za-z0-9_]+' "$crate_dir"; then
    echo "client-key-shaped content in crate archive" >&2
    exit 1
fi

crate_bytes="$(wc -c <"$crate" | tr -d ' ')"
unpacked_kib="$(du -sk "$crate_dir" | awk '{print $1}')"
test "$crate_bytes" -le 2097152
test "$unpacked_kib" -le 8192

printf 'package gate passed: %s bytes compressed, %s KiB unpacked\n' \
    "$crate_bytes" "$unpacked_kib"
