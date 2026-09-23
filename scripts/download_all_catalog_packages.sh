#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
config=${CATALOG_CONFIG:-$HOME/.autonomics/vfs.toml}
cache=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
registry_repo=${CATALOG_REGISTRY_REPO:-}
revision=${CATALOG_REVISION:-}
search_limit=${CATALOG_SEARCH_LIMIT:-100000}
retries=${CATALOG_INSTALL_RETRIES:-3}
all_versions=${CATALOG_ALL_VERSIONS:-0}
dry_run=${DRY_RUN:-0}

if [[ -n ${AUTONOMICS_CATALOG_BIN:-} ]]; then
  if [[ -x "$AUTONOMICS_CATALOG_BIN" ]]; then
    catalog=("$AUTONOMICS_CATALOG_BIN")
  elif command -v "$AUTONOMICS_CATALOG_BIN" >/dev/null 2>&1; then
    catalog=("$(command -v "$AUTONOMICS_CATALOG_BIN")")
  else
    echo "error: AUTONOMICS_CATALOG_BIN is not executable: $AUTONOMICS_CATALOG_BIN" >&2
    exit 1
  fi
elif [[ -x "$root/target/release/autonomics-catalog" ]]; then
  catalog=("$root/target/release/autonomics-catalog")
elif [[ -x "$root/target/debug/autonomics-catalog" ]]; then
  catalog=("$root/target/debug/autonomics-catalog")
elif command -v autonomics-catalog >/dev/null 2>&1; then
  catalog=("$(command -v autonomics-catalog)")
else
  catalog=(cargo run --quiet --manifest-path "$root/Cargo.toml" -p data-catalog --bin autonomics-catalog --)
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required to parse autonomics-catalog JSON output" >&2
  exit 1
fi

mkdir -p "$cache"

search_args=(search --limit "$search_limit")
install_args=()
if [[ -n "$registry_repo" ]]; then
  search_args+=(--repo "$registry_repo")
  install_args+=(--config "$config")
else
  if [[ ! -f "$config" ]]; then
    echo "error: catalog config does not exist: $config" >&2
    exit 1
  fi
  search_args+=(--config "$config")
fi
if [[ -n "$revision" ]]; then
  search_args+=(--revision "$revision")
  install_args+=(--revision "$revision")
fi
if [[ "$all_versions" == "1" || "$all_versions" == "true" ]]; then
  search_args+=(--all)
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

echo "Discovering catalog packages..."
"${catalog[@]}" "${search_args[@]}" | jq -r '
  if type != "array" then
    error("catalog search did not return a JSON array")
  end
  | .[]
  | [.repo, .version, .digest, (.current | tostring)]
  | @tsv
' >"$tmp"

total=$(wc -l <"$tmp" | tr -d ' ')
if [[ "$total" == "0" ]]; then
  echo "No catalog packages found."
  exit 0
fi

ok=0
skipped=0
failed=0
declare -a failures=()

while IFS=$'\t' read -r repo version digest current; do
  [[ -n "$repo" && -n "$version" && -n "$digest" ]] || continue

  entry_dir="$cache/$repo@$digest"
  marker="$entry_dir/.autonomics-panel-complete"
  if [[ -f "$entry_dir/manifest.json" && -f "$marker" ]] \
    && [[ "$(<"$marker")" == "digest=$digest" ]]; then
    printf 'skip   %s@%s (%s)\n' "$repo" "$version" "${digest#sha256:}"
    skipped=$((skipped + 1))
    continue
  fi

  if [[ "$dry_run" == "1" || "$dry_run" == "true" ]]; then
    printf 'dry-run %s@%s (%s)%s\n' \
      "$repo" "$version" "${digest#sha256:}" \
      "$([[ "$current" == "true" ]] && printf ' current' || true)"
    continue
  fi

  installed_ok=0
  for ((attempt = 1; attempt <= retries; attempt++)); do
    printf 'install %s@%s (%s), attempt %d/%d\n' \
      "$repo" "$version" "${digest#sha256:}" "$attempt" "$retries"
    if "${catalog[@]}" install "$repo" \
      --version "$version" \
      --digest "$digest" \
      --cache "$cache" \
      "${install_args[@]}"; then
      installed_ok=1
      break
    fi
    if ((attempt < retries)); then
      sleep $((attempt * 5))
    fi
  done

  if ((installed_ok)); then
    ok=$((ok + 1))
  else
    failed=$((failed + 1))
    failures+=("$repo@$version ($digest)")
  fi
done <"$tmp"

echo
echo "Catalog download complete: installed=$ok skipped=$skipped failed=$failed"
if ((failed > 0)); then
  printf 'Failed packages:\n' >&2
  printf '  %s\n' "${failures[@]}" >&2
  exit 1
fi
