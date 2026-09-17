#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_mvmr.sh

Builds the pinned official MVMR image and smoke-tests it.

Environment:
  MVMR_IMAGE     Image tag (default localhost/atc/mvmr:0.4.8)
  BUILD_IMAGE=0  Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${MVMR_IMAGE:-localhost/atc/mvmr:0.4.8}
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

export AUTONOMICS_PANEL_CACHE_ROOT=${AUTONOMICS_PANEL_CACHE_ROOT:-$HOME/.autonomics/panels}

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/mvmr/Dockerfile" \
    -t "$image" "$root/containers/mvmr"
fi
podman run --rm --entrypoint Rscript "$image" \
  -e 'stopifnot(requireNamespace("MVMR", quietly=TRUE))' >/dev/null

podman run --rm \
  -v "$root/containers/mvmr/fixtures:/fixtures:ro,Z" \
  --entrypoint Rscript "$image" -e '
    data <- read.csv("/fixtures/rawdat_mvmr.csv", check.names = FALSE)
    mvmr_input <- MVMR::format_mvmr(
      BXGs = as.matrix(data[, c("LDL_beta", "HDL_beta"), drop = FALSE]),
      BYG = data$SBP_beta,
      seBXGs = as.matrix(data[, c("LDL_se", "HDL_se"), drop = FALSE]),
      seBYG = data$SBP_se,
      RSID = data$SNP
    )
    pcor <- matrix(c(1, 0.3, 0.3, 1), nrow = 2, byrow = TRUE)
    gencov <- MVMR::phenocov_mvmr(
      pcor,
      as.matrix(data[, c("LDL_se", "HDL_se"), drop = FALSE])
    )
    ivw_default <- MVMR::ivw_mvmr(mvmr_input)
    ivw_pcor <- MVMR::ivw_mvmr(mvmr_input, gencov = gencov)
    strength_default <- suppressWarnings(MVMR::strength_mvmr(mvmr_input, 0))
    strength_pcor <- MVMR::strength_mvmr(mvmr_input, gencov)
    pleio_default <- suppressWarnings(MVMR::pleiotropy_mvmr(mvmr_input, 0))
    pleio_pcor <- MVMR::pleiotropy_mvmr(mvmr_input, gencov)
    stopifnot(isTRUE(all.equal(ivw_default, ivw_pcor)))
    stopifnot(!isTRUE(all.equal(strength_default, strength_pcor)))
    stopifnot(!isTRUE(all.equal(pleio_default, pleio_pcor)))
  ' >/dev/null

echo "Official MVMR container test completed successfully."
