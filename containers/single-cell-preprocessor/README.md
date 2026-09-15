# Single-cell preprocessor container

This image provides a pinned [Scanpy](https://github.com/scverse/scanpy) runtime
for 10x-style MatrixMarket single-cell inputs. It contains only the official
Python implementation and dependencies; expression matrices, barcodes, feature
maps, clinical metadata, and reference panels remain outside the image.

The node has four File inputs:

1. MatrixMarket coordinate counts (`genes x cells`, optionally gzip-compressed).
2. Cell barcode TSV, one ID per line.
3. 10x feature TSV with `gene_id`, `gene_symbol`, and optional feature type.
4. Whitespace-delimited cell metadata whose first column is the cell ID.

The default `inspect` operation reads only the MatrixMarket banner and
dimensions, then validates feature/barcode/metadata alignment and writes a
metadata-only H5AD profile plus a JSON report. This is the safe path for very
large inputs such as an 18 GiB `.mtx`.

The `ingest` operation reads expression values through Scanpy, applies optional
cell/gene filters and total-count normalization, and writes an expression-backed
H5AD. This operation requires enough container memory for the sparse matrix.

## Build and smoke test

```sh
podman build -f containers/single-cell-preprocessor/Dockerfile \
  -t localhost/atc/single-cell-preprocessor:0.1.0 \
  containers/single-cell-preprocessor

containers/single-cell-preprocessor/test_single_cell_preprocessor.sh
```

Set `BUILD_IMAGE=0` to rerun the container contract against an existing image.
