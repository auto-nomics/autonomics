# Single-cell H5AD DAG contract

The main data path is opaque H5AD. Rust owns scheduling, caching, VFS artifacts, and port types; the pinned scverse container is the only component that interprets expression matrices.

## Main path

| Node | Inputs | Outputs |
| --- | --- | --- |
| `h5ad_qc_filter` | `h5ad` | `output.h5ad`, `report.json` |
| `h5ad_pca_neighbors_umap_leiden` | `h5ad` | `output.h5ad`, `report.json` |
| `h5ad_celltypist_annotate` | `h5ad`, `model` | `output.h5ad`, `report.json` |
| `sc_dense_ingest` | `count_matrix` (optional when `path` is set) | `output.h5ad`, `report.json` |
| `h5ad_rank_genes_groups` | `h5ad` | `rank_genes_groups.parquet`, `report.json`, `output.h5ad` |
| `h5ad_cluster_mean_expression` | `h5ad` | `cluster_mean_expression.parquet` |
| `gene_set_score` | `h5ad` | `output.h5ad`, `report.json` |

All artifacts are written beneath the node-specific `/artifacts/{kind}/{run_id}/` prefix. Existing H5AD inputs are staged as private container inputs and are not copied to the output prefix. Each H5AD node preserves unique `obs_names` and `var_names`, keeps sparse CSR data sparse, and retains `layers["counts"]` when it slices or normalizes an existing counts layer.

The embedding node creates `X_pca`, `X_umap`, neighbor graphs in `obsp`, parameters in `uns`, and `obs["leiden"]`. If `X_pca` is already present, it is reused rather than recomputed; this allows a corrected embedding from a future integration node to stay the single source of truth.

`sc_dense_ingest` accepts CSV/TSV (optionally gzip/BGZF) with genes in rows or cells in rows, applies optional cell/gene count filters, and retains raw counts in `layers["counts"]`. `h5ad_rank_genes_groups` supports Wilcoxon, t-tests, and logistic regression and emits a tidy `group/gene/rank/score/pvalue/pvalue_adj` table. `h5ad_cluster_mean_expression` emits one `cluster/gene` row per requested gene with optional CP10K normalization and percent expressed. `gene_set_score` adds one numeric obs column per requested module.

## Parquet bypass

| Node | Inputs | Outputs |
| --- | --- | --- |
| `h5ad_obs_to_parquet` | `h5ad` | `cells.parquet` |
| `datafusion_sql` | `parquet` | result `parquet` |
| `h5ad_subset_by_obs` | `h5ad`, `selection_parquet` | `output.h5ad`, `report.json` |

`cells.parquet` starts with `cell_id` derived from `obs_names`, followed by obs columns and explicitly selected `obsm` dimensions named `obsm_{key}_{dimension}`. The SQL node registers the input as table `input` by default and materializes a single-file Parquet result. The reverse bridge keeps H5AD row order and reports matched and missing IDs.

## Workflow shape

```text
sc_dense_ingest(path=dense counts)
  -> h5ad_qc_filter
  -> h5ad_pca_neighbors_umap_leiden
  -> h5ad_rank_genes_groups

file_reference(h5ad)
  -> h5ad_qc_filter
  -> h5ad_pca_neighbors_umap_leiden
  -> h5ad_celltypist_annotate(model=file_reference)

h5ad_qc_filter
  -> h5ad_obs_to_parquet
  -> datafusion_sql
  -> h5ad_subset_by_obs(h5ad=h5ad_qc_filter.output[0])

h5ad_pca_neighbors_umap_leiden
  -> h5ad_cluster_mean_expression
  -> file_to_dataframe
  -> lr_communication_score(lr_table=port_0, cluster_mean_table=port_1)
```

Use `file_reference` with format `h5ad` for an existing VFS or local input. Container network access is disabled; CellTypist models are explicit File inputs. A model must match the input feature space expected by its trained model, and expression should follow that model's preprocessing contract.

## Reference data packages

The reproducible build/publish recipe is `scripts/build_single_cell_reference_packages.sh`.
Current catalog IDs are:

| Dataset | Stable path | Payload |
| --- | --- | --- |
| `lrdb.cellphonedb.v5` | `/bundles/lrdb.cellphonedb.v5` | Official v5 inputs plus normalized `lr_pairs.parquet` |
| `lrdb.ramilowski2015` | `/bundles/lrdb.ramilowski2015` | OmniPath/Ramilowski raw and normalized LR tables |
| `genecards.hgnc_symbols` | `/bundles/genecards.hgnc_symbols` | HGNC complete set and exploded alias dictionary |
| `celltypist.models.pan_immune` | `/bundles/celltypist.models.pan_immune` | `Immune_All_Low.pkl` and `Immune_All_High.pkl` |

For `lr_communication_score`, read `<stable path>/lr_pairs.parquet` with `file_to_dataframe`.
For CellTypist, connect `<stable path>/Immune_All_Low.pkl` to the model input.
