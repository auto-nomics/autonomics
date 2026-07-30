# MiXeR —— 因果混合模型 (`mixer`)

[English](mixer.md) | [中文](mixer_zh.md)

MiXeR 模型（Holland et al. 2020, _PLoS Genetics_）的纯 Rust 重新实现：面向 GWAS 汇总统计的单变量（`fit1`）和双变量（`fit2`）spike-and-slab 因果混合模型。

## 单变量（`fit1`）

通过高斯矩匹配代价函数拟合三个参数（π 多基因性、σ²_β 可发现性、σ²_zero 截距）。优化策略：差分进化 × N 次重复 → Nelder-Mead 精修。与原始 C++ 实现的金标准交叉验证，所有参数偏差 < 0.2%。

## 充分统计量压缩

LD 矩阵在拟合前被折叠为两个每 SNP 标量（`m1`/`m2`），将 O(nnz) 的 CSR 压缩为 O(n_snp)。此后每次代价评估为 O(1)，使优化过程中约 10^4 次代价评估无需触碰 LD。

## 两种加权模式

- **LdScore** —— 单遍，`1/(1+Σr²)`，无需 CSR
- **Randprune** —— 与原始 C++ `std::mt19937_64` 位精确一致

## DAG 节点

作为 `univariate_mixer` 和 `bivariate_mixer` 节点类型暴露（`data-engine/nodes/univariate_mixer.rs`）。两阶段流水线：

1. 离线 `precompute_tags` 生成包含每 tag 充分统计量的 Iceberg 表（`eur_tagsuff`）
2. 运行时节点加载汇总统计，按 rsid 与 tagsuff 连接，并调用 `fit1`

## 数据基础设施

完整的算法推导、流水线图和决策记录见 [`docs/data_infra/univariate_mixer.md`](../data_infra/univariate_mixer.md)。
