# LD Score Regression (`ldsc`)

[English](ldsc.md) | [中文](ldsc_zh.md)

A faithful, library-only pure-Rust port of [LD Score Regression](https://github.com/bulik/ldsc) (Bulik-Sullivan & Finucane).

## Features

- SNP-heritability (h²)
- Genetic correlation (rg)
- Cell-type-specific analysis (cts)
- LD-score computation from PLINK genotypes
- Summary-statistic munging
- Annotation building

Numerics run on [`faer`](https://github.com/sarah-ek/faer) (no LAPACK/MKL). Point estimates are cross-checked against the Python reference's own test suite and golden fixtures.

## DAG integration

The original Rust `ldsc` h² node factory is removed. The library implementation
remains available for S-LDSC, LCV, liability conversion, and MRlap internals,
but production h² analyses use `nodes_io::ldsc_h2_container`.

`ldsc_h2_container` accepts one tab-separated LDSC sumstats File with
`SNP`, `A1`, `A2`, `N`, and `Z` columns. Plain `.tsv` and gzip-compressed
`.sumstats.gz` are both accepted; LDSC ignores extra columns, but CSV is not
accepted. The node binds the original LDSC image to its compatible EUR
reference panels, runs an isolated Podman container, and publishes the raw LDSC log as a
VFS File artifact. Raw GWAS inputs can first pass through
`ldsc_munge_container`.

`ldsc_munge_container` invokes the original `munge_sumstats.py` CLI from the
same official image. It accepts one raw GWAS sumstats File, applies official
column mapping, QC filters, N inference/overrides, and P-to-Z conversion, then
emits a `.sumstats.gz` File directly compatible with the h²/rg nodes plus the
raw munging log.

`ldsc_rg_container` performs the corresponding official `ldsc --rg` analysis.
It accepts two tab-separated LDSC sumstats Files, uses the same image and EUR
panel contract, and emits the raw LDSC rg log as a VFS File artifact. The Rust
`ldsc_rg` node factory is removed.
