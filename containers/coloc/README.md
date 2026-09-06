# coloc container

Official R `coloc` package (Wallace, CRAN 5.2.3) packaged for the Podman-backed
DAG runtime.

Reference: <https://chr1swallace.github.io/coloc/>

## Layout

```
containers/coloc/
  Dockerfile            # rocker/r-ver:4.5.1 + coloc 5.2.3 from CRAN Archive
  test_coloc_abf.sh     # build -> smoke test
```

The image contains only the official R package and its runtime; no reference
panels, no LD matrices, no GWAS inputs are baked in.

## End-to-end workflow

```sh
./containers/coloc/test_coloc_abf.sh
```

The script builds `localhost/atc/coloc:5.2.3` and smoke-tests it with
Podman.

## Thin wrapper

`crates/node-bundles/nodes-io/src/coloc_abf_container.rs` owns the analysis
contract: input TSV of two GWAS summary-statistic datasets, output RDS + log
artifacts. The wrapper delegates execution to `ContainerCommandNode`; it
does not reimplement any coloc numerical logic.
