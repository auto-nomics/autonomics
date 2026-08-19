# LAVA — Local Genetic Correlation (`lava`)

[English](lava.md) | [中文](lava_zh.md)

A pure-Rust port of LAVA (Werme et al. 2022) for estimating local genetic correlation from GWAS summary statistics and LD reference data.

## LD Reference

The locus node resolves the 1000G EUR PLINK prefix from the first candidate with
all requested chromosomes under `/data/mixer/resources`,
`/mnt/data/mixer/resources`, or `/mnt/disk3/mixer/reference/mixer_data/stage`; the legacy
`/mnt/disk2/dataset/1000g_plink/eur` path remains a final fallback. A deployment
can set `PLINK_REF_PREFIX_TEMPLATE` to a shared `{N}` template, or
`LAVA_PLINK_REF_PREFIX_TEMPLATE` for a LAVA-only template.

## Cross-validation

All 5 cross-validation tests pass against the R reference implementation.
