# autonomics bulk-rnaseq image

Layered on top of the published `autonomics/deseq2:1.50.2` image
(`sha256:8b2e2a78d87293e6cae6dbed2e283dff1dd8461a7f993cb347ca9810698b2b3e`).
The base image already ships R 4.5.3 and the pinned CRAN snapshot used by
the autonomics build pipeline; this stage adds:

- `limma` 3.66.0 (Bioconductor 3.22) — voom + eBayes + contrasts
- `edgeR` 4.8.2 (Bioconductor 3.22) — TMM normalization + GLM
- `dynamicTreeCut`, `preprocessCore`, `impute` (Bioconductor 3.22) +
  `fastcluster`, `matrixStats`, `doParallel`, `foreach`, `Rcpp`, `Hmisc`
  from CRAN
- `WGCNA` 1.74 (CRAN, sha256-verified)

The two runner scripts are copied into `/opt/autonomics/`:

- `limma_voom_runner.R` — consumes a `gene_id` + sample count matrix and a
  `sample_id` + covariate metadata table, fits
  `limma::voom + lmFit + eBayes` (with optional `makeContrasts`), applies
  TMM (default) or quantile normalization, and publishes
  `results.tsv`, `voom_weights.tsv`, `normalized_expression.tsv`,
  `contrast_summary.tsv` and a JSON run report.
- `wgcna_runner.R` — consumes a `gene_id` + sample expression matrix plus an
  optional sample-metadata table, runs `WGCNA::blockwiseModules` with signed
  adjacency, TOM, dynamic tree cut, module merge and signed kME, and
  publishes `soft_threshold.tsv`, `adjacency_stats.tsv`, `tom_stats.tsv`,
  `modules.tsv`, `module_eigengenes.tsv` and a JSON run report. TOM is
  kept inside the container — only summary statistics land on VFS, so the
  node never materializes a large TOM dataframe in the agent runtime.

## Build, smoke, publish

```bash
./containers/bulk-rnaseq/test_smoke.sh
```

The script builds the image, verifies the exact R and package versions in an
isolated Podman container, pushes it to the configured `ACR_ENDPOINT`, and
prints the immutable manifest digest. The default build uses `--no-cache`;
set `BUILD_FLAGS` explicitly when reusing local layers during development.

The wrapper nodes pin the published digest:

- `limma_voom_container` (`crates/node-bundles/nodes-io/src/limma_voom_container.rs`)
- `wgcna_container` (`crates/node-bundles/nodes-io/src/wgcna_container.rs`)

Current immutable manifest digest:
`sha256:fc0e90c2883a799db1e2c8934589ab7addd44a6edd1769d50f9d2e668925a66e`.

## Baselines

The real Podman DAG integration tests live in
`crates/node-bundles/nodes-io/tests/bulk_rnaseq_real_podman.rs`. Run them with:

```bash
cargo test -p nodes-io --test bulk_rnaseq_real_podman -- --ignored --test-threads=1
```

Recorded pasilla baselines:

- categorical `condition_treated_vs_untreated`, TMM, covariate `type`:
  14,599 input genes, 8,066 genes after expression filtering, 7 samples;
- continuous `duration`, quantile normalization, no contrast levels:
  14,599 input genes, 7,919 genes after expression filtering, 7 samples;
- both use limma 3.66.0, edgeR 4.8.2, R 4.5.3 and explicit BH correction.

The WGCNA baseline uses a deterministic 60-gene by 12-sample matrix with
optional sample metadata. It returns three modules after signed adjacency,
TOM, dynamic tree cut and merge; module eigengenes retain the metadata
columns. The runner supports Pearson and bicor correlations. The full TOM is
not published to VFS.

## Network-restricted environments

The build requires network access to download `limma`, `edgeR`, the WGCNA
closure, and Bioconductor's `preprocessCore` / `impute`. In offline
environments the same image can be built with the CRAN snapshot pinned by
the `DESEQ2_CRAN_SNAPSHOT` env var and the local Posit snapshot mirror
configured for the deployment.
