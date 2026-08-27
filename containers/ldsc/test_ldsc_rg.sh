#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_ldsc_rg.sh

Builds/imports the official LDSC image and runs a real catalog-backed
genetic-correlation test through k3s.

Environment:
  VFS_CONFIG                    Catalog config (default ~/.autonomics/vfs.toml)
  LDSC_IMAGE                    Image tag (default localhost/atc/ldsc:3.0)
  LDSC_CODE_DIR                 Official source/build context
                                (default /mnt/disk3/ldsc3/ldsc)
  AUTONOMICS_LDSC_IT_SUMSTATS   First standard LDSC sumstats input
  AUTONOMICS_LDSC_RG_IT_SUMSTATS2
                                Second standard LDSC sumstats input
  BUILD_IMAGE=0                 Skip podman build
  IMPORT_IMAGE=0                Skip k3s import
  RUN_TEST=0                    Skip the Rust ignored test
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
image=${LDSC_IMAGE:-localhost/atc/ldsc:3.0}
code_dir=${LDSC_CODE_DIR:-/mnt/disk3/ldsc3/ldsc}
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
[[ -f "$config" ]] || {
  echo "VFS config does not exist: $config" >&2
  exit 1
}
[[ -f "$code_dir/ldsc.py" ]] || {
  echo "official LDSC source is incomplete: $code_dir" >&2
  exit 1
}

export KUBECONFIG=${KUBECONFIG:-"$HOME/.kube/autonomics-k3s.yaml"}
export AUTONOMICS_K3S_NAMESPACE=${AUTONOMICS_K3S_NAMESPACE:-autonomics}
export AUTONOMICS_K3S_WORKSPACE_PVC=${AUTONOMICS_K3S_WORKSPACE_PVC:-autonomics-workspace}
export AUTONOMICS_K3S_WORKSPACE_ROOT=${AUTONOMICS_K3S_WORKSPACE_ROOT:-/var/lib/autonomics/k3s/workspace}
export AUTONOMICS_K3S_PANEL_PVC=${AUTONOMICS_K3S_PANEL_PVC:-autonomics-panels}
export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
export AUTONOMICS_K3S_PANEL_PVC_PREFIX=${AUTONOMICS_K3S_PANEL_PVC_PREFIX:-}
export AUTONOMICS_K3S_POLL_INTERVAL_MS=${AUTONOMICS_K3S_POLL_INTERVAL_MS:-250}
export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_LDSC_IT_SUMSTATS=${AUTONOMICS_LDSC_IT_SUMSTATS:-/mnt/data/ldsc_data/sumstats_107/GBMI.Asthma.sumstats.gz}
export AUTONOMICS_LDSC_RG_IT_SUMSTATS2=${AUTONOMICS_LDSC_RG_IT_SUMSTATS2:-/mnt/data/ldsc_data/sumstats_107/PASS.BMI.Yengo2018.sumstats.gz}

[[ -f "$AUTONOMICS_LDSC_IT_SUMSTATS" ]] || {
  echo "first LDSC input does not exist: $AUTONOMICS_LDSC_IT_SUMSTATS" >&2
  exit 1
}
[[ -f "$AUTONOMICS_LDSC_RG_IT_SUMSTATS2" ]] || {
  echo "second LDSC input does not exist: $AUTONOMICS_LDSC_RG_IT_SUMSTATS2" >&2
  exit 1
}

kubectl get node >/dev/null
kubectl get pvc -n "$AUTONOMICS_K3S_NAMESPACE" \
  "$AUTONOMICS_K3S_WORKSPACE_PVC" >/dev/null
kubectl get pvc -n "$AUTONOMICS_K3S_NAMESPACE" \
  "$AUTONOMICS_K3S_PANEL_PVC" >/dev/null

if [[ "$build_image" == 1 ]]; then
  podman build --layers -f "$root/containers/ldsc/Dockerfile" \
    -t "$image" "$code_dir"
fi
podman run --rm "$image" --help >/dev/null

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
    real_catalog_backed_original_ldsc_rg_runs_in_k3s -- --ignored --nocapture
fi

echo "Official LDSC rg container test completed successfully."
