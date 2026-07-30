# TwoSampleMR (`mr`)

[English](mr.md) | [中文](mr_zh.md)

A pure-Rust port of [TwoSampleMR](https://github.com/MRCIEU/TwoSampleMR)'s algorithm API (no IO/plotting) for the DAG engine.

## Features

- Wald ratio
- IVW family estimators
- MR-Egger
- Median and mode estimators
- `harmonise_data`
- Steiger filtering
- Heterogeneity/pleiotropy tests

Built on `faer` + `statrs`, with point estimates validated bit-for-bit against R golden fixtures.
