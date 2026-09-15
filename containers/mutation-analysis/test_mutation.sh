#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_mutation.sh

Builds the maftools image and validates the tmb and top_genes contracts with
the deterministic repository fixture.

Environment:
  MUTATION_IMAGE  Image tag (default localhost/atc/mutation-analysis:0.1.0)
  BUILD_IMAGE=0   Skip the Podman build
EOF
}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && usage && exit 0
command -v podman >/dev/null || { echo "missing required command: podman" >&2; exit 1; }
command -v python3 >/dev/null || { echo "missing required command: python3" >&2; exit 1; }

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${MUTATION_IMAGE:-localhost/atc/mutation-analysis:0.1.0}
build_image=${BUILD_IMAGE:-1}
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
runtime_flags=(
  --rm
  --network=none
  --read-only
  --security-opt=no-new-privileges
  --userns=keep-id
  --user="$(id -u):$(id -g)"
  --tmpfs=/tmp:rw,nosuid,nodev
)
cp "$root/fixtures/maf/sample_mutations.maf" "$scratch/sample.maf"
printf 'sample_id\tgroup\nsample_a\tprimary\nsample_b\tcontrol\nsample_c\tprimary\nsample_d\tcontrol\n' \
  > "$scratch/clinical.tsv"

if [[ "$build_image" == 1 ]]; then
  podman build --network=host \
    -f "$root/containers/mutation-analysis/Dockerfile" \
    -t "$image" "$root/containers/mutation-analysis"
fi

run_mutation() {
  local operation=$1
  local report=$2
  local details=$3
  local -a extra_env=()
  if [[ "$operation" == tmb ]]; then
    extra_env=(-e AUTONOMICS_INPUT1=/data/clinical.tsv)
  fi
  podman run "${runtime_flags[@]}" \
    -v "$scratch":/data:Z \
    -e AUTONOMICS_INPUT0=/data/sample.maf \
    "${extra_env[@]}" \
    -e AUTONOMICS_OUTPUT0="$report" \
    -e AUTONOMICS_OUTPUT1="$details" \
    -e MUTATION_CONFIG="$4" \
    "$image"
}

run_mutation tmb /data/tmb.tsv /data/tmb.json \
  '{"operation":"tmb","panel_size_mb":38,"tmb_group_col":"group","tmb_groups":["primary","control"],"gene_col":"Hugo_Symbol","variant_col":"Variant_Classification","tumor_sample_col":"Tumor_Sample_Barcode","variant_type_col":"Variant_Type","chromosome_col":"Chromosome","start_position_col":"Start_Position","end_position_col":"End_Position","reference_allele_col":"Reference_Allele","tumor_seq_allele_col":"Tumor_Seq_Allele2"}'

python3 - "$scratch/tmb.tsv" "$scratch/tmb.json" <<'PY'
import csv
import json
import sys

with open(sys.argv[1], encoding="utf-8", newline="") as handle:
    rows = list(csv.DictReader(handle, delimiter="\t"))
assert len(rows) == 4, rows
assert {"sample_id", "group", "mutations", "panel_size_mb", "tmb_per_mb"} <= rows[0].keys()
assert {row["sample_id"] for row in rows} == {"sample_a", "sample_b", "sample_c", "sample_d"}
with open(sys.argv[2], encoding="utf-8") as handle:
    details = json.load(handle)
assert details["schema_version"] == "1.0"
assert details["operation"] == "tmb"
assert details["variant_count"] == 11
assert details["group_comparison"] == "wilcox:primary_vs_control"
PY

run_mutation top_genes /data/top_genes.tsv /data/top_genes.json \
  '{"operation":"top_genes","top_n":20,"gene_col":"Hugo_Symbol","variant_col":"Variant_Classification","tumor_sample_col":"Tumor_Sample_Barcode","variant_type_col":"Variant_Type","chromosome_col":"Chromosome","start_position_col":"Start_Position","end_position_col":"End_Position","reference_allele_col":"Reference_Allele","tumor_seq_allele_col":"Tumor_Seq_Allele2"}'

python3 - "$scratch/top_genes.tsv" "$scratch/top_genes.json" <<'PY'
import csv
import json
import sys

with open(sys.argv[1], encoding="utf-8", newline="") as handle:
    rows = list(csv.DictReader(handle, delimiter="\t"))
assert len(rows) == 3, rows
assert {"gene_id", "mutation_count", "sample_count", "rank"} <= rows[0].keys()
assert {row["gene_id"] for row in rows} == {"TP53", "KRAS", "BRAF"}
expected_counts = {"TP53": 4, "KRAS": 4, "BRAF": 3}
assert {row["gene_id"]: int(row["mutation_count"]) for row in rows} == expected_counts
with open(sys.argv[2], encoding="utf-8") as handle:
    details = json.load(handle)
assert details["operation"] == "top_genes"
assert details["gene_count"] == 3
PY

echo "mutation analysis tmb and top_genes smoke tests completed successfully."
