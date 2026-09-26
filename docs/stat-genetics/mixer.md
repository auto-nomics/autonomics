# MiXeR

MiXeR fits univariate (`fit1`) and bivariate (`fit2`) spike-and-slab causal
mixture models to GWAS summary statistics. The execution path is the official
container nodes `mixer_fit1_container` and `mixer_fit2_container`, which run the
`precimed/gsa-mixer` v2.2.1 CLI (source commit
`ea2a445912f83e5767d67372b6075912ed5655d8`) in an isolated rootless-Podman
container. There are
no native Rust MiXeR nodes.

## Nodes

`mixer_fit1_container` takes one LDSC-compatible GWAS summary-statistics File
(gzip or plain text) and emits the official `fit1` JSON plus complete log as
immutable VFS Files. `mixer_fit2_container` takes trait1 sumstats, trait2
sumstats, trait1 fit1 JSON, and trait2 fit1 JSON, in that order, and emits the
official bivariate JSON plus log. Both bind the catalog package
`wjixiang/catalog-mixer-g1000-eur-rsid`; callers do not select images or mount panels.

The production package is `wjixiang/catalog-mixer-g1000-eur-rsid`, version
`v2.2.1-rsid1`, digest
`sha256:46a73e2e4faac1fc216e1d5918257546c87dae727a0f7a55cca39d87bdf4b1ca`.
It contains the GRCh37 EUR BIM, unchanged LD matrices, and rsID-addressed
tag-SNP templates. IDs were translated from the coordinate-addressed source
panel with an exact allele-aware match against
`wjixiang/catalog-plink-ref-1000g-eur-binary`; 6,844,696 of 9,588,757 BIM variants and 7,906
of 18,269 tag SNPs have rsID addresses. The official chr21-22 migration
fixture is retained in
`containers/mixer/fixtures/mixer-test-data`; its fit1 result is bit-identical
to the upstream baseline, and its fit2 baseline is reproducible under the
image's pinned Python 3.10/NumPy 1.23.3/SciPy 1.9.1 environment.

The image is published as `ghcr.io/auto-nomics/autonomics/mixer:2.2.1` with immutable
manifest digest
`sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7`.
The wrappers combine the same digest with the fixed GHCR namespace.
