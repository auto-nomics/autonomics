# Single-cell H5AD DAG contract

The main data path is opaque H5AD. Rust owns scheduling, caching, VFS artifacts, and port types; the pinned scverse container is the only component that interprets expression matrices.

## Main path

| Node | Inputs | Outputs |
| --- | --- | --- |
| `h5ad_qc_filter` | `h5ad` | `output.h5ad`, `report.json` |
| `h5ad_pca_neighbors_umap_leiden` | `h5ad` | `output.h5ad`, `report.json` |
| `h5ad_celltypist_annotate` | `h5ad`, `model` | `output.h5ad`, `report.json` |

All artifacts are written beneath the node-specific `/artifacts/{kind}/{run_id}/` prefix. Existing H5AD inputs are staged as private container inputs and are not copied to the output prefix. Each H5AD node preserves unique `obs_names` and `var_names`, keeps sparse CSR data sparse, and retains `layers["counts"]` when it slices or normalizes an existing counts layer.

The embedding node creates `X_pca`, `X_umap`, neighbor graphs in `obsp`, parameters in `uns`, and `obs["leiden"]`. If `X_pca` is already present, it is reused rather than recomputed; this allows a corrected embedding from a future integration node to stay the single source of truth.

## Parquet bypass

| Node | Inputs | Outputs |
| --- | --- | --- |
| `h5ad_obs_to_parquet` | `h5ad` | `cells.parquet` |
| `datafusion_sql` | `parquet` | result `parquet` |
| `h5ad_subset_by_obs` | `h5ad`, `selection_parquet` | `output.h5ad`, `report.json` |

`cells.parquet` starts with `cell_id` derived from `obs_names`, followed by obs columns and explicitly selected `obsm` dimensions named `obsm_{key}_{dimension}`. The SQL node registers the input as table `input` by default and materializes a single-file Parquet result. The reverse bridge keeps H5AD row order and reports matched and missing IDs.

## Workflow shape

```text
file_reference(h5ad)
  -> h5ad_qc_filter
  -> h5ad_pca_neighbors_umap_leiden
  -> h5ad_celltypist_annotate(model=file_reference)

h5ad_qc_filter
  -> h5ad_obs_to_parquet
  -> datafusion_sql
  -> h5ad_subset_by_obs(h5ad=h5ad_qc_filter.output[0])
```

Use `file_reference` with format `h5ad` for an existing VFS or local input. Container network access is disabled; CellTypist models are explicit File inputs. A model must match the input feature space expected by its trained model, and expression should follow that model's preprocessing contract.
