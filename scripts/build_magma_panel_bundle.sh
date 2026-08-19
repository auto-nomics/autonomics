#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build_magma_panel_bundle.sh REFERENCE_DIR GENE_LOC [MAGMA_BIN]

Environment:
  ID=bundle ID (default: REFERENCE_DIR basename)
  GENOME_BUILD=GRCh37
  POPULATION=EAS
  GENE_RELEASE=NCBI37.3
  WINDOW_KB=35
  FORCE=1 overwrite an existing annotation/manifest
EOF
}

if [[ $# -lt 2 || $# -gt 3 ]]; then
  usage
  exit 2
fi

reference_dir=$1
gene_loc=$2
magma_bin=${3:-/mnt/data/magma/bin/magma}
id=${ID:-"$(basename "$reference_dir")"}
genome_build=${GENOME_BUILD:-GRCh37}
population=${POPULATION:-EAS}
gene_release=${GENE_RELEASE:-NCBI37.3}
window_kb=${WINDOW_KB:-35}
force=${FORCE:-0}

if [[ ! -d "$reference_dir" ]]; then
  echo "Reference directory does not exist: $reference_dir" >&2
  exit 1
fi
if [[ ! -x "$magma_bin" ]]; then
  echo "MAGMA executable is not runnable: $magma_bin" >&2
  exit 1
fi
if [[ ! "$window_kb" =~ ^[1-9][0-9]*(\.[0-9]+)?$ ]]; then
  echo "WINDOW_KB must be a positive number" >&2
  exit 1
fi
if [[ ! "$id" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
  echo "ID contains unsupported characters: $id" >&2
  exit 1
fi
for metadata in "$genome_build" "$population" "$gene_release"; do
  if [[ ! "$metadata" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    echo "Bundle metadata contains unsupported characters: $metadata" >&2
    exit 1
  fi
done

plink_prefix=$(find "$reference_dir" -maxdepth 1 -type f -name '*.bim' -printf '%f\n' | sed 's/\.bim$//' | head -n1)
if [[ -z "$plink_prefix" ]]; then
  echo "No .bim file found at the top level of $reference_dir" >&2
  exit 1
fi
for extension in bed bim fam; do
  path="$reference_dir/$plink_prefix.$extension"
  if [[ ! -s "$path" ]]; then
    echo "Missing or empty reference file: $path" >&2
    exit 1
  fi
done
if [[ ! -s "$gene_loc" ]]; then
  echo "Missing or empty gene location file: $gene_loc" >&2
  exit 1
fi

annotation_dir="$reference_dir/annotations"
annotation="$annotation_dir/$id.$gene_release.window$window_kb.genes.annot"
manifest="$reference_dir/bundle.json"
if [[ -e "$annotation" || -e "$manifest" ]] && [[ "$force" != "1" ]]; then
  echo "Bundle annotation or manifest already exists; set FORCE=1 to replace it" >&2
  exit 1
fi

scratch=$(mktemp -d)
cleanup() { rm -rf "$scratch"; }
trap cleanup EXIT

# MAGMA writes chromosome labels verbatim into .genes.annot, while its gene
# analysis path expects numeric PLINK chromosome codes. Normalize only the
# temporary input so source gene-location files remain unchanged.
normalized_gene_loc="$scratch/gene_loc.txt"
awk '
  BEGIN { OFS = "\t" }
  NF && $1 !~ /^#/ {
    if (toupper($2) == "X") $2 = 23
    else if (toupper($2) == "Y") $2 = 24
    else if (toupper($2) == "XY") $2 = 25
    else if (toupper($2) == "MT" || toupper($2) == "M") $2 = 26
    print
    next
  }
  { print }
' "$gene_loc" > "$normalized_gene_loc"

"$magma_bin" \
  --annotate \
  --snp-loc "$reference_dir/$plink_prefix.bim" \
  --gene-loc "$normalized_gene_loc" \
  --out "$scratch/$id"

generated="$scratch/$id.genes.annot"
if [[ ! -s "$generated" ]]; then
  echo "MAGMA did not generate $generated" >&2
  exit 1
fi

mkdir -p "$annotation_dir"
mv "$generated" "$annotation"
checksum=$(sha256sum "$annotation" | awk '{print $1}')

cat > "$manifest" <<EOF
{
  "schema_version": 1,
  "id": "$id",
  "genome_build": "$genome_build",
  "population": "$population",
  "plink_prefix": "$plink_prefix",
  "gene_annotation": {
    "release": "$gene_release",
    "window_kb": $window_kb,
    "path": "annotations/$(basename "$annotation")",
    "sha256": "$checksum"
  }
}
EOF

echo "$manifest"
