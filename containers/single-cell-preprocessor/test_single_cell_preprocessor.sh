#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_single_cell_preprocessor.sh

Builds the Scanpy runtime image and validates the four-input inspect contract.
Environment:
  SINGLE_CELL_IMAGE  Image tag (default localhost/atc/single-cell-preprocessor:0.1.0)
  BUILD_IMAGE=0      Skip the Podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${SINGLE_CELL_IMAGE:-localhost/atc/single-cell-preprocessor:0.1.0}
build_image=${BUILD_IMAGE:-1}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && usage && exit 0
command -v podman >/dev/null || { echo "missing required command: podman" >&2; exit 1; }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cat > "$scratch/matrix.mtx" <<'EOF'
%%MatrixMarket matrix coordinate integer general
3 4 5
1 1 1
2 1 2
2 2 3
3 3 4
1 4 5
EOF
printf 'cell_a\ncell_b\ncell_c\ncell_d\n' > "$scratch/barcodes.tsv"
printf 'gene_1\tCD3D\tGene Expression\ngene_2\tCD8A\tGene Expression\ngene_3\tMS4A1\tGene Expression\n' \
  > "$scratch/features.tsv"
cat > "$scratch/metadata.tsv" <<'EOF'
cell_id patient treatment tissue cell_type
cell_a p01 baseline tumor CD8
cell_b p01 baseline tumor CD8
cell_c p01 baseline normal B
cell_d p01 baseline blood B
EOF

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/single-cell-preprocessor/Dockerfile" \
    -t "$image" "$root/containers/single-cell-preprocessor"
fi

podman run --rm --network=none --read-only \
  -v "$scratch":/data:Z \
  -e AUTONOMICS_INPUT0=/data/matrix.mtx \
  -e AUTONOMICS_INPUT1=/data/barcodes.tsv \
  -e AUTONOMICS_INPUT2=/data/features.tsv \
  -e AUTONOMICS_INPUT3=/data/metadata.tsv \
  -e AUTONOMICS_OUTPUT0=/data/report.json \
  -e AUTONOMICS_OUTPUT1=/data/profile.h5ad \
  -e AUTONOMICS_SINGLE_CELL_OPERATION=inspect \
  "$image"

python3 - "$scratch/report.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)
assert report["matrix"]["genes"] == 3
assert report["matrix"]["cells"] == 4
assert report["matrix"]["nonzero"] == 5
assert report["alignment"]["cell_ids_aligned"] is True
assert report["matrix"]["expression_loaded"] is False
print("single-cell preprocessor inspect smoke test completed successfully.")
PY
