# Single-cell preprocessor container

This image provides a pinned official [Scanpy](https://github.com/scverse/scanpy)
1.11.3 runtime for 10x-style MatrixMarket single-cell inputs. It carries only
the Python implementation and its locked wheel dependencies; expression
matrices, barcodes, feature maps, metadata, and reference panels stay outside
the image.

Published immutable image:

```text
crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com/autonomics/single-cell-preprocessor@sha256:53fd628049d4b115b8fb805edfaf45905d4f5a60f87e4fcd2cf6acf8b378e940
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

## Build and baselines

```sh
containers/single-cell-preprocessor/test_single_cell_preprocessor.sh
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
