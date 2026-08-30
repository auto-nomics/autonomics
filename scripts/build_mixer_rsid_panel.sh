#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build_mixer_rsid_panel.sh

Builds the rsID-addressed MiXeR GRCh37 EUR panel from two immutable catalog
packages:

1. mixer.g1000_eur v2.2.1: BIM order, binary LD matrices, and tag-SNP set
2. plink.ref.1000g_eur.binary: exact CHR/POS/allele -> rsID translation

The LD matrices and the BIM row order are never changed. Only BIM variant IDs
and tag-SNP IDs that have an exact allele-aware match are translated to rsIDs.
Unmatched BIM rows retain their CHR:POS:A2:A1 IDs so binary LD indexing remains
valid. Unmatched tag SNPs are omitted.

Environment:
  MIXER_PANEL_PACKAGE   Default /mnt/data/mixer_catalog_packages/g1000_eur-v2.2.1
  RSID_PANEL_PACKAGE    Default: current plink.ref.1000g_eur.binary cache entry
  STAGING_DIR           Default /mnt/data/mixer_panel_staging/g1000_eur_rsid-v2.2.1
  OUTPUT_PACKAGE        Default /mnt/data/mixer_catalog_packages/g1000_eur_rsid-v2.2.1-rsid1
  VFS_CONFIG            Catalog config (default ~/.autonomics/vfs.toml)
  BUILD_PACKAGE=1       Build and validate the normalized package
  PUBLISH_PACKAGE=1     Publish the package after validation
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mixer_package=${MIXER_PANEL_PACKAGE:-/mnt/data/mixer_catalog_packages/g1000_eur-v2.2.1}
rsid_package=${RSID_PANEL_PACKAGE:-$(find /mnt/data/panels -maxdepth 1 -type d \
  -name 'plink.ref.1000g_eur.binary@sha256:*' | sort | head -1)}
staging=${STAGING_DIR:-/mnt/data/mixer_panel_staging/g1000_eur_rsid-v2.2.1-rsid1}
output=${OUTPUT_PACKAGE:-/mnt/data/mixer_catalog_packages/g1000_eur_rsid-v2.2.1-rsid1}
config=${VFS_CONFIG:-"$HOME/.autonomics/vfs.toml"}
build_package=${BUILD_PACKAGE:-1}
publish_package=${PUBLISH_PACKAGE:-1}

[[ -f "$mixer_package/manifest.json" ]] || {
  echo "missing MiXeR package manifest: $mixer_package" >&2
  exit 1
}
[[ -n "$rsid_package" && -d "$rsid_package" ]] || {
  echo "missing rsID PLINK cache entry: ${rsid_package:-<unset>}" >&2
  exit 1
}
[[ ! -e "$staging" ]] || {
  echo "staging directory already exists: $staging" >&2
  exit 1
}
if [[ "$build_package" == 1 && -e "$output" && "${FORCE_PACKAGE:-0}" != 1 ]]; then
  echo "output package already exists: $output" >&2
  exit 1
fi

mixer_id=$(jq -r .id "$mixer_package/manifest.json")
mixer_digest=$(jq -r .digest "$mixer_package/manifest.json")
rsid_cache_name=$(basename "$rsid_package")
rsid_id=${rsid_cache_name%@sha256:*}
rsid_digest=sha256:${rsid_cache_name##*@sha256:}
[[ "$mixer_id" == "mixer.g1000_eur" ]] || {
  echo "unexpected MiXeR panel id: $mixer_id" >&2
  exit 1
}
[[ "$rsid_id" == "plink.ref.1000g_eur.binary" ]] || {
  echo "unexpected rsID panel id: $rsid_id" >&2
  exit 1
}
[[ -f "$rsid_package/.autonomics-panel-complete" ]] || {
  echo "rsID PLINK cache entry is incomplete: $rsid_package" >&2
  exit 1
}
grep -qx "digest=$rsid_digest" "$rsid_package/.autonomics-panel-complete" || {
  echo "rsID PLINK cache digest marker mismatch: $rsid_package" >&2
  exit 1
}

mkdir -p "$staging/stage_flat" "$staging/ld_mixer" "$staging/snps"
total_bim=0
mapped_bim=0
total_extract=0
mapped_extract=0

for chromosome in $(seq 1 22); do
  mixer_bim="$mixer_package/payload/stage_flat/chr$chromosome.bim"
  mixer_ld="$mixer_package/payload/ld_mixer/1000G.EUR.chr$chromosome"
  mixer_extract="$mixer_package/payload/snps/g1000_eur_chr$chromosome.snps"
  rsid_bim="$rsid_package/1000G.EUR.QC.$chromosome.bim"

  for required in "$mixer_bim" "$mixer_ld" "$mixer_extract" "$rsid_bim"; do
    [[ -f "$required" ]] || {
      echo "missing required panel file: $required" >&2
      exit 1
    }
  done

  counts=$(awk '
    NR == FNR {
      map[$1 ":" $4 ":" $5 ":" $6] = $2
      next
    }
    {
      total++
      key = $1 ":" $4 ":" $5 ":" $6
      if (key in map) {
        mapped++
        $2 = map[key]
      }
      print
    }
    END { print total, mapped > "/dev/stderr" }
  ' "$rsid_bim" "$mixer_bim" 2>"$staging/.counts" \
    > "$staging/stage_flat/chr$chromosome.bim")
  read bim_total bim_mapped < "$staging/.counts"
  total_bim=$((total_bim + bim_total))
  mapped_bim=$((mapped_bim + bim_mapped))

  cp -al "$mixer_ld" "$staging/ld_mixer/1000G.EUR.chr$chromosome"

  awk '
    NR == FNR {
      map[$1 ":" $4 ":" $5 ":" $6] = $2
      next
    }
    ($0 in map) {
      mapped++
      print map[$0]
    }
    END { print mapped > "/dev/stderr" }
  ' "$rsid_bim" "$mixer_extract" 2>"$staging/.counts" \
    > "$staging/snps/g1000_eur_chr$chromosome.snps"
  read extract_mapped < "$staging/.counts"
  extract_total=$(wc -l < "$mixer_extract")
  total_extract=$((total_extract + extract_total))
  mapped_extract=$((mapped_extract + extract_mapped))
done

cat "$staging"/snps/g1000_eur_chr{1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22}.snps \
  > "$staging/snps/g1000_eur_chr@.snps"
rm "$staging/.counts"

echo "MiXeR source digest:       $mixer_digest"
echo "rsID mapping source:       $rsid_digest"
echo "BIM variants translated:   $mapped_bim / $total_bim"
echo "tag SNPs translated:       $mapped_extract / $total_extract"

if [[ "$build_package" != 1 ]]; then
  echo "staging complete: $staging"
  exit 0
fi

catalog=(cargo run -q -p data-catalog --bin autonomics-catalog --)
"${catalog[@]}" build "$staging" "$output" \
  --id mixer.g1000_eur_rsid \
  --version v2.2.1-rsid1 \
  --kind mixer_reference \
  --metadata population=EUR \
  --metadata genome_build=GRCh37 \
  --metadata marker_scheme=rsid \
  --metadata source_panel_digest="$mixer_digest" \
  --metadata rsid_mapping_panel_digest="$rsid_digest" \
  --metadata bim_variants_total="$total_bim" \
  --metadata bim_variants_rsid="$mapped_bim" \
  --metadata tag_snps_total="$total_extract" \
  --metadata tag_snps_rsid="$mapped_extract" \
  --metadata description='MiXeR GRCh37 EUR panel with exact 1000G rsID translation; LD matrices unchanged' \
  --payload "{\"bim_template\":\"stage_flat/chr@.bim\",\"ld_template\":\"ld_mixer/1000G.EUR.chr@\",\"extract_template\":\"snps/g1000_eur_chr@.snps\",\"source_panel_digest\":\"$mixer_digest\",\"rsid_mapping_panel_digest\":\"$rsid_digest\"}" \
  ${FORCE_PACKAGE:+--force}

"${catalog[@]}" validate "$output"

if [[ "$publish_package" == 1 ]]; then
  "${catalog[@]}" publish "$output" --config "$config"
fi

echo "rsID MiXeR panel package: $output"
