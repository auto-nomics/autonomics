#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_smr_heidi.sh

Builds and publishes the official SMR 1.4.2 image, verifies the Westra BESD
catalog package, and runs the real catalog-backed chr22 baseline in k3s.

Environment:
  SMR_REGISTRY             Registry host (default 192.168.10.24:30500)
  SMR_LOCAL_IMAGE          Build tag (default localhost/atc/smr:1.4.2)
  SMR_IMAGE                Published tag (default $SMR_REGISTRY/atc/smr:1.4.2)
  SMR_DIGEST_REFERENCE     Immutable reference expected by the wrapper
  BUILD_IMAGE=0            Skip podman build
  PUSH_IMAGE=0             Skip registry push
  RUN_TEST=0               Skip the Rust ignored test
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
registry=${SMR_REGISTRY:-192.168.10.24:30500}
local_image=${SMR_LOCAL_IMAGE:-localhost/atc/smr:1.4.2}
image=${SMR_IMAGE:-$registry/atc/smr:1.4.2}
digest_reference=${SMR_DIGEST_REFERENCE:-$registry/atc/smr@sha256:40c0db3c71eda506913c376ab939027fff8ce8eb55c262fd1e5da2fe4c351b6d}
build_image=${BUILD_IMAGE:-1}
push_image=${PUSH_IMAGE:-1}
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
need curl
need kubectl

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

catalog_json=$(cargo run -p data-catalog --bin autonomics-catalog -- \
  list --config ~/.autonomics/vfs.toml)
grep -q '"id": "smr.eqtl.westra_hg19"' <<<"$catalog_json" || {
  echo "catalog current index is missing smr.eqtl.westra_hg19" >&2
  exit 1
}
grep -q '"id": "plink.ref.1000g_eur.binary"' <<<"$catalog_json" || {
  echo "catalog current index is missing plink.ref.1000g_eur.binary" >&2
  exit 1
}

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/smr/Dockerfile" \
    -t "$local_image" "$root/containers/smr"
  podman tag "$local_image" "$image"
elif [[ "$push_image" == 1 ]]; then
  podman tag "$local_image" "$image"
fi

if [[ "$push_image" == 1 ]]; then
  podman push --tls-verify=false "$image"
fi

actual_digest=$(curl -fsS \
  -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
  "http://$registry/v2/atc/smr/manifests/1.4.2" -D - -o /dev/null |
  tr -d '\r' | awk 'tolower($1)=="docker-content-digest:" {print $2}')
expected_digest=${digest_reference##*@}
[[ "$actual_digest" == "$expected_digest" ]] || {
  echo "published SMR digest mismatch: expected $expected_digest, got $actual_digest" >&2
  exit 1
}

help_text=$(podman run --rm --tls-verify=false "$image" --help 2>&1 || true)
grep -q 'Version 1.4.2 Linux' <<<"$help_text" || {
  echo "registry image is not running official SMR 1.4.2; got:" >&2
  echo "$help_text" >&2
  exit 1
}

if [[ "$run_test" == 1 ]]; then
  cargo test -p nodes-io --test container_file_flow \
    real_catalog_backed_official_smr_heidi_runs_in_k3s -- --ignored --nocapture
fi

echo "Official SMR container test completed successfully."
