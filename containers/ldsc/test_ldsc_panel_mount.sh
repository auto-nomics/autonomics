#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_ldsc_panel_mount.sh

Builds and publishes the native LDSC panels, imports the original LDSC image
into k3s, and runs a real catalog-backed h2 integration test.

Environment:
  LDSC_SOURCE_ROOT     Reference data root
                       (default: /mnt/data/ldsc_data)
  VFS_CONFIG           Catalog VFS config (default: ~/.autonomics/vfs.toml)
  LDSC_IMAGE           OCI image tag (default: localhost/atc/ldsc:3.0)
  LDSC_CODE_DIR        Original LDSC source tree used as image context
                       (default: /mnt/disk3/ldsc3/ldsc)
  AUTONOMICS_LDSC_IT_SUMSTATS
                       Gzip LDSC input for h2; the test derives a plain .tsv copy
                       (default: /mnt/data/ldsc_data/sumstats_107/GBMI.Asthma.sumstats.gz)
  KUBECONFIG           k3s kubeconfig
  BUILD_IMAGE=0        Skip podman build
  PUBLISH_PANELS=0     Skip package build/publish
  IMPORT_IMAGE=0       Skip podman save and k3s ctr import
  RUN_TEST=0           Skip the Rust integration test
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
source_root=${LDSC_SOURCE_ROOT:-/mnt/data/ldsc_data}
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
image=${LDSC_IMAGE:-localhost/atc/ldsc:3.0}
code_dir=${LDSC_CODE_DIR:-/mnt/disk3/ldsc3/ldsc}
build_image=${BUILD_IMAGE:-1}
publish_panels=${PUBLISH_PANELS:-1}
import_image=${IMPORT_IMAGE:-1}
run_test=${RUN_TEST:-1}
sumstats=${AUTONOMICS_LDSC_IT_SUMSTATS:-/mnt/data/ldsc_data/sumstats_107/GBMI.Asthma.sumstats.gz}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

need cargo
need podman
need kubectl
if [[ "$import_image" == 1 ]]; then
  need sudo
fi

if [[ ! -f "$config" ]]; then
  echo "VFS config does not exist: $config" >&2
  exit 1
fi

ref_source="$source_root/LDscore"
wld_source="$source_root/1000G_Phase3_weights_hm3_no_MHC"
if [[ ! -d "$ref_source" || ! -d "$wld_source" ]]; then
  usage
  echo "Native LDSC reference data not found under: $source_root" >&2
  exit 1
fi
if [[ ! -f "$code_dir/ldsc.py" || ! -f "$code_dir/munge_sumstats.py" || ! -d "$code_dir/ldscore" ]]; then
  usage
  echo "Original LDSC source tree is incomplete: $code_dir" >&2
  exit 1
fi
if [[ ! -f "$sumstats" ]]; then
  echo "LDSC integration sumstats does not exist: $sumstats" >&2
  exit 1
fi

export KUBECONFIG=${KUBECONFIG:-"$HOME/.kube/autonomics-k3s.yaml"}
export AUTONOMICS_K3S_NAMESPACE=${AUTONOMICS_K3S_NAMESPACE:-autonomics}
export AUTONOMICS_K3S_WORKSPACE_PVC=${AUTONOMICS_K3S_WORKSPACE_PVC:-autonomics-workspace}
export AUTONOMICS_K3S_WORKSPACE_ROOT=${AUTONOMICS_K3S_WORKSPACE_ROOT:-/var/lib/autonomics/k3s/workspace}
export AUTONOMICS_K3S_PANEL_PVC=${AUTONOMICS_K3S_PANEL_PVC:-autonomics-panels}
export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-/var/lib/autonomics/k3s/panels}
export AUTONOMICS_K3S_PANEL_PVC_PREFIX=${AUTONOMICS_K3S_PANEL_PVC_PREFIX:-}
export AUTONOMICS_K3S_POLL_INTERVAL_MS=${AUTONOMICS_K3S_POLL_INTERVAL_MS:-250}
export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_CONTAINER_IT_IMAGE=$image
export AUTONOMICS_LDSC_IT_SUMSTATS=$sumstats

catalog() {
  cargo run -q -p data-catalog --bin autonomics-catalog -- "$@"
}

cleanup_paths=()
cleanup() {
  if [[ ${#cleanup_paths[@]} -gt 0 ]]; then
    rm -rf "${cleanup_paths[@]}"
  fi
}
trap cleanup EXIT

if [[ "$publish_panels" == 1 ]]; then
  work=$(mktemp -d)
  cleanup_paths+=("$work")
  mkdir -p "$work/ref-input" "$work/wld-input"

  ref_count=$(find "$ref_source" -maxdepth 1 -type f -name 'LDscore.*.l2.ldscore.gz' | wc -l)
  ref_m_count=$(find "$ref_source" -maxdepth 1 -type f -name 'LDscore.*.l2.M' | wc -l)
  ref_m550_count=$(find "$ref_source" -maxdepth 1 -type f -name 'LDscore.*.l2.M_5_50' | wc -l)
  wld_count=$(find "$wld_source" -maxdepth 1 -type f -name 'weights.hm3_noMHC.*.l2.ldscore.gz' | wc -l)
  if [[ "$ref_count" != 22 || "$ref_m_count" != 22 || "$ref_m550_count" != 22 || "$wld_count" != 22 ]]; then
    echo "Expected 22 chromosomes of native LDSC data; found scores=$ref_count M=$ref_m_count M_5_50=$ref_m550_count weights=$wld_count" >&2
    exit 1
  fi

  find "$ref_source" -maxdepth 1 -type f \
    \( -name 'LDscore.*.l2.ldscore.gz' -o -name 'LDscore.*.l2.M' -o -name 'LDscore.*.l2.M_5_50' \) \
    -exec cp {} "$work/ref-input/" \;
  find "$wld_source" -maxdepth 1 -type f \
    -name 'weights.hm3_noMHC.*.l2.ldscore.gz' \
    -exec cp {} "$work/wld-input/" \;

  catalog build "$work/ref-input" "$work/ref-package" \
    --id ldsc.ref_ld.1000g_eur.basic --version v1 --kind ldsc_ref_ld_chr \
    --metadata population=EUR --metadata genome_build=GRCh37
  catalog validate "$work/ref-package"
  catalog publish "$work/ref-package" --config "$config"

  catalog build "$work/wld-input" "$work/wld-package" \
    --id ldsc.w_ld.1000g_eur_hm3_no_mhc --version v1 --kind ldsc_w_ld_chr \
    --metadata population=EUR --metadata genome_build=GRCh37
  catalog validate "$work/wld-package"
  catalog publish "$work/wld-package" --config "$config"
fi

current=$(catalog list --config "$config")
for panel_id in ldsc.ref_ld.1000g_eur.basic ldsc.w_ld.1000g_eur_hm3_no_mhc; do
  if ! grep -q "\"id\": \"$panel_id\"" <<<"$current"; then
    echo "catalog current index is missing $panel_id" >&2
    exit 1
  fi
done

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
podman run --rm --entrypoint munge_sumstats "$image" --help >/dev/null

if [[ "$import_image" == 1 ]]; then
  image_tar=$(mktemp --suffix=.tar)
  cleanup_paths+=("$image_tar")
  podman save -o "$image_tar" "$image"
  sudo k3s ctr images import "$image_tar"
fi

if [[ "$run_test" == 1 ]]; then
  cargo test -p nodes-io --test container_file_flow \
    real_catalog_backed_original_ldsc_h2_accepts_tsv_and_gz_in_k3s -- --ignored --nocapture
  cargo test -p nodes-io --test container_file_flow \
    real_official_ldsc_munge_runs_in_k3s -- --ignored --nocapture
fi

echo "LDSC panel mount test completed successfully."
