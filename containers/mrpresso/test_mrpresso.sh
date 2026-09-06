#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_mrpresso.sh

Builds the pinned official MRPRESSO image and smoke-tests it.

Environment:
  MRPRESSO_IMAGE   Image tag (default localhost/atc/mrpresso:1.0.0)
  BUILD_IMAGE=0    Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${MRPRESSO_IMAGE:-localhost/atc/mrpresso:1.0.0}
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
  podman build -f "$root/containers/mrpresso/Dockerfile" \
    -t "$image" "$root/containers/mrpresso"
fi
podman run --rm --entrypoint Rscript "$image" \
  -e 'stopifnot(requireNamespace("MRPRESSO", quietly=TRUE))' >/dev/null

echo "Official MRPRESSO container test completed successfully."
