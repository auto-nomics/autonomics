#!/usr/bin/env bash
set -euo pipefail

readonly default_source="crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com/autonomics"
readonly default_destination="ghcr.io/auto-nomics/autonomics"

SOURCE_PREFIX=${SOURCE_PREFIX:-$default_source}
DESTINATION_PREFIX=${DESTINATION_PREFIX:-$default_destination}
CRANE=${CRANE:-crane}
CRANE_JOBS=${CRANE_JOBS:-1}
only=
dry_run=0

usage() {
  cat >&2 <<'EOF'
Usage: containers/migrate-to-ghcr.sh [--dry-run] [--only REPOSITORY]

Environment:
  SOURCE_PREFIX       Source namespace (default: the historical ACR namespace)
  DESTINATION_PREFIX  Destination namespace (default: GHCR)
  CRANE               crane executable

The script copies each immutable source digest to a destination tag. If the
source digest has already been removed but the digest exists at the destination,
the destination is treated as already migrated. Login to GHCR before running.
EOF
}

while (($# > 0)); do
  case "$1" in
    --dry-run)
      dry_run=1
      ;;
    --only)
      (($# >= 2)) || { usage; exit 2; }
      only=$2
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
  shift
done

command -v "$CRANE" >/dev/null 2>&1 || {
  echo "missing required command: $CRANE" >&2
  exit 1
}

inventory="$(dirname "${BASH_SOURCE[0]}")/image-inventory.tsv"
[[ -f "$inventory" ]] || {
  echo "missing inventory: $inventory" >&2
  exit 1
}

count=0
failed=0
while IFS=$'\t' read -r repository digest tag; do
  if [[ -z "$repository" || "$repository" == \#* ]]; then
    continue
  fi
  [[ "$digest" == sha256:* ]] || {
    echo "$repository: invalid digest `$digest`" >&2
    failed=1
    continue
  }
  [[ "$tag" =~ ^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$ ]] || {
    echo "$repository: invalid destination tag `$tag`" >&2
    failed=1
    continue
  }
  if [[ -n "$only" && "$repository" != "$only" ]]; then
    continue
  fi

  count=$((count + 1))
  source_ref="${SOURCE_PREFIX}/${repository}@${digest}"
  destination_ref="${DESTINATION_PREFIX}/${repository}:${tag}"

  if ((dry_run)); then
    printf '%s\n  source      %s\n  destination %s\n' "$repository" "$source_ref" "$destination_ref"
    continue
  fi

  source_digest=$("$CRANE" digest "$source_ref" 2>/dev/null) || source_digest=
  destination_digest=$("$CRANE" digest "$destination_ref" 2>/dev/null) || destination_digest=

  if [[ -n "$destination_digest" && "$destination_digest" == "$digest" ]]; then
    echo "$repository:$tag already migrated ($digest)"
    continue
  fi
  if [[ -n "$destination_digest" && "$destination_digest" != "$digest" ]]; then
    echo "$repository:$tag points to $destination_digest, expected $digest" >&2
    failed=1
    continue
  fi

  if [[ -z "$source_digest" ]]; then
    if digest_at_destination=$("$CRANE" digest "${DESTINATION_PREFIX}/${repository}@${digest}" 2>/dev/null); then
      "$CRANE" tag "${DESTINATION_PREFIX}/${repository}@${digest}" "$tag"
      echo "$repository:$tag restored from destination digest $digest"
      continue
    fi
    echo "$repository: source digest is unavailable and destination digest is absent" >&2
    failed=1
    continue
  fi
  [[ "$source_digest" == "$digest" ]] || {
    echo "$repository: source digest mismatch: expected $digest, got $source_digest" >&2
    failed=1
    continue
  }

  "$CRANE" copy --jobs "$CRANE_JOBS" "$source_ref" "$destination_ref"
  actual_digest=$("$CRANE" digest "$destination_ref")
  [[ "$actual_digest" == "$digest" ]] || {
    echo "$repository:$tag verification failed: expected $digest, got $actual_digest" >&2
    failed=1
    continue
  }
  echo "$repository:$tag migrated ($digest)"
done <"$inventory"

((count > 0)) || {
  echo "no inventory rows selected" >&2
  exit 1
}
((failed == 0)) || exit 1
