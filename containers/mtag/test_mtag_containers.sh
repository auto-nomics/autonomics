#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_mtag_containers.sh

Builds the official MTAG image, publishes its dedicated LD panel, prepares a
deterministic two-trait baseline, and runs the catalog-backed rootless-Podman
end-to-end test.

Environment:
  VFS_CONFIG                 Catalog config (default ~/.autonomics/vfs.toml)
  MTAG_IMAGE                 Local image tag (default localhost/atc/mtag:1.0.8)
  BUILD_IMAGE=1              Build the local image
  PUBLISH_PANEL=0            Build/publish mtag.ld_ref.1000g_eur_w_ld
  RUN_TEST=1                 Run the ignored Rust E2E test
  MTAG_IT_VARIANTS           Fixture variant count (default 200000)
  MTAG_IT_INPUT1             First LDSC standard sumstats
  MTAG_IT_INPUT2             Second LDSC standard sumstats
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
source="containers/mtag/mtag"
panel_source="$source/ld_ref_panel/eur_w_ld_chr"

if [[ ! -f "$source/mtag.py" ]]; then
  git submodule update --init "$source"
fi

config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
image=${MTAG_IMAGE:-localhost/atc/mtag:1.0.8}
build_image=${BUILD_IMAGE:-1}
publish_panel=${PUBLISH_PANEL:-0}
run_test=${RUN_TEST:-1}
variants=${MTAG_IT_VARIANTS:-200000}
input1=${MTAG_IT_INPUT1:-/mnt/data/ldsc_data/sumstats_107/GBMI.Asthma.sumstats.gz}
input2=${MTAG_IT_INPUT2:-/mnt/data/ldsc_data/sumstats_107/PASS.BMI.Yengo2018.sumstats.gz}
fixture_root=${MTAG_IT_FIXTURE_ROOT:-/tmp/autonomics-mtag-it}
commit=$(git -C "$source" rev-parse HEAD)

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

need git
need cargo
need python3
need podman

[[ "$commit" == "9e17f3cf1fbcf57b6bc466daefdc51fd0de3c5dc" ]] || {
  echo "unexpected MTAG source commit: $commit" >&2
  exit 1
}
[[ -f "$config" ]] || {
  echo "catalog config does not exist: $config" >&2
  exit 1
}
[[ -f "$input1" && -f "$input2" ]] || {
  echo "one or both MTAG baseline inputs do not exist" >&2
  exit 1
}

catalog() {
  cargo run -q -p data-catalog --bin autonomics-catalog -- "$@"
}

cleanup_paths=()
cleanup() {
  if (( ${#cleanup_paths[@]} > 0 )); then
    rm -rf "${cleanup_paths[@]}"
  fi
}
trap cleanup EXIT

if [[ "$publish_panel" == 1 ]]; then
  work=$(mktemp -d)
  cleanup_paths+=("$work")
  mkdir "$work/payload"
  find "$panel_source" -maxdepth 1 -type f \
    \( -name '[1-9].l2.ldscore.gz' -o -name '1[0-9].l2.ldscore.gz' \
       -o -name '2[0-2].l2.ldscore.gz' -o -name '[1-9].l2.M_5_50' \
       -o -name '1[0-9].l2.M_5_50' -o -name '2[0-2].l2.M_5_50' \) \
    -exec cp {} "$work/payload/" \;
  catalog build "$work/payload" "$work/package" \
    --id mtag.ld_ref.1000g_eur_w_ld --version v1 --kind mtag_ld_ref_chr \
    --metadata population=EUR --metadata genome_build=GRCh37 \
    --metadata 'description=Official MTAG 1.0.8 1000G EUR eur_w_ld_chr LD Score panel' \
    --metadata "source_commit=$commit"
  catalog validate "$work/package"
  catalog publish "$work/package" --config "$config"
fi

catalog list --config "$config" | grep -q '"id": "mtag.ld_ref.1000g_eur_w_ld"'

if [[ "$build_image" == 1 ]]; then
  podman build --layers -f containers/mtag/Dockerfile -t "$image" containers/mtag
fi
podman run --rm "$image" --help >/dev/null
if [[ $(podman run --rm --entrypoint find "$image" /opt /work -type f \
  \( -name '*.ldscore.gz' -o -name '*.M_5_50' -o -name '*.bcor' \) | wc -l) != 0 ]]; then
  echo "MTAG image unexpectedly contains reference panel data" >&2
  exit 1
fi

local_digest_repo=localhost/autonomics/mtag:1.0.8
podman tag "$image" "$local_digest_repo"

mkdir -p "$fixture_root"
python3 containers/mtag/make_baseline_fixture.py \
  --panel-dir "$panel_source" \
  --trait1 "$input1" \
  --trait2 "$input2" \
  --output1 "$fixture_root/trait1.sumstats.tsv" \
  --output2 "$fixture_root/trait2.sumstats.tsv" \
  --max-variants "$variants"

export AUTONOMICS_TEST_VFS_CONFIG=$config
export AUTONOMICS_MTAG_IMAGE_PREFIX=localhost/autonomics
export AUTONOMICS_MTAG_IT_SUMSTATS1="$fixture_root/trait1.sumstats.tsv"
export AUTONOMICS_MTAG_IT_SUMSTATS2="$fixture_root/trait2.sumstats.tsv"

if [[ "$run_test" == 1 ]]; then
  cargo test -p nodes-io --test container_file_flow \
    real_catalog_backed_official_mtag_runs_in_podman -- --ignored --nocapture
fi

echo "Official MTAG Podman container test completed successfully."
