#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_single_cell_workflow.sh

Builds the single-cell runtime and validates the H5AD-first QC, embedding,
Parquet projection, and reverse-subset contracts.

Environment:
  SINGLE_CELL_IMAGE  Image tag (default localhost/atc/single-cell-preprocessor:0.2.0)
  BUILD_IMAGE=0      Skip the Podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${SINGLE_CELL_IMAGE:-localhost/atc/single-cell-preprocessor:0.2.0}
build_image=${BUILD_IMAGE:-1}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && usage && exit 0
command -v podman >/dev/null || { echo "missing required command: podman" >&2; exit 1; }

scratch=$(mktemp -d)
cleanup() { rm -rf "$scratch"; }
trap cleanup EXIT
flags=(
  --rm
  --network=none
  --read-only
  --security-opt=no-new-privileges
  --userns=keep-id
  --user="$(id -u):$(id -g)"
  --tmpfs=/tmp:rw,nosuid,nodev
  -v "$scratch":/data:Z
)

if [[ "$build_image" == 1 ]]; then
  podman build --network=host \
    -f "$root/containers/single-cell-preprocessor/Dockerfile" \
    -t "$image" "$root/containers/single-cell-preprocessor"
fi

podman run -i "${flags[@]}" "$image" python - <<'PY'
import anndata as ad
import numpy as np
import pandas as pd
import scipy.sparse as sp

n_cells = 12
n_genes = 10
rng = np.random.default_rng(17)
dense = np.zeros((n_cells, n_genes), dtype=np.float32)
for row in range(n_cells):
    genes = rng.choice(n_genes, size=5, replace=False)
    dense[row, genes] = rng.integers(1, 8, size=5)
dense[:, 0] += 1
obs = pd.DataFrame(
    {"sample": [f"s{row % 3}" for row in range(n_cells)]},
    index=pd.Index([f"cell_{row:02d}" for row in range(n_cells)], name="cell_id"),
)
var = pd.DataFrame(index=pd.Index([f"gene_{col:02d}" for col in range(n_genes)], name="gene_id"))
ad.AnnData(X=sp.csr_matrix(dense), obs=obs, var=var).write_h5ad(
    "/data/input.h5ad", compression="lzf"
)
PY

printf '%s\n' '{"max_pct_mt":100}' > "$scratch/qc-params.json"
printf '%s\n' '{"n_pcs":3,"n_neighbors":3,"n_top_genes":6,"resolution":0.5,"random_state":17}' \
  > "$scratch/embed-params.json"
printf '%s\n' '{"include_obsm":["X_umap"]}' > "$scratch/projection-params.json"
printf '%s\n' '{"join_column":"cell_id"}' > "$scratch/subset-params.json"

podman run "${flags[@]}" \
  -e AUTONOMICS_SINGLE_CELL_WORKFLOW=qc_filter \
  -e AUTONOMICS_SINGLE_CELL_PARAMS=/data/qc-params.json \
  -e AUTONOMICS_INPUT0=/data/input.h5ad \
  -e AUTONOMICS_OUTPUT0=/data/qc.h5ad \
  -e AUTONOMICS_OUTPUT1=/data/qc.json \
  "$image" python /opt/autonomics/workflow.py

podman run "${flags[@]}" \
  -e AUTONOMICS_SINGLE_CELL_WORKFLOW=pca_neighbors_umap_leiden \
  -e AUTONOMICS_SINGLE_CELL_PARAMS=/data/embed-params.json \
  -e AUTONOMICS_INPUT0=/data/qc.h5ad \
  -e AUTONOMICS_OUTPUT0=/data/embedded.h5ad \
  -e AUTONOMICS_OUTPUT1=/data/embedded.json \
  "$image" python /opt/autonomics/workflow.py

podman run "${flags[@]}" \
  -e AUTONOMICS_SINGLE_CELL_WORKFLOW=obs_to_parquet \
  -e AUTONOMICS_SINGLE_CELL_PARAMS=/data/projection-params.json \
  -e AUTONOMICS_INPUT0=/data/embedded.h5ad \
  -e AUTONOMICS_OUTPUT0=/data/cells.parquet \
  "$image" python /opt/autonomics/workflow.py

podman run -i "${flags[@]}" "$image" python - <<'PY'
import anndata as ad
import celltypist
import igraph
import leidenalg
import pandas as pd
import pyarrow

assert celltypist.__version__ == "1.7.1"
assert igraph.__version__ == "1.0.0"
assert leidenalg.__version__ == "0.12.0"
assert pyarrow.__version__ == "25.0.1"

qc = ad.read_h5ad("/data/qc.h5ad")
assert qc.shape == (12, 10)
assert set(qc.obs.columns) >= {
    "n_genes_by_counts", "total_counts", "pct_counts_mt", "pct_counts_rb"
}
assert "qc_params" in qc.uns

embedded = ad.read_h5ad("/data/embedded.h5ad")
assert embedded.obsm["X_pca"].shape == (12, 3)
assert embedded.obsm["X_umap"].shape == (12, 2)
assert "connectivities" in embedded.obsp
assert "leiden" in embedded.obs
assert "counts" in embedded.layers

cells = pd.read_parquet("/data/cells.parquet")
assert cells.shape[0] == 12
assert cells.columns[0] == "cell_id"
assert {"obsm_X_umap_0", "obsm_X_umap_1"}.issubset(cells.columns)
cells[["cell_id"]].head(5).to_parquet("/data/selection.parquet", index=False)
PY

podman run "${flags[@]}" \
  -e AUTONOMICS_SINGLE_CELL_WORKFLOW=subset_by_obs \
  -e AUTONOMICS_SINGLE_CELL_PARAMS=/data/subset-params.json \
  -e AUTONOMICS_INPUT0=/data/embedded.h5ad \
  -e AUTONOMICS_INPUT1=/data/selection.parquet \
  -e AUTONOMICS_OUTPUT0=/data/subset.h5ad \
  -e AUTONOMICS_OUTPUT1=/data/subset.json \
  "$image" python /opt/autonomics/workflow.py

podman run -i "${flags[@]}" "$image" python - <<'PY'
import anndata as ad

subset = ad.read_h5ad("/data/subset.h5ad")
assert subset.shape[0] == 5
assert list(subset.obs_names) == [f"cell_{index:02d}" for index in range(5)]
assert subset.obsm["X_umap"].shape == (5, 2)
PY

echo "single-cell H5AD workflow smoke tests completed successfully."
echo "local image manifest digest: $(podman image inspect "$image" --format '{{.Digest}}')"
