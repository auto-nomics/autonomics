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

原生 Rust `ldsc` h² 节点 Factory 已删除。库实现仍供 S-LDSC、LCV、liability
转换和 MRlap 内部使用；生产 h² 分析改用 `nodes_io::ldsc_h2_container`。

`ldsc_h2_container` 接收一个 TAB 分隔的 LDSC sumstats File，必须包含
`SNP`、`A1`、`A2`、`N`、`Z` 列。明文 `.tsv` 和 gzip 压缩 `.sumstats.gz`
均可；LDSC 会忽略额外列，但 CSV 不被接受。该节点固定绑定原版 LDSC 镜像及兼容
的 EUR 参考 panel，在隔离的 Podman 容器中运行，并把原始 LDSC log 发布为 VFS
File artifact。原始 GWAS 输入可以先经过 `ldsc_munge_container`。

`ldsc_munge_container` 调用同一官方镜像中的原版 `munge_sumstats.py`。它接收
一个原始 GWAS sumstats File，执行官方列名映射、QC 过滤、N 推断/覆盖和 P 转 Z，
输出可直接连接到 h²/rg 节点的 `.sumstats.gz`，以及原始 munge log。

`ldsc_rg_container` 使用同一官方镜像和 EUR panel 契约执行官方
`ldsc --rg` 分析。它接收两个 TAB 分隔的 LDSC sumstats File，并把原始 rg log
发布为 VFS File artifact。原生 Rust `ldsc_rg` 节点 Factory 已删除。
