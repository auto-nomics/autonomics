# TwoSampleMR (`mr`)

[English](mr.md) | [中文](mr_zh.md)

[TwoSampleMR](https://github.com/MRCIEU/TwoSampleMR) 算法 API（不含 IO/绘图）的纯 Rust 移植，面向 DAG 引擎。

## 功能

- Wald ratio
- IVW 族估计器
- MR-Egger
- 中位数和众数估计器
- `harmonise_data`
- Steiger 过滤
- 异质性/多效性检验

基于 `faer` + `statrs`，点估计与 R 金标准夹具逐位验证。
