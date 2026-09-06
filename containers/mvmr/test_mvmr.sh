#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_mvmr.sh

Builds the pinned official MVMR image and smoke-tests it.

Environment:
  MVMR_IMAGE     Image tag (default localhost/atc/mvmr:0.4.8)
  BUILD_IMAGE=0  Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${MVMR_IMAGE:-localhost/atc/mvmr:0.4.8}
build_image=${BUILD_IMAGE:-1}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && {
  usage
  exit 0
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

need cargo
need podman

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/mvmr/Dockerfile" \
    -t "$image" "$root/containers/mvmr"
fi
podman run --rm --entrypoint Rscript "$image" \
  -e 'stopifnot(requireNamespace("MVMR", quietly=TRUE))' >/dev/null

echo "Official MVMR container test completed successfully."
