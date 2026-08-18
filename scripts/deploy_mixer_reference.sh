#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: deploy_mixer_reference.sh [SOURCE_DIR] [DEST_DIR]

Environment:
  ID=g1000_eur
  POPULATION=EUR
  GENOME_BUILD=GRCh37
  SOURCE_COMMIT=ea2a445912f83e5767d67372b6075912ed5655d8
  FORCE=1 replace an existing destination
EOF
}

source_dir=${1:-/mnt/disk3/mixer/reference/mixer_data}
dest_dir=${2:-/mnt/data/mixer/resources/g1000_eur}
id=${ID:-g1000_eur}
population=${POPULATION:-EUR}
genome_build=${GENOME_BUILD:-GRCh37}
source_commit=${SOURCE_COMMIT:-ea2a445912f83e5767d67372b6075912ed5655d8}
force=${FORCE:-0}

if [[ ! -d "$source_dir/engine" || ! -f "$source_dir/engine/libbgmg.so" ]]; then
  usage
  echo "Source does not contain engine/libbgmg.so: $source_dir" >&2
  exit 1
fi
for template in \
  "stage/chr@/1000G.EUR.chr@.qc.bim" \
  "ld_mixer/1000G.EUR.chr@" \
  "snps/g1000_eur_chr@.snps"; do
  if [[ ! -e "$source_dir/$template" ]]; then
    echo "Missing source template: $source_dir/$template" >&2
    exit 1
  fi
done
if [[ -e "$dest_dir" && "$force" != "1" ]]; then
  echo "Destination already exists; set FORCE=1 to replace it: $dest_dir" >&2
  exit 1
fi

for metadata in "$id" "$population" "$genome_build" "$source_commit"; do
  if [[ ! "$metadata" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    echo "Bundle metadata contains unsupported characters: $metadata" >&2
    exit 1
  fi
done

rm -rf "$dest_dir"
mkdir -p "$(dirname "$dest_dir")"
cp -a "$source_dir" "$dest_dir"

# gsa-MiXeR has two small pure-Python dependencies that are not packaged by
# every container distribution. Vendor them beside the entrypoint so a managed
# system Python can import them without touching the read-only resource mount.
site_packages=$(find "$dest_dir/engine/.venv/lib" -maxdepth 3 -type d -name site-packages -print -quit)
if [[ -z "$site_packages" ]]; then
  echo "Source engine venv has no site-packages directory" >&2
  exit 1
fi
for package in intervaltree numdifftools sortedcontainers; do
  if [[ ! -d "$site_packages/$package" ]]; then
    echo "Source engine venv is missing dependency: $package" >&2
    exit 1
  fi
  cp -a "$site_packages/$package" "$dest_dir/engine/precimed/"
done

checksum=$(sha256sum "$dest_dir/engine/libbgmg.so" | awk '{print $1}')

cat > "$dest_dir/bundle.json" <<EOF
{
  "schema_version": 1,
  "id": "$id",
  "source_commit": "$source_commit",
  "genome_build": "$genome_build",
  "population": "$population",
  "engine_path": "engine",
  "bim_template": "stage/chr@/1000G.EUR.chr@.qc.bim",
  "ld_template": "ld_mixer/1000G.EUR.chr@",
  "extract_template": "snps/g1000_eur_chr@.snps",
  "engine_sha256": "$checksum"
}
EOF

echo "$dest_dir/bundle.json"
