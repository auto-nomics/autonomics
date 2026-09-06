#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_gcta_containers.sh

Builds the official GCTA 1.95.3 image, verifies/downloads the required panel
payloads, publishes missing catalog packages, pushes the image to the local
registry, and smoke-tests it.

Environment:
  VFS_CONFIG                         Catalog VFS config
                                     (default ~/.autonomics/vfs.toml)
  GCTA_REGISTRY                      Registry host (default 192.168.10.24:30500)
  GCTA_IMAGE                         Immutable image reference expected by the wrapper
  GCTA_1000G_REFERENCE_TARBALL       1000G Phase3 PLINK tarball
  GCTA_GENE_LIST                     Official GCTA hg19 gene list
  DOWNLOAD_PANELS=1                  Download a missing panel payload
  BUILD_IMAGE=1                      Build the image
  PUBLISH_PANEL=1                    Build/publish a missing panel package
  PUSH_IMAGE=1                       Push to the local registry

Panel checksums are enforced. The 1000G payload source is the Broad ALKEs
Group LDSCORE downloads directory; the gene list source is the official GCTA
resource directory.
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
registry=${GCTA_REGISTRY:-192.168.10.24:30500}
local_image=${GCTA_LOCAL_IMAGE:-localhost/atc/gcta:1.95.3}
digest=${GCTA_IMAGE_DIGEST:-sha256:4cbf8c91376f7b314eebf1dfa02ad44028575991cd3d4c4e81b5324942bec20b}
image=${GCTA_IMAGE:-$registry/atc/gcta@$digest}
remote_tag=$registry/atc/gcta:1.95.3
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
ref_tarball=${GCTA_1000G_REFERENCE_TARBALL:-/mnt/data/ldsc_data/1000G_Phase3_plinkfiles.tgz}
gene_list=${GCTA_GENE_LIST:-/mnt/data/gcta_panel_staging/glist-hg19.txt}
download_panels=${DOWNLOAD_PANELS:-1}
build_image=${BUILD_IMAGE:-1}
publish_panel=${PUBLISH_PANEL:-1}
push_image=${PUSH_IMAGE:-1}
ref_url=https://alkesgroup.broadinstitute.org/downloads/LDSCORE/1000G_Phase3_plinkfiles.tgz
gene_url=https://yanglab.westlake.edu.cn/software/gcta/res/glist-hg19.txt
ref_sha256=18383e998035521270d158b0aa4e546d269fc938f6e8490ff6916b644330f5df
gene_sha256=5ee4dbf367912ea4c1ddda3e187e85882a485f4b0aa1b24b61bcb8a4882f404e
plink_panel=plink.ref.1000g_eur.binary
gene_panel=gcta.gene_list.hg19

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
need curl
need podman
need sha256sum
need tar
[[ "$push_image" == 1 ]] && need curl

[[ -f "$config" ]] || {
  echo "VFS config does not exist: $config" >&2
  exit 1
}

verify_sha256() {
  local path=$1 expected=$2
  echo "$expected  $path" | sha256sum -c -
}

download_atomic() {
  local url=$1 path=$2 expected=$3
  mkdir -p "$(dirname "$path")"
  if [[ ! -f "$path" ]]; then
    [[ "$download_panels" == 1 ]] || {
      echo "required panel payload is missing: $path" >&2
      exit 1
    }
    echo "downloading $url" >&2
    curl -fL --retry 3 --retry-delay 2 "$url" -o "$path.part"
    verify_sha256 "$path.part" "$expected"
    mv "$path.part" "$path"
  else
    verify_sha256 "$path" "$expected"
  fi
}

download_atomic "$ref_url" "$ref_tarball" "$ref_sha256"
download_atomic "$gene_url" "$gene_list" "$gene_sha256"

catalog() {
  cargo run -q -p data-catalog --bin autonomics-catalog -- "$@"
}

has_catalog_id() {
  catalog list --config "$config" | grep -q "\"id\": \"$1\""
}

cleanup_paths=()
cleanup() {
  if [[ ${#cleanup_paths[@]} -gt 0 ]]; then
    rm -rf "${cleanup_paths[@]}"
  fi
}
trap cleanup EXIT

publish_missing() {
  local panel_id=$1
  if has_catalog_id "$panel_id"; then
    echo "catalog already contains $panel_id"
    return
  fi
  [[ "$publish_panel" == 1 ]] || {
    echo "catalog is missing $panel_id and PUBLISH_PANEL=0" >&2
    exit 1
  }
}

if ! has_catalog_id "$plink_panel"; then
  publish_missing "$plink_panel"
  work=$(mktemp -d)
  cleanup_paths+=("$work")
  mkdir -p "$work/staging"
  tar -xzf "$ref_tarball" -C "$work/staging" --strip-components=1
  bed_count=$(find "$work/staging" -maxdepth 1 -type f -name '*.bed' | wc -l)
  bim_count=$(find "$work/staging" -maxdepth 1 -type f -name '*.bim' | wc -l)
  fam_count=$(find "$work/staging" -maxdepth 1 -type f -name '*.fam' | wc -l)
  [[ "$bed_count" -eq 22 && "$bim_count" -eq 22 && "$fam_count" -eq 22 ]] || {
    echo "invalid 1000G PLINK payload counts: bed=$bed_count bim=$bim_count fam=$fam_count" >&2
    exit 1
  }
  catalog build "$work/staging" "$work/package" \
    --id "$plink_panel" --version v1 --kind plink_ref_binary \
    --metadata population=EUR --metadata genome_build=GRCh37 \
    --metadata reference=1000G_Phase3 \
    --metadata source=official_1000g_phase3_plinkfiles \
    --metadata sample_size=489 \
    --metadata description="Official 1000G EUR Phase3 PLINK reference for GCTA summary-statistics analyses."
  catalog validate "$work/package"
  catalog publish "$work/package" --config "$config"
fi

if ! has_catalog_id "$gene_panel"; then
  publish_missing "$gene_panel"
  work=$(mktemp -d)
  cleanup_paths+=("$work")
  mkdir -p "$work/staging"
  cp "$gene_list" "$work/staging/glist-hg19.txt"
  gene_count=$(wc -l < "$work/staging/glist-hg19.txt")
  bad_gene_rows=$(awk '!/^([0-9]+|XY|X|Y|M)[[:space:]]+[0-9]+[[:space:]]+[0-9]+[[:space:]]+\S+/ {count++} END{print count+0}' \
    "$work/staging/glist-hg19.txt")
  [[ "$gene_count" -ge 26000 && "$bad_gene_rows" -eq 0 ]] || {
    echo "invalid GCTA hg19 gene list: rows=$gene_count malformed=$bad_gene_rows" >&2
    exit 1
  }
  catalog build "$work/staging" "$work/package" \
    --id "$gene_panel" --version v1 --kind gcta_gene_list \
    --metadata genome_build=hg19 \
    --metadata source=official_gcta_resource \
    --metadata description="Official GCTA hg19 gene list for fastBAT and ACAT-V analyses."
  catalog validate "$work/package"
  catalog publish "$work/package" --config "$config"
fi

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/gcta/Dockerfile" \
    -t "$local_image" "$root/containers/gcta"
fi

if [[ "$push_image" == 1 ]]; then
  podman tag "$local_image" "$remote_tag"
  podman push --tls-verify=false "$remote_tag"
  actual_digest=$(curl -fsS \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
    "http://$registry/v2/atc/gcta/manifests/1.95.3" -D - -o /dev/null |
    tr -d '\r' | awk 'tolower($1)=="docker-content-digest:" {print $2}')
  [[ "$actual_digest" == "$digest" ]] || {
    echo "published GCTA digest mismatch: expected $digest, got $actual_digest" >&2
    exit 1
  }
fi

version_text=$(podman run --rm --tls-verify=false --entrypoint gcta64 "$image" 2>&1 || true)
grep -q "version v1.95.3 Linux" <<<"$version_text" || {
  echo "image is not running official GCTA 1.95.3; got:" >&2
  echo "$version_text" >&2
  exit 1
}

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_GCTA_IT_IMAGE=$image

echo "Official GCTA container tests completed successfully."
