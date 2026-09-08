#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_ldsc_rg.sh

Builds the official LDSC image and smoke-tests it.

Environment:
  VFS_CONFIG                    Catalog config (default ~/.autonomics/vfs.toml)
  LDSC_IMAGE                    Image tag (default localhost/atc/ldsc:3.0)
  LDSC_CODE_DIR                 Official source/build context
                                (default /mnt/disk3/ldsc3/ldsc)
  AUTONOMICS_LDSC_IT_SUMSTATS   First standard LDSC sumstats input
  AUTONOMICS_LDSC_RG_IT_SUMSTATS2
                                Second standard LDSC sumstats input
  BUILD_IMAGE=0                 Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
image=${LDSC_IMAGE:-localhost/atc/ldsc:3.0}
code_dir=${LDSC_CODE_DIR:-/mnt/disk3/ldsc3/ldsc}
build_image=${BUILD_IMAGE:-1}

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
[[ -f "$config" ]] || {
  echo "VFS config does not exist: $config" >&2
  exit 1
}
[[ -f "$code_dir/ldsc.py" ]] || {
  echo "official LDSC source is incomplete: $code_dir" >&2
  exit 1
}

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}
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

if [[ "$build_image" == 1 ]]; then
  podman build --layers -f "$root/containers/ldsc/Dockerfile" \
    -t "$image" "$code_dir"
fi
podman run --rm "$image" --help >/dev/null

echo "Official LDSC rg container test completed successfully."
