#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build_hdl_ukb_panel.sh

Downloads and verifies the official HDL-L UKB EUR LD SVD and BIM payloads,
normalizes them into LD/ and bim/, and builds a local data-catalog package.
Publication is opt-in because the HDL-L runtime target is rootless Podman.

Environment:
  OUTPUT_ROOT       Package output parent (default /mnt/data/hdl_catalog_packages)
  STAGING_ROOT      Download/staging parent (default /mnt/data/hdl_panel_staging)
  DOWNLOAD_PANEL=1  Download missing Zenodo archives
  PUBLISH_PANEL=1   Publish the built package using ~/.autonomics/vfs.toml
  VFS_CONFIG        Catalog config (default ~/.autonomics/vfs.toml)
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
output_root=${OUTPUT_ROOT:-/mnt/data/hdl_catalog_packages}
staging_root=${STAGING_ROOT:-/mnt/data/hdl_panel_staging}
download_panel=${DOWNLOAD_PANEL:-1}
publish_panel=${PUBLISH_PANEL:-0}
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
panel_id=hdl.ref.ukb_eur
panel_version=v1.0

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
need md5sum
need unzip

download_atomic() {
  local url=$1 path=$2 expected=$3
  if [[ ! -f "$path" ]]; then
    [[ "$download_panel" == 1 ]] || {
      echo "required archive is missing: $path" >&2
      exit 1
    }
    mkdir -p "$(dirname "$path")"
    curl -fL --retry 3 --retry-delay 5 "$url" -o "$path.part"
  else
    cp -- "$path" "$path.part"
  fi
  local actual
  actual=$(md5sum "$path.part" | awk '{print $1}')
  [[ "$actual" == "$expected" ]] || {
    echo "checksum mismatch for $path: expected $expected, got $actual" >&2
    rm -f "$path.part"
    exit 1
  }
  mv -- "$path.part" "$path"
}

ld_zip_url=https://zenodo.org/api/records/14825987/files/LDfile.zip/content
bim_zip_url=https://zenodo.org/api/records/14825987/files/bimfile.zip/content
ld_zip=$staging_root/LDfile.zip
bim_zip=$staging_root/bimfile.zip

download_atomic "$ld_zip_url" "$ld_zip" 1d2aa5d3afee8b3bef7836f943476696
download_atomic "$bim_zip_url" "$bim_zip" 608f037e4fc4d69f0ee9231c7fc005bb

work=$(mktemp -d)
cleanup() {
  rm -rf "$work"
}
trap cleanup EXIT

mkdir -p "$work/staging/LD" "$work/staging/bim" "$work/ld-root" "$work/bim-root"
unzip -q "$ld_zip" -d "$work/ld-root"
unzip -q "$bim_zip" -d "$work/bim-root"

ld_marker=$(find "$work/ld-root" -type f -name HDLL_LOC_snps.RData -print -quit)
bim_marker=$(find "$work/bim-root" -type f -name '*.bim' -print -quit)
[[ -n "$ld_marker" && -n "$bim_marker" ]] || {
  echo "official HDL archives do not contain the expected payload roots" >&2
  exit 1
}

cp -a "$(dirname "$ld_marker")/." "$work/staging/LD/"
cp -a "$(dirname "$bim_marker")/." "$work/staging/bim/"

ldsvd_count=$(find "$work/staging/LD" -maxdepth 1 -type f -name '*_LDSVD.rda' | wc -l)
bim_count=$(find "$work/staging/bim" -maxdepth 1 -type f -name '*.bim' | wc -l)
[[ "$ldsvd_count" -ge 2400 && "$bim_count" -ge 2400 ]] || {
  echo "invalid official HDL payload counts: LDSVD=$ldsvd_count BIM=$bim_count" >&2
  exit 1
}
[[ -f "$work/staging/LD/HDLL_LOC_snps.RData" ]] || {
  echo "HDL panel marker is not at LD/HDLL_LOC_snps.RData" >&2
  exit 1
}

catalog() {
  cargo run -q --manifest-path "$root/Cargo.toml" \
    -p data-catalog --bin autonomics-catalog -- "$@"
}

package=$output_root/hdl.ref.ukb_eur-$panel_version
rm -rf "$package"
mkdir -p "$output_root"
catalog build "$work/staging" "$package" \
  --id "$panel_id" --version "$panel_version" --kind hdl_ld_svd_ref \
  --metadata population=EUR --metadata sample_size=335272 \
  --metadata source=zenodo_14825987 \
  --metadata description="Official HDL-L UKB EUR LD SVD and per-block BIM reference."
catalog validate "$package"

if [[ "$publish_panel" == 1 ]]; then
  catalog publish "$package" --config "$config"
fi

echo "HDL UKB panel package built at $package"
