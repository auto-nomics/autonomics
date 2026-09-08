#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_magma_annotate.sh

Builds the official MAGMA image, validates the gene-location catalog package,
and smoke-tests the image.

Environment:
  VFS_CONFIG                 Catalog VFS config (default: ~/.autonomics/vfs.toml)
  MAGMA_IMAGE                OCI tag (default: localhost/atc/magma:1.10)
  MAGMA_SOURCE_ROOT          Official MAGMA deployment root
                             (default: /mnt/data/magma)
  AUTONOMICS_MAGMA_IT_SNP_LOC
                             Three-column SNP location smoke input
  BUILD_IMAGE=0              Skip podman build
  PUBLISH_PANEL=0            Skip package build/publish
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
image=${MAGMA_IMAGE:-localhost/atc/magma:1.10}
source_root=${MAGMA_SOURCE_ROOT:-/mnt/data/magma}
snp_loc=${AUTONOMICS_MAGMA_IT_SNP_LOC:-$source_root/results/smoke_test.annotation.snp.loc}
build_image=${BUILD_IMAGE:-1}
publish_panel=${PUBLISH_PANEL:-1}

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

[[ -f "$config" ]] || {
  echo "VFS config does not exist: $config" >&2
  exit 1
}
[[ -x "$source_root/bin/magma" ]] || {
  echo "official MAGMA static binary is missing: $source_root/bin/magma" >&2
  exit 1
}
[[ -f "$snp_loc" ]] || {
  echo "MAGMA SNP-location input does not exist: $snp_loc" >&2
  exit 1
}

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_MAGMA_IT_SNP_LOC=$snp_loc

catalog() {
  cargo run -q -p data-catalog --bin autonomics-catalog -- "$@"
}

if [[ "$publish_panel" == 1 ]]; then
  work=$(mktemp -d)
  cleanup_paths=("$work")
  cleanup() {
    if [[ ${#cleanup_paths[@]} -gt 0 ]]; then
      rm -rf "${cleanup_paths[@]}"
    fi
  }
  trap cleanup EXIT
  mkdir -p "$work/gene-location"
  cp "$source_root/resources/genes/NCBI37.3.gene.loc" "$work/gene-location/"

  catalog build "$work/gene-location" "$work/package" \
    --id magma.gene_loc.ncbi37_3 --version v1 --kind magma_gene_loc \
    --metadata population=multi --metadata genome_build=GRCh37 \
    --metadata release=NCBI37.3 \
    --metadata description="Official MAGMA NCBI37.3 gene location table"
  catalog validate "$work/package"
  catalog publish "$work/package" --config "$config"
else
  cleanup_paths=()
  cleanup() {
    if [[ ${#cleanup_paths[@]} -gt 0 ]]; then
      rm -rf "${cleanup_paths[@]}"
    fi
  }
  trap cleanup EXIT
fi

current=$(catalog list --config "$config")
grep -q '"id": "magma.gene_loc.ncbi37_3"' <<<"$current" || {
  echo "catalog current index is missing magma.gene_loc.ncbi37_3" >&2
  exit 1
}

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/magma/Dockerfile" \
    -t "$image" "$source_root"
fi
podman run --rm "$image" --version >/dev/null

echo "MAGMA official annotation test completed successfully."
