# TwoSampleMR (`twosamplemr_container`)

[English](mr.md) | [中文](mr_zh.md)

The container-backed DAG node runs the official
[TwoSampleMR](https://github.com/MRCIEU/TwoSampleMR) R package. The legacy
Rust-port `two_sample_mr` node has been removed.

## Features

- Wald ratio
- IVW family estimators
- MR-Egger
- Median and mode estimators
- `harmonise_data`
- Steiger filtering
- Heterogeneity/pleiotropy tests

The official image pins TwoSampleMR 0.7.9, R 4.5.1, and PLINK2
2.0.0-a.6.26. Instrument clumping uses the catalog-backed 1000G EUR binary
reference rather than a runtime OpenGWAS API call. The wrapper emits the
official MR table, harmonised table, RDS result, and execution log.
