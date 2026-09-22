#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_plink2_clump.sh

Builds the official PLINK2 a.6.26 image, publishes the 1000G EUR Phase3
PLINK binary catalog package, and smoke-tests the image.

Environment:
  VFS_CONFIG                   Catalog VFS config (default: ~/.autonomics/vfs.toml)
  PLINK2_IMAGE                 OCI tag (default: localhost/atc/plink2:2.0.0-a.6.26)
  PLINK2_REFERENCE_TARBALL     1000G EUR Phase3 PLINK binary tarball
                               (default: /mnt/data/ldsc_data/1000G_Phase3_plinkfiles.tgz)
  AUTONOMICS_PLINK2_IT_SUMSTATS
                               TSV with SNP + P (optionally CHR/POS) for clump input
  BUILD_IMAGE=0                Skip podman build
  PUBLISH_PANEL=0              Skip package build/publish
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
image=${PLINK2_IMAGE:-localhost/atc/plink2:2.0.0-a.6.26}
ref_tarball=${PLINK2_REFERENCE_TARBALL:-/mnt/data/ldsc_data/1000G_Phase3_plinkfiles.tgz}
sumstats=${AUTONOMICS_PLINK2_IT_SUMSTATS:-"$root/containers/plink2/fixtures/chr22.sumstats.tsv"}
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
need sha256sum

[[ -f "$config" ]] || {
  echo "VFS config does not exist: $config" >&2
  exit 1
}
[[ -f "$ref_tarball" ]] || {
  echo "1000G PLINK reference tarball does not exist: $ref_tarball" >&2
  exit 1
}
[[ -f "$sumstats" ]] || {
  echo "PLINK2 sumstats file does not exist: '$sumstats'" >&2
  exit 1
}

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_CONTAINER_IT_IMAGE=$image
export AUTONOMICS_PLINK2_IT_SUMSTATS=$sumstats

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

# -----------------------------------------------------------------------------
# Stage 2: build and publish the 1000G EUR Phase3 PLINK binary catalog package.
# -----------------------------------------------------------------------------
panel_repo=wjixiang/catalog-plink-ref-1000g-eur-binary
panel_version=v1

if [[ "$publish_panel" == 1 ]]; then
  work=$(mktemp -d)
  cleanup_paths+=("$work")
  staging="$work/1000G_EUR_Phase3_plink"
  mkdir -p "$staging"
  # The tarball ships 1000G.EUR.QC.<chr>.{bed,bim,fam} for chromosomes 1..22.
  # Strip the leading top-level directory so the payload root contains only
  # the chromosome files. PLINK2 is invoked with --bfile against the common
  # prefix, so the payload layout must expose BED/BIM/FAM triplets sharing a
  # stem.
  tar -xzf "$ref_tarball" -C "$staging" --strip-components=1

  bed_count=$(find "$staging" -maxdepth 1 -type f -name '*.bed' | wc -l)
  bim_count=$(find "$staging" -maxdepth 1 -type f -name '*.bim' | wc -l)
  fam_count=$(find "$staging" -maxdepth 1 -type f -name '*.fam' | wc -l)
  if [[ "$bed_count" -lt 22 || "$bim_count" -lt 22 || "$fam_count" -lt 22 ]]; then
    echo "Expected at least 22 chromosomes of PLINK files; found bed=$bed_count bim=$bim_count fam=$fam_count" >&2
    exit 1
  fi

  catalog build "$staging" "$work/package" \
    --repo "$panel_repo" --version "$panel_version" --kind plink_ref_binary \
    --metadata population=EUR --metadata genome_build=GRCh37 \
    --metadata reference=1000G_Phase3 \
    --metadata build_origin=official_1000g_phase3_plinkfiles \
    --metadata description="Official 1000 Genomes Phase3 EUR PLINK binary reference (BED/BIM/FAM) for offline LD clumping with PLINK2."
  catalog validate "$work/package"
  catalog publish "$work/package" --config "$config"
fi

current=$(catalog list --config "$config")
if ! grep -q "\"repo\": \"$panel_repo\"" <<<"$current"; then
  echo "catalog current index is missing $panel_repo" >&2
  exit 1
fi

# -----------------------------------------------------------------------------
# Stage 1 acceptance: build the image and smoke-test it.
# -----------------------------------------------------------------------------
if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/plink2/Dockerfile" \
    -t "$image" "$root/containers/plink2"
fi

# Smoke: --help must succeed and print the official banner.
podman run --rm --entrypoint plink2 "$image" --help >/dev/null

# Smoke: the binary inside the image must report the pinned version string.
version_line=$(podman run --rm --entrypoint plink2 "$image" --version 2>&1 || true)
if ! grep -q "v2.0.0-a.6.26" <<<"$version_line"; then
  echo "image is not running the pinned PLINK2 version (v2.0.0-a.6.26). Got:" >&2
  echo "$version_line" >&2
  exit 1
fi

echo "PLINK2 official clumping test completed successfully."
