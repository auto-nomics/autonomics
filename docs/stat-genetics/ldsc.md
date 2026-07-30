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

The `estimate_h2` DataFrame entry point is wired into the data-engine (`data-engine/nodes/ldsc_hsq.rs`).
