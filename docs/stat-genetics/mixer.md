# MiXeR — Causal Mixture Model (`mixer`)

[English](mixer.md) | [中文](mixer_zh.md)

A pure-Rust reimplementation of the MiXeR model (Holland et al. 2020, _PLoS Genetics_): univariate (`fit1`) and bivariate (`fit2`) spike-and-slab causal mixture for GWAS summary statistics.

## Univariate (`fit1`)

Fits three parameters (π polygenicity, σ²_β discoverability, σ²_zero intercept) via Gaussian moment-matching cost function. Optimization: differential evolution × N repeats → Nelder-Mead refinement. Cross-validated against the original C++ implementation's gold standard to < 0.2% across all parameters.

## Sufficient statistics compression

The LD matrix is folded into two per-SNP scalars (`m1`/`m2`) pre-fit, collapsing O(nnz) CSR to O(n_snp). Cost evaluation is O(1) per tag thereafter, enabling ~10^4 cost evaluations during optimization without touching LD.

## Two weighting modes

- **LdScore** — single-pass, `1/(1+Σr²)`, no CSR needed
- **Randprune** — bit-exact with original C++ `std::mt19937_64`

## DAG node

Exposed as `univariate_mixer` and `bivariate_mixer` node kinds (`data-engine/nodes/univariate_mixer.rs`). Two-phase pipeline:

1. Offline `precompute_tags` produces a VFS table (`eur_tagsuff`) containing per-tag sufficient statistics
2. The runtime node loads sumstats, joins with tagsuff by rsid, and invokes `fit1`
