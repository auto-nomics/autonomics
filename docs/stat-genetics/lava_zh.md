# LAVA —— 局部遗传相关 (`lava`)

[English](lava.md) | [中文](lava_zh.md)

LAVA（Werme et al. 2022）的纯 Rust 移植，用于从 GWAS 汇总统计和 LD 参考数据估计局部遗传相关。

## LD 参考

locus 节点会依次在 `/data/mixer/resources`、`/mnt/data/mixer/resources` 和
`/mnt/disk3/mixer/reference/mixer_data/stage` 下查找覆盖全部请求染色体的
1000G EUR PLINK 前缀，
旧路径 `/mnt/disk2/dataset/1000g_plink/eur` 仅作为最后回退。部署可用共享的
`PLINK_REF_PREFIX_TEMPLATE` 指定 `{N}` 模板，或用
`LAVA_PLINK_REF_PREFIX_TEMPLATE` 只覆盖 LAVA。

## 交叉验证

全部 5 个交叉验证测试均通过，与 R 参考实现对齐。
