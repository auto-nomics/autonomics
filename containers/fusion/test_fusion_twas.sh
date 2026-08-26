#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_fusion_twas.sh

Builds the official FUSION image, verifies the fusion.gtex_v8 catalog panel,
pushes the image to the local registry, and runs the real k3s TWAS regression.

Environment:
  FUSION_REGISTRY            Registry host (default: 192.168.10.24:30500)
  FUSION_IMAGE               OCI tag (default: $FUSION_REGISTRY/atc/fusion:1.0.0)
  FUSION_DIGEST_REFERENCE    Immutable image digest expected by the wrapper
  AUTONOMICS_FUSION_IT_LDREF
                             chr21 BIM used to generate test z-scores
  BUILD_IMAGE=0              Skip podman build
  IMPORT_IMAGE=0             Skip podman push
  RUN_TEST=0                 Skip Rust integration test
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
registry=${FUSION_REGISTRY:-192.168.10.24:30500}
image=${FUSION_IMAGE:-$registry/atc/fusion:1.0.0}
digest_reference=${FUSION_DIGEST_REFERENCE:-192.168.10.24:30500/atc/fusion@sha256:91d11747476967b0131571308f2a64bb12aa459205f282103f6bed4fb69ca0bf}
ldref=${AUTONOMICS_FUSION_IT_LDREF:-/mnt/data/twas_fusion/LDREF/1000G.EUR.21.bim}
build_image=${BUILD_IMAGE:-1}
import_image=${IMPORT_IMAGE:-1}
run_test=${RUN_TEST:-1}

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
need curl
need kubectl
[[ -f "$ldref" ]] || {
  echo "LDREF BIM does not exist: $ldref" >&2
  exit 1
}

export KUBECONFIG=${KUBECONFIG:-"$HOME/.kube/autonomics-k3s.yaml"}
export AUTONOMICS_K3S_NAMESPACE=${AUTONOMICS_K3S_NAMESPACE:-autonomics}
export AUTONOMICS_K3S_WORKSPACE_PVC=${AUTONOMICS_K3S_WORKSPACE_PVC:-autonomics-workspace}
export AUTONOMICS_K3S_WORKSPACE_ROOT=${AUTONOMICS_K3S_WORKSPACE_ROOT:-/var/lib/autonomics/k3s/workspace}
export AUTONOMICS_K3S_PANEL_PVC=${AUTONOMICS_K3S_PANEL_PVC:-autonomics-panels}
export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-/var/lib/autonomics/k3s/panels}
export AUTONOMICS_K3S_PANEL_PVC_PREFIX=${AUTONOMICS_K3S_PANEL_PVC_PREFIX:-}
export AUTONOMICS_K3S_POLL_INTERVAL_MS=${AUTONOMICS_K3S_POLL_INTERVAL_MS:-250}
export AUTONOMICS_FUSION_IT_LDREF=$ldref

current=$(cargo run -q -p data-catalog -- list)
grep -q '"id": "fusion.gtex_v8"' <<<"$current" || {
  echo "catalog current index is missing fusion.gtex_v8" >&2
  exit 1
}

kubectl get node >/dev/null
kubectl get pvc -n "$AUTONOMICS_K3S_NAMESPACE" \
  "$AUTONOMICS_K3S_WORKSPACE_PVC" >/dev/null
kubectl get pvc -n "$AUTONOMICS_K3S_NAMESPACE" \
  "$AUTONOMICS_K3S_PANEL_PVC" >/dev/null

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/fusion/Dockerfile" -t "$image" \
    "$root/containers/fusion"
fi

podman run --rm --tls-verify=false --entrypoint Rscript "$image" \
  /opt/fusion/FUSION.assoc_test.R --help >/dev/null

if [[ "$import_image" == 1 ]]; then
  podman push --tls-verify=false "$image"
  actual_digest=$(curl -fsS \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
    "http://$registry/v2/atc/fusion/manifests/1.0.0" -D - -o /dev/null |
    awk 'tolower($1)=="docker-content-digest:" {gsub(/\r$/, "", $2); print $2}')
  [[ "$digest_reference" == *"$actual_digest" ]] || {
    echo "registry digest changed: $actual_digest; update FUSION_ORIGINAL_IMAGE" >&2
    exit 1
  }
fi

if [[ "$run_test" == 1 ]]; then
  cargo test -p nodes-io --test container_file_flow \
    real_catalog_backed_official_fusion_twas_runs_in_k3s \
    -- --ignored --nocapture
fi

echo "FUSION TWAS official regression completed successfully."
