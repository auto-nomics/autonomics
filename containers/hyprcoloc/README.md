# HyPrColoc container

Official R `hyprcoloc` package (Foley and Staley, version 0.0.2, commit
`0348bbd`) packaged for the Podman-backed DAG runtime.

Reference: <https://jrs95.github.io/hyprcoloc/>

## Layout

```text
containers/hyprcoloc/
  Dockerfile
  test_hyprcoloc.sh
  fixtures/test-summary-stats.tsv
```

The image contains only the official R package and its pinned runtime
(CRAN snapshot 2025-06-15, with the upstream-recommended RcppEigen
0.3.3.9.3). No LD matrix, trait-correlation matrix, GWAS summary statistics,
or user data are baked into the image.

## End-to-End Workflow

```sh
./containers/hyprcoloc/test_hyprcoloc.sh
```

The script builds `localhost/atc/hyprcoloc:0.0.2` and smoke-tests it with
Podman.

## Thin Wrapper

`crates/node-bundles/nodes-io/src/hyprcoloc_container.rs` accepts one
SNP-aligned tab-separated File. Each configured trait supplies one beta column
and one standard-error column. The wrapper delegates all numerical analysis to
`hyprcoloc::hyprcoloc()` and publishes the official result table, the full R
result object, and an execution log.
