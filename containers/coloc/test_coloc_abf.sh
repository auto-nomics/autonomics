#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_coloc_abf.sh

Builds the pinned official coloc image and smoke-tests it.

Environment:
  COLOC_IMAGE     Image tag (default localhost/atc/coloc:5.2.3)
  BUILD_IMAGE=0   Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${COLOC_IMAGE:-localhost/atc/coloc:5.2.3}
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
  podman build -f "$root/containers/coloc/Dockerfile" \
    -t "$image" "$root/containers/coloc"
fi
podman run --rm --entrypoint Rscript "$image" \
  -e 'stopifnot(requireNamespace("coloc", quietly=TRUE));
       stopifnot(packageVersion("coloc") == "5.2.3")' >/dev/null

echo "Official coloc container test completed successfully."
