#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_hyprcoloc.sh

Builds the official HyPrColoc image and smoke-tests it.

Environment:
  HYPRCOLOC_IMAGE  Image tag (default localhost/atc/hyprcoloc:0.0.2)
  BUILD_IMAGE=0    Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${HYPRCOLOC_IMAGE:-localhost/atc/hyprcoloc:0.0.2}
build_image=${BUILD_IMAGE:-1}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

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
  podman build -f "$root/containers/hyprcoloc/Dockerfile" \
    -t "$image" "$root/containers/hyprcoloc"
fi

podman run --rm --entrypoint Rscript "$image" \
  -e 'stopifnot(requireNamespace("hyprcoloc", quietly=TRUE));
       stopifnot(packageVersion("hyprcoloc") == "0.0.2")' >/dev/null

echo "Official HyPrColoc container test completed successfully."
