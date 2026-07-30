# LD Score Regression (`ldsc`)

[English](ldsc.md) | [中文](ldsc_zh.md)

[LD Score Regression](https://github.com/bulik/ldsc)（Bulik-Sullivan & Finucane）的忠实纯 Rust 库移植。

## 功能

- SNP 遗传力（h²）
- 遗传相关（rg）
- 细胞类型特异性分析（cts）
- 从 PLINK 基因型计算 LD score
- 汇总统计清洗（munging）
- 注释构建

数值计算基于 [`faer`](https://github.com/sarah-ek/faer)（不依赖 LAPACK/MKL）。点估计与 Python 参考实现的测试套件和金标准夹具交叉验证。

## DAG 集成

`estimate_h2` DataFrame 入口已接入数据引擎（`data-engine/nodes/ldsc_hsq.rs`）。
