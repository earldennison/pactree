#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
py="${PACTREE_PY_DIR:-$root/../pactree-py}"
py_manifest="${PACTREE_PY_MANIFEST:-$py/Cargo.toml}"
py_python="${PACTREE_PY_PYTHON:-$py/.venv/bin/python}"
maturin="${MATURIN:-maturin}"
uv="${UV:-uv}"
consumer_tmp="$(mktemp -d)"

declare -a checkout_names=()
declare -A checkout_paths=()

snapshot_checkout() {
  local name="$1"
  local directory="$2"
  checkout_names+=("$name")
  checkout_paths["$name"]="$directory"
  git -C "$directory" status --porcelain=v1 -z --untracked-files=all \
    >"$consumer_tmp/$name.status.before"
  {
    git -C "$directory" diff --binary --no-ext-diff
    git -C "$directory" diff --cached --binary --no-ext-diff
  } >"$consumer_tmp/$name.diff.before"
}

cleanup() {
  local result=$?
  local name directory
  trap - EXIT
  set +e
  for name in "${checkout_names[@]}"; do
    directory="${checkout_paths[$name]}"
    git -C "$directory" status --porcelain=v1 -z --untracked-files=all \
      >"$consumer_tmp/$name.status.after"
    {
      git -C "$directory" diff --binary --no-ext-diff
      git -C "$directory" diff --cached --binary --no-ext-diff
    } >"$consumer_tmp/$name.diff.after"
    if ! cmp -s "$consumer_tmp/$name.status.before" "$consumer_tmp/$name.status.after" \
      || ! cmp -s "$consumer_tmp/$name.diff.before" "$consumer_tmp/$name.diff.after"; then
      printf 'consumer contract gate changed checkout state: %s\n' "$directory" >&2
      result=3
    fi
  done
  rm -rf "$consumer_tmp"
  exit "$result"
}
trap cleanup EXIT

for directory in "$py"; do
  if [[ ! -d "$directory/.git" && ! -f "$directory/.git" ]]; then
    printf 'consumer git repository not found: %s\n' "$directory" >&2
    exit 2
  fi
done
for executable in "$py_python"; do
  if [[ ! -x "$executable" ]]; then
    printf 'consumer Python interpreter not found: %s\n' "$executable" >&2
    exit 2
  fi
done

resolved_pactree="$({
  cargo metadata --no-deps --format-version 1 --manifest-path "$py_manifest"
} | jq -r '.packages[0].dependencies[] | select(.name == "pactree") | .path')"
if [[ "$(realpath "$resolved_pactree")" != "$(realpath "$root")" ]]; then
  printf 'pactree-py resolves %s, expected exact candidate %s\n' \
    "$resolved_pactree" "$root" >&2
  exit 2
fi

snapshot_checkout pactree_py "$py"

export CARGO_TARGET_DIR="$consumer_tmp/cargo-target"
export PIP_DISABLE_PIP_VERSION_CHECK=1
export PYTHONDONTWRITEBYTECODE=1
export XDG_CACHE_HOME="$consumer_tmp/cache"
export TMPDIR="$consumer_tmp/tmp"
mkdir -p "$TMPDIR"

build_and_install() {
  local name="$1"
  local python="$2"
  local wheelhouse="$consumer_tmp/wheels-$name"
  local site="$consumer_tmp/site-$name"
  mkdir -p "$wheelhouse" "$site"
  "$maturin" build --release --interpreter "$python" \
    --manifest-path "$py_manifest" --out "$wheelhouse" >&2 || return
  local wheel
  wheel="$(find "$wheelhouse" -maxdepth 1 -type f -name '*.whl' -print -quit)"
  if [[ -z "$wheel" ]]; then
    printf 'maturin produced no wheel for %s\n' "$python" >&2
    exit 2
  fi
  "$uv" pip install --python "$python" --no-deps --target "$site" "$wheel" >&2 || return
  printf '%s\n' "$site"
}

py_site="$(build_and_install pactree-py "$py_python")"

cd "$root"
cargo test --test behavior_transcripts --test storage_differential

cd "$py"
PYTHONPATH="$py/src:$py_site" PACTREE_RS_DIR="$root" \
  "$py_python" -m pytest -q -p no:cacheprovider
