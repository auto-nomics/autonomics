#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_twosamplemr.sh

Builds the pinned official TwoSampleMR image and runs its official-package
numerical baseline.

Environment:
  TWOSAMPLEMR_IMAGE  Image tag (default localhost/atc/twosamplemr:0.7.9)
  BUILD_IMAGE=0      Skip the Podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${TWOSAMPLEMR_IMAGE:-localhost/atc/twosamplemr:0.7.9}
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

if [[ "$build_image" == 1 ]]; then
  podman build --network=host \
    -f "$root/containers/twosamplemr/Dockerfile" \
    -t "$image" "$root/containers/twosamplemr"
fi

baseline=$(mktemp -d)
trap 'rm -rf "$baseline"' EXIT

podman run --rm --network=host \
  -v "$baseline:/out:Z" --entrypoint Rscript "$image" -e \
  'load(system.file("extdata", "test_commondata.RData", package = "TwoSampleMR"));
   input <- data.frame(
     snp = dat$SNP,
     beta_exposure = dat$beta.exposure,
     se_exposure = dat$se.exposure,
     effect_allele_exposure = dat$effect_allele.exposure,
     other_allele_exposure = dat$other_allele.exposure,
     eaf_exposure = dat$eaf.exposure,
     beta_outcome = dat$beta.outcome,
     se_outcome = dat$se.outcome,
     effect_allele_outcome = dat$effect_allele.outcome,
     other_allele_outcome = dat$other_allele.outcome,
     eaf_outcome = dat$eaf.outcome,
     pval_exposure = dat$pval.exposure
   );
   write.table(input, "/out/commondata.tsv", sep = "\t", quote = FALSE, row.names = FALSE)'

podman run --rm --network=host \
  -v "$baseline:/work:Z" --entrypoint Rscript "$image" -e \
  'load(system.file("extdata", "test_commondata.RData", package = "TwoSampleMR"));
   write.table(TwoSampleMR::mr(dat), stdout(), sep = "\t", quote = FALSE, row.names = FALSE)' \
  >"$baseline/mr.tsv"

ivw=$(awk -F '\t' '$5 == "Inverse variance weighted" {print $7}' "$baseline/mr.tsv")
egger=$(awk -F '\t' '$5 == "MR Egger" {print $7}' "$baseline/mr.tsv")

awk -v ivw="$ivw" -v egger="$egger" 'BEGIN {
  if ((ivw - 0.4459 > 1e-3) || (0.4459 - ivw > 1e-3)) exit 1
  if ((egger - 0.5025 > 1e-3) || (0.5025 - egger > 1e-3)) exit 1
}'

echo "Official TwoSampleMR container test completed successfully."
