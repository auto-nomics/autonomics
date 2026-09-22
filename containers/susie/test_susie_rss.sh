#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_susie_rss.sh

Builds the official susieR 0.16.6 image, verifies the catalog wjixiang/catalog-mixer-g1000-eur
panel, pushes the image to the local registry, and smoke-tests it.

Environment:
  VFS_CONFIG                   Catalog VFS config (default: ~/.autonomics/vfs.toml)
  SUSIE_IMAGE                  OCI tag pushed to the local registry
  SUSIE_REGISTRY               Registry host (default: 192.168.10.24:30500)
  SUSIE_DIGEST_REFERENCE       Immutable image reference expected by the wrapper
  AUTONOMICS_SUSIE_IT_SUMSTATS
                               TSV with snp/chrom/z (default: committed chr21 fixture)
  BUILD_IMAGE=0                Skip podman build
  IMPORT_IMAGE=0               Skip podman push to the local registry
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
registry=${SUSIE_REGISTRY:-192.168.10.24:30500}
image=${SUSIE_IMAGE:-$registry/atc/susie:0.16.6}
digest_reference=${SUSIE_DIGEST_REFERENCE:-192.168.10.24:30500/atc/susie@sha256:8a72a443461add5c94c4907f9a1d6106b989850e93217a95587d9095542febe8}
sumstats=${AUTONOMICS_SUSIE_IT_SUMSTATS:-"$root/containers/susie/fixtures/chr21.sumstats.tsv"}
build_image=${BUILD_IMAGE:-1}
import_image=${IMPORT_IMAGE:-1}

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
if [[ "$import_image" == 1 ]]; then
  need curl
  curl -fsS "http://$registry/v2/" >/dev/null
fi

[[ -f "$config" ]] || {
  echo "VFS config does not exist: $config" >&2
  exit 1
}
[[ -f "$sumstats" ]] || {
  echo "Susie sumstats fixture does not exist: $sumstats" >&2
  exit 1
}

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_CONTAINER_IT_IMAGE=$image
export AUTONOMICS_SUSIE_IT_SUMSTATS=$sumstats

catalog() {
  cargo run -q -p data-catalog --bin autonomics-catalog -- "$@"
}

current=$(catalog list --config "$config")
if ! grep -q '"id": "wjixiang/catalog-mixer-g1000-eur"' <<<"$current"; then
  echo "catalog current index is missing wjixiang/catalog-mixer-g1000-eur" >&2
  exit 1
fi

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/susie/Dockerfile" \
    -t "$image" "$root/containers/susie"
fi

podman run --rm --tls-verify=false --entrypoint Rscript "$image" -e \
  'stopifnot(packageVersion("susieR") == "0.16.6")'
podman run --rm --tls-verify=false --entrypoint python3 "$image" \
  /opt/susie/bin/susie_ld_query.py --help >/dev/null

if [[ "$import_image" == 1 ]]; then
  podman push --tls-verify=false "$image"
  actual_digest=$(curl -fsS -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
    "http://$registry/v2/atc/susie/manifests/0.16.6" -D - -o /dev/null |
    awk 'tolower($1)=="docker-content-digest:" {gsub(/\r$/, "", $2); print $2}')
  [[ "$digest_reference" == *"$actual_digest" ]] || {
    echo "registry digest changed: $actual_digest; update SUSIE_ORIGINAL_IMAGE" >&2
    exit 1
  }
fi

echo "susieR official susie_rss test completed successfully."
