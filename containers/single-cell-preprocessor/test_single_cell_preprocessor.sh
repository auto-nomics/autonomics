#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_single_cell_preprocessor.sh

Builds the Scanpy runtime image and validates the four-input inspect and ingest
contracts.

Environment:
  SINGLE_CELL_IMAGE  Image tag (default localhost/atc/single-cell-preprocessor:0.2.0)
  BUILD_IMAGE=0      Skip the Podman build
  SINGLE_CELL_PRODUCTION_BASELINE=1
                     Also generate and inspect a deterministic 20,000-gene by
                     50,000-cell gzip MatrixMarket fixture with 20,000,000
                     nonzero values
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${SINGLE_CELL_IMAGE:-localhost/atc/single-cell-preprocessor:0.2.0}
build_image=${BUILD_IMAGE:-1}
production_baseline=${SINGLE_CELL_PRODUCTION_BASELINE:-0}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && usage && exit 0
command -v podman >/dev/null || { echo "missing required command: podman" >&2; exit 1; }
command -v python3 >/dev/null || { echo "missing required command: python3" >&2; exit 1; }

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

cat > "$scratch/matrix.mtx" <<'EOF'
%%MatrixMarket matrix coordinate integer general
% MatrixMarket comments are legal before the dimension line.
3 4 5
1 1 1
2 1 2
2 2 3
3 3 4
1 4 5
EOF
printf 'cell_a\ncell_b\ncell_c\ncell_d\n' > "$scratch/barcodes.tsv"
printf 'gene_1\tCD3D\tGene Expression\ngene_2\tCD3D\tGene Expression\ngene_3\tMS4A1\tGene Expression\n' \
  > "$scratch/features.tsv"
cat > "$scratch/metadata.tsv" <<'EOF'
cell_id patient treatment tissue cell_type
cell_a p01 baseline tumor CD8
cell_b p01 baseline tumor CD8
cell_c p01 baseline normal B
cell_d p01 baseline blood B
EOF

if [[ "$build_image" == 1 ]]; then
  podman build --network=host \
    -f "$root/containers/single-cell-preprocessor/Dockerfile" \
    -t "$image" "$root/containers/single-cell-preprocessor"
fi

podman run "${runtime_flags[@]}" \
  -v "$scratch":/data:Z \
  -e AUTONOMICS_INPUT0=/data/matrix.mtx \
  -e AUTONOMICS_INPUT1=/data/barcodes.tsv \
  -e AUTONOMICS_INPUT2=/data/features.tsv \
  -e AUTONOMICS_INPUT3=/data/metadata.tsv \
  -e AUTONOMICS_OUTPUT0=/data/report.json \
  -e AUTONOMICS_OUTPUT1=/data/profile.h5ad \
  -e AUTONOMICS_SINGLE_CELL_OPERATION=inspect \
  "$image"

podman run "${runtime_flags[@]}" \
  -v "$scratch":/data:Z \
  "$image" python -c '
import json
import os
import sys

import anndata as ad
import numpy as np

with open("/data/report.json", encoding="utf-8") as handle:
    report = json.load(handle)
assert report["schema_version"] == "1.0"
assert report["operation"] == "inspect"
matrix = report["matrix"]
assert matrix["format"] == "matrix_market_coordinate"
assert matrix["field"] == "integer"
assert matrix["genes"] == 3
assert matrix["cells"] == 4
assert matrix["nonzero"] == 5
assert matrix["bytes"] == os.path.getsize("/data/matrix.mtx")
assert matrix["gzip"] is False
assert matrix["expression_loaded"] is False
assert report["alignment"]["cell_id_order_preserved"] is True

profile = ad.read_h5ad("/data/profile.h5ad")
assert profile.shape == (4, 3)
assert profile.X is None
assert list(profile.obs_names) == ["cell_a", "cell_b", "cell_c", "cell_d"]
assert list(profile.var_names) == ["CD3D", "CD3D-1", "MS4A1"]
assert list(profile.var["gene_id"]) == ["gene_1", "gene_2", "gene_3"]
assert list(profile.var["feature_type"]) == ["Gene Expression"] * 3
assert np.array_equal(
    profile.obs["tissue"].to_numpy(), ["tumor", "tumor", "normal", "blood"]
)
assert sys.argv[1:] == ["/data/report.json", "/data/profile.h5ad"]
' /data/report.json /data/profile.h5ad

podman run "${runtime_flags[@]}" \
  -v "$scratch":/data:Z \
  -e AUTONOMICS_INPUT0=/data/matrix.mtx \
  -e AUTONOMICS_INPUT1=/data/barcodes.tsv \
  -e AUTONOMICS_INPUT2=/data/features.tsv \
  -e AUTONOMICS_INPUT3=/data/metadata.tsv \
  -e AUTONOMICS_OUTPUT0=/data/ingest-report.json \
  -e AUTONOMICS_OUTPUT1=/data/preprocessed.h5ad \
  -e AUTONOMICS_SINGLE_CELL_OPERATION=ingest \
  -e AUTONOMICS_SINGLE_CELL_MIN_GENES=1 \
  -e AUTONOMICS_SINGLE_CELL_MIN_CELLS=1 \
  -e AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL=true \
  "$image"

podman run "${runtime_flags[@]}" \
  -v "$scratch":/data:Z \
  "$image" python -c '
import json

import anndata as ad
import numpy as np

with open("/data/ingest-report.json", encoding="utf-8") as handle:
    report = json.load(handle)
assert report["operation"] == "ingest"
assert report["matrix"]["expression_loaded"] is True
assert report["matrix"]["output_cells"] == 4
assert report["matrix"]["output_genes"] == 3

preprocessed = ad.read_h5ad("/data/preprocessed.h5ad")
assert preprocessed.shape == (4, 3)
assert list(preprocessed.var_names) == ["CD3D", "CD3D-1", "MS4A1"]
assert list(preprocessed.var["gene_id"]) == ["gene_1", "gene_2", "gene_3"]
counts = preprocessed.layers["counts"].toarray()
assert np.array_equal(counts.sum(axis=1), [3, 3, 4, 5])
expected_x = np.log1p(counts / counts.sum(axis=1, keepdims=True) * 10_000)
assert np.allclose(preprocessed.X.toarray(), expected_x)
assert preprocessed.obs["patient"].tolist() == ["p01"] * 4
'

if [[ "$production_baseline" == 1 ]]; then
  echo "Generating deterministic 20000 x 50000 MatrixMarket baseline..."
  python3 - "$scratch" <<'PY'
import gzip
import sys
from pathlib import Path

root = Path(sys.argv[1])
genes = 20_000
cells = 50_000
nonzeros_per_cell = 400
nonzeros = cells * nonzeros_per_cell

with gzip.open(root / "large-matrix.mtx.gz", "wt", encoding="ascii", compresslevel=1) as handle:
    handle.write("%%MatrixMarket matrix coordinate integer general\n")
    handle.write(f"{genes} {cells} {nonzeros}\n")
    for cell in range(cells):
        base = (cell * 37) % genes
        for offset in range(nonzeros_per_cell):
            gene = (base + offset) % genes
            value = 1 + ((cell + offset) % 8)
            handle.write(f"{gene + 1} {cell + 1} {value}\n")

with (root / "large-features.tsv").open("w", encoding="ascii") as handle:
    for gene in range(genes):
        handle.write(f"gene_{gene:05d}\tSYMBOL_{gene:05d}\tGene Expression\n")

with (root / "large-barcodes.tsv").open("w", encoding="ascii") as handle:
    for cell in range(cells):
        handle.write(f"cell_{cell:05d}\n")

with (root / "large-metadata.tsv").open("w", encoding="ascii") as handle:
    handle.write("cell_id\tpatient\ttissue\n")
    for cell in range(cells):
        handle.write(f"cell_{cell:05d}\tpatient_{cell % 25:02d}\ttissue_{cell % 7}\n")
PY

  podman run "${runtime_flags[@]}" --memory=2g --cpus=2 \
    -v "$scratch":/data:Z \
    -e AUTONOMICS_INPUT0=/data/large-matrix.mtx.gz \
    -e AUTONOMICS_INPUT1=/data/large-barcodes.tsv \
    -e AUTONOMICS_INPUT2=/data/large-features.tsv \
    -e AUTONOMICS_INPUT3=/data/large-metadata.tsv \
    -e AUTONOMICS_OUTPUT0=/data/large-report.json \
    -e AUTONOMICS_OUTPUT1=/data/large-profile.h5ad \
    -e AUTONOMICS_SINGLE_CELL_OPERATION=inspect \
    "$image"

  python3 - "$scratch/large-report.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)
assert report["matrix"]["genes"] == 20_000
assert report["matrix"]["cells"] == 50_000
assert report["matrix"]["nonzero"] == 20_000_000
assert report["matrix"]["gzip"] is True
assert report["matrix"]["expression_loaded"] is False
PY
fi

echo "single-cell preprocessor inspect and ingest smoke tests completed successfully."
if [[ "$production_baseline" == 1 ]]; then
  echo "production-scale header-only MatrixMarket baseline completed successfully."
fi
echo "local image manifest digest: $(podman image inspect "$image" --format '{{.Digest}}')"
