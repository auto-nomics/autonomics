#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_mvmr.sh

Builds the pinned official MVMR image, imports it into k3s, and runs the real
official-package end-to-end test.

Environment:
  MVMR_IMAGE     Image tag (default localhost/atc/mvmr:0.4.8)
  BUILD_IMAGE=0  Skip podman build
  IMPORT_IMAGE=0 Skip k3s import
  RUN_TEST=0     Skip the Rust ignored test
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${MVMR_IMAGE:-localhost/atc/mvmr:0.4.8}
build_image=${BUILD_IMAGE:-1}
import_image=${IMPORT_IMAGE:-1}
run_test=${RUN_TEST:-1}

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
need kubectl
[[ "$import_image" == 1 ]] && need sudo

export KUBECONFIG=${KUBECONFIG:-"$HOME/.kube/autonomics-k3s.yaml"}
export AUTONOMICS_K3S_NAMESPACE=${AUTONOMICS_K3S_NAMESPACE:-autonomics}
export AUTONOMICS_K3S_WORKSPACE_PVC=${AUTONOMICS_K3S_WORKSPACE_PVC:-autonomics-workspace}
export AUTONOMICS_K3S_WORKSPACE_ROOT=${AUTONOMICS_K3S_WORKSPACE_ROOT:-/var/lib/autonomics/k3s/workspace}
export AUTONOMICS_K3S_PANEL_PVC=${AUTONOMICS_K3S_PANEL_PVC:-autonomics-panels}
export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
export AUTONOMICS_K3S_PANEL_PVC_PREFIX=${AUTONOMICS_K3S_PANEL_PVC_PREFIX:-}
export AUTONOMICS_K3S_POLL_INTERVAL_MS=${AUTONOMICS_K3S_POLL_INTERVAL_MS:-250}

kubectl get node >/dev/null
kubectl get pvc -n "$AUTONOMICS_K3S_NAMESPACE" \
  "$AUTONOMICS_K3S_WORKSPACE_PVC" >/dev/null

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/mvmr/Dockerfile" \
    -t "$image" "$root/containers/mvmr"
fi
podman run --rm --entrypoint Rscript "$image" \
  -e 'stopifnot(requireNamespace("MVMR", quietly=TRUE))' >/dev/null

if [[ "$import_image" == 1 ]]; then
  image_tar=$(mktemp --suffix=.tar)
  cleanup() {
    [[ -n "${image_tar:-}" ]] && rm -f "$image_tar"
  }
  trap cleanup EXIT
  podman save -o "$image_tar" "$image"
  sudo k3s ctr images import "$image_tar"
fi

if [[ "$run_test" == 1 ]]; then
  cargo test -p nodes-io --test container_file_flow \
    real_official_mvmr_runs_in_k3s -- --ignored --nocapture
fi

echo "Official MVMR container test completed successfully."
