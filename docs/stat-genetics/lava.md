# LAVA — Local Genetic Correlation (`lava`)

[English](lava.md) | [中文](lava_zh.md)

A pure-Rust port of LAVA (Werme et al. 2022) for estimating local genetic correlation from GWAS summary statistics and LD reference data.

## LD Reference

When the runtime has VFS storage configured, the locus node reads the 1000G EUR
per-chromosome PLINK files from
`vfs:///data/mixer/resources/g1000_eur/stage` and stages only the requested
chromosomes locally before running the synchronous LAVA loader. Direct-host
prefixes remain available for deployments without VFS, and explicit
`PLINK_REF_PREFIX_TEMPLATE` or `LAVA_PLINK_REF_PREFIX_TEMPLATE` values take
precedence over the mounted panel.

## Cross-validation

All 5 cross-validation tests pass against the R reference implementation.
