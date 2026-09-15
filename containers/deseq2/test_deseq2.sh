#!/usr/bin/env bash
# shellcheck disable=SC2016
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_deseq2.sh

Builds the pinned official DESeq2 image and validates the complete pasilla
analysis contract.

Environment:
  DESEQ2_IMAGE  Image tag (default localhost/atc/deseq2:1.50.2)
  BUILD_IMAGE=0 Skip the Podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
fixture_dir="$root/containers/deseq2/fixtures"
image=${DESEQ2_IMAGE:-localhost/atc/deseq2:1.50.2}
build_image=${BUILD_IMAGE:-1}

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

need podman
need sha256sum

"$root/containers/deseq2/test_deseq2_fixture.sh"

if [[ "$build_image" == 1 ]]; then
  podman build --network=host \
    -f "$root/containers/deseq2/Dockerfile" \
    -t "$image" \
    "$root/containers/deseq2"
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

runtime_flags=(
  --rm
  --network=none
  --read-only
  --security-opt=no-new-privileges
  --userns=keep-id
  "--user=$(id -u):$(id -g)"
  "--tmpfs=/tmp:rw,nosuid,nodev"
)

run_analysis() {
  local output_dir=$1
  mkdir -p "$output_dir"
  podman run "${runtime_flags[@]}" \
    -v "$fixture_dir/pasilla_gene_counts.tsv:/input/counts.tsv:ro" \
    -v "$fixture_dir/pasilla_sample_metadata.tsv:/input/metadata.tsv:ro" \
    -v "$output_dir:/output" \
    -e AUTONOMICS_INPUT0=/input/counts.tsv \
    -e AUTONOMICS_INPUT1=/input/metadata.tsv \
    -e AUTONOMICS_OUTPUT0=/output/results.tsv \
    -e AUTONOMICS_OUTPUT1=/output/normalized_counts.tsv \
    -e AUTONOMICS_OUTPUT2=/output/size_factors.tsv \
    -e AUTONOMICS_OUTPUT3=/output/deseq2_dataset.rds \
    -e AUTONOMICS_OUTPUT4=/output/run_report.json \
    -e AUTONOMICS_DESEQ2_CONDITION_REFERENCE=untreated \
    -e AUTONOMICS_DESEQ2_CONDITION_TEST=treated \
    -e AUTONOMICS_DESEQ2_COVARIATES=type \
    -e AUTONOMICS_DESEQ2_FIT_TYPE=parametric \
    -e AUTONOMICS_DESEQ2_ALPHA=0.1 \
    -e AUTONOMICS_DESEQ2_THREADS=1 \
    "$image"
}

run_validator() {
  podman run \
    --rm \
    --network=none \
    --read-only \
    --security-opt=no-new-privileges \
    --userns=keep-id \
    "--user=$(id -u):$(id -g)" \
    "--tmpfs=/tmp:rw,nosuid,nodev" \
    -v "$fixture_dir/pasilla_gene_counts.tsv:/input/counts.tsv:ro" \
    -v "$scratch/first:/output" \
    --entrypoint Rscript \
    "$image" --vanilla -e "$1"
}

run_analysis "$scratch/first"
run_analysis "$scratch/second"

for file in results.tsv normalized_counts.tsv size_factors.tsv deseq2_dataset.rds run_report.json; do
  first=$(sha256sum "$scratch/first/$file" | awk '{print $1}')
  second=$(sha256sum "$scratch/second/$file" | awk '{print $1}')
  expected=$(sed -n "s/.*\"${file//./\\.}\": \"\\([0-9a-f]\\{64\\}\\)\".*/\\1/p" \
    "$fixture_dir/pasilla_baseline.json")
  if [[ "$first" != "$second" ]]; then
    echo "nondeterministic output: $file ($first != $second)" >&2
    exit 1
  fi
  if [[ "$first" != "$expected" ]]; then
    echo "baseline mismatch for $file: $first != $expected" >&2
    exit 1
  fi
done

awk -F '\t' '
  BEGIN {
    expected_top_lfc = -3.12676061403957
    expected_pasilla_lfc = -1.8688179971032
  }
  $1 == "FBgn0003360" {
    delta = $3 - expected_top_lfc
    if (delta < 0) delta = -delta
    if (delta > 1e-12) exit 1
  }
  $1 == "FBgn0261552" {
    delta = $3 - expected_pasilla_lfc
    if (delta < 0) delta = -delta
    if (delta > 1e-12) exit 1
    if ($3 >= 0) exit 1
  }
' "$scratch/first/results.tsv"

run_validator '
raw_frame <- read.delim("/input/counts.tsv", check.names = FALSE)
result <- read.delim("/output/results.tsv", check.names = FALSE)
normalized <- read.delim("/output/normalized_counts.tsv", check.names = FALSE)
size_factors <- read.delim("/output/size_factors.tsv", check.names = FALSE)
report <- jsonlite::fromJSON("/output/run_report.json")
dds <- readRDS("/output/deseq2_dataset.rds")

cat("Validating table schema and dimensions\n")
stopifnot(identical(names(result), c("gene_id", "baseMean", "log2FoldChange", "lfcSE", "stat", "pvalue", "padj")))
stopifnot(nrow(raw_frame) == 14599L, nrow(result) == 14599L, nrow(normalized) == 14599L)
stopifnot(
  identical(as.character(raw_frame$gene_id), as.character(result$gene_id)),
  identical(as.character(raw_frame$gene_id), as.character(normalized$gene_id))
)
stopifnot(nrow(size_factors) == 7L, all(is.finite(size_factors$size_factor)), all(size_factors$size_factor > 0))
stopifnot(is(dds, "DESeqDataSet"))
stopifnot(report$engine$package == "DESeq2", report$engine$version == "1.50.2")
stopifnot(report$dimensions$genes == 14599L, report$dimensions$samples == 7L)
stopifnot(report$analysis$design == "~ type + condition")
stopifnot(report$result$genes == 14599L)
'

run_validator '
cat("Validating DESeq2 size-factor estimation\n")
raw_frame <- read.delim("/input/counts.tsv", check.names = FALSE)
size_factors <- read.delim("/output/size_factors.tsv", check.names = FALSE)
raw <- as.matrix(raw_frame[, -1, drop = FALSE])
storage.mode(raw) <- "numeric"
complete_genes <- apply(raw > 0, 1, all)
geometric_reference <- exp(rowMeans(log(raw[complete_genes, , drop = FALSE])))
ratios <- sweep(raw[complete_genes, , drop = FALSE], 1, geometric_reference, "/")
expected_size_factors <- apply(ratios, 2, median)
names(expected_size_factors) <- colnames(raw)
expected_size_factors <- expected_size_factors[size_factors$sample_id]
stopifnot(isTRUE(all.equal(size_factors$size_factor, unname(expected_size_factors), tolerance = 1e-12)))
'

run_validator '
cat("Validating normalized counts and base means\n")
raw_frame <- read.delim("/input/counts.tsv", check.names = FALSE)
result <- read.delim("/output/results.tsv", check.names = FALSE)
normalized <- read.delim("/output/normalized_counts.tsv", check.names = FALSE)
size_factors <- read.delim("/output/size_factors.tsv", check.names = FALSE)
raw <- as.matrix(raw_frame[, -1, drop = FALSE])
storage.mode(raw) <- "numeric"
normalized_matrix <- as.matrix(normalized[, -1, drop = FALSE])
expected_normalized <- sweep(raw, 2, size_factors$size_factor, "/")
stopifnot(isTRUE(all.equal(normalized_matrix, expected_normalized, tolerance = 1e-12)))
stopifnot(isTRUE(all.equal(result$baseMean, rowMeans(expected_normalized), tolerance = 1e-10)))
'

run_validator '
cat("Validating p-value domains\n")
result <- read.delim("/output/results.tsv", check.names = FALSE)
pvalue <- result$pvalue
padj <- result$padj
tested_pvalue <- !is.na(pvalue)
stopifnot(all(is.finite(pvalue[tested_pvalue])))
stopifnot(all(pvalue[tested_pvalue] >= 0), all(pvalue[tested_pvalue] <= 1))
stopifnot(all(is.na(padj) | (padj >= 0 & padj <= 1)))
stopifnot(all(is.na(padj) | !tested_pvalue | padj >= pvalue - 1e-12))
'

mkdir -p "$scratch/mismatch"
sed 's/^untreated4/unknown4/' "$fixture_dir/pasilla_sample_metadata.tsv" \
  >"$scratch/mismatch/metadata.tsv"
if podman run "${runtime_flags[@]}" \
  -v "$fixture_dir/pasilla_gene_counts.tsv:/input/counts.tsv:ro" \
  -v "$scratch/mismatch/metadata.tsv:/input/metadata.tsv:ro" \
  -v "$scratch/mismatch:/output" \
  -e AUTONOMICS_INPUT0=/input/counts.tsv \
  -e AUTONOMICS_INPUT1=/input/metadata.tsv \
  -e AUTONOMICS_OUTPUT0=/output/results.tsv \
  -e AUTONOMICS_OUTPUT1=/output/normalized.tsv \
  -e AUTONOMICS_OUTPUT2=/output/size_factors.tsv \
  -e AUTONOMICS_OUTPUT3=/output/dds.rds \
  -e AUTONOMICS_OUTPUT4=/output/report.json \
  -e AUTONOMICS_DESEQ2_CONDITION_REFERENCE=untreated \
  -e AUTONOMICS_DESEQ2_CONDITION_TEST=treated \
  -e AUTONOMICS_DESEQ2_COVARIATES=type \
  "$image" >/dev/null 2>"$scratch/mismatch/stderr"; then
  echo "mismatched sample IDs were unexpectedly accepted" >&2
  exit 1
fi
rg -q "sample sets must match exactly" "$scratch/mismatch/stderr"

mkdir -p "$scratch/noninteger"
mkdir -p "$scratch/noninteger-output"
printf 'gene_id\ta\tb\nc1\t1\t2\nc2\t1.5\t3\n' >"$scratch/noninteger/counts.tsv"
printf 'sample_id\tcondition\ta\tuntreated\nb\ttreated\n' >"$scratch/noninteger/metadata.tsv"
if podman run "${runtime_flags[@]}" \
  -v "$scratch/noninteger:/input:ro" \
  -v "$scratch/noninteger-output:/output" \
  -e AUTONOMICS_INPUT0=/input/counts.tsv \
  -e AUTONOMICS_INPUT1=/input/metadata.tsv \
  -e AUTONOMICS_OUTPUT0=/output/results.tsv \
  -e AUTONOMICS_OUTPUT1=/output/normalized.tsv \
  -e AUTONOMICS_OUTPUT2=/output/size_factors.tsv \
  -e AUTONOMICS_OUTPUT3=/output/dds.rds \
  -e AUTONOMICS_OUTPUT4=/output/report.json \
  -e AUTONOMICS_DESEQ2_CONDITION_REFERENCE=untreated \
  -e AUTONOMICS_DESEQ2_CONDITION_TEST=treated \
  "$image" >/dev/null 2>"$scratch/noninteger/stderr"; then
  echo "non-integer counts were unexpectedly accepted" >&2
  exit 1
fi
rg -q "finite nonnegative integers" "$scratch/noninteger/stderr"

mkdir -p "$scratch/rank-deficient"
mkdir -p "$scratch/rank-deficient-output"
printf 'gene_id\ta\tb\tc\td\nx\t1\t2\t3\t4\ny\t5\t6\t7\t8\n' \
  >"$scratch/rank-deficient/counts.tsv"
printf 'sample_id\tcondition\tbatch\na\tuntreated\tu\nb\tuntreated\tu\nc\ttreated\tt\nd\ttreated\tt\n' \
  >"$scratch/rank-deficient/metadata.tsv"
if podman run "${runtime_flags[@]}" \
  -v "$scratch/rank-deficient:/input:ro" \
  -v "$scratch/rank-deficient-output:/output" \
  -e AUTONOMICS_INPUT0=/input/counts.tsv \
  -e AUTONOMICS_INPUT1=/input/metadata.tsv \
  -e AUTONOMICS_OUTPUT0=/output/results.tsv \
  -e AUTONOMICS_OUTPUT1=/output/normalized.tsv \
  -e AUTONOMICS_OUTPUT2=/output/size_factors.tsv \
  -e AUTONOMICS_OUTPUT3=/output/dds.rds \
  -e AUTONOMICS_OUTPUT4=/output/report.json \
  -e AUTONOMICS_DESEQ2_CONDITION_REFERENCE=untreated \
  -e AUTONOMICS_DESEQ2_CONDITION_TEST=treated \
  -e AUTONOMICS_DESEQ2_COVARIATES=batch \
  "$image" >/dev/null 2>"$scratch/rank-deficient/stderr"; then
  echo "rank-deficient design was unexpectedly accepted" >&2
  exit 1
fi
rg -q "rank deficient" "$scratch/rank-deficient/stderr"

echo "Official DESeq2 container test completed successfully."
