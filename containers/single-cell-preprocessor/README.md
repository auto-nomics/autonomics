# Single-cell preprocessor container

This image provides a pinned official [Scanpy](https://github.com/scverse/scanpy)
1.11.3 runtime for 10x MatrixMarket and H5AD single-cell workflows. It carries only
the Python implementation and its locked wheel dependencies; expression
matrices, barcodes, feature maps, metadata, and reference panels stay outside
the image.

Published immutable image:

```text
crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com/autonomics/single-cell-preprocessor@sha256:7a7397f45775a4c4b6c4c220711db2b95dd37fdc40b7b7f06d181a220903e467
```

The DAG wrapper combines this digest with the repository above through
`$ACR_ENDPOINT/autonomics/single-cell-preprocessor`. The image uses the
digest-pinned Python 3.11.11 slim base, runs as UID/GID 1000, and expects the
Podman backend's read-only root filesystem, `/tmp` tmpfs, and isolated network.

## Contract

The node has four File inputs:

1. MatrixMarket coordinate counts (`genes x cells`, optionally gzip-compressed).
2. Cell barcode TSV, one ID per line.
3. 10x feature TSV with `gene_id`, `gene_symbol`, and an optional third
   feature-type column.
4. Whitespace-delimited cell metadata whose first column is the cell ID.

It always emits:

1. `preprocess_report.json`: schema, matrix header, alignment, QC settings, and
   output dimensions.
2. `preprocessed.h5ad`: AnnData H5AD with cell metadata and the feature map.

The default `inspect` operation reads only the MatrixMarket banner, legal
comment lines, dimensions, and companion tables. It rejects filter or
normalization parameters and writes a metadata-only H5AD with `X` unset. This
is the safe path for very large inputs such as an 18 GiB `.mtx` or `.mtx.gz`.
The report still records the declared nonzero count but does not validate every
coordinate.

The `ingest` operation reads expression values through Scanpy, applies optional
cell/gene filters, stores raw counts in the `counts` layer when normalization
is requested, and writes a full H5AD. Normalization is deterministic Scanpy
total-count scaling to 10,000 followed by `log1p`. Duplicate gene symbols are
made unique in `var_names`; `gene_id` and `feature_type` remain available in
`var`. This operation requires enough container memory for the sparse matrix.

## H5AD workflow contract

The same image also runs `/opt/autonomics/workflow.py` for the DAG H5AD
nodes:

1. `h5ad_qc_filter`: H5AD to filtered H5AD plus JSON report.
2. `h5ad_pca_neighbors_umap_leiden`: optional normalize/log1p/HVG plus PCA,
   neighbors, UMAP, and Leiden.
3. `h5ad_celltypist_annotate`: H5AD plus explicit local model File to annotated
   H5AD and report.
4. `h5ad_obs_to_parquet`: backed H5AD obs projection, including explicit
   `obsm` keys.
5. `h5ad_subset_by_obs`: H5AD plus selection Parquet to filtered H5AD.

The scheduler also registers a native `h5ad_obs_to_dataframe` source outside
this container image. It streams the same `/obs` projection into DataFusion and
is preferred when the DAG can consume a DataFrame directly; the container
Parquet bridge remains available for legacy workflows.

The DAG wrapper supplies the workflow script as an inline private script and a
JSON parameter file, so parameters are schema-validated in Rust before reaching
the container. Expression matrices remain opaque File values.

## Build and baselines

```sh
containers/single-cell-preprocessor/test_single_cell_preprocessor.sh
containers/single-cell-preprocessor/test_single_cell_workflow.sh
```

The script builds the image and validates both paths with real rootless Podman:
header parsing, four-input alignment, metadata-only H5AD output, full ingest,
filtering, the raw-count layer, normalized values, and feature metadata. Set
`BUILD_IMAGE=0` to test an existing tag.

To generate and inspect a deterministic production-scale fixture with 20,000
genes, 50,000 cells, and 20,000,000 declared nonzero values:

```sh
SINGLE_CELL_PRODUCTION_BASELINE=1 \
containers/single-cell-preprocessor/test_single_cell_preprocessor.sh
```

The production baseline keeps the container under a 2 GiB memory limit because
`inspect` does not load expression values. The generated gzip fixture and all
outputs are removed when the test exits.
