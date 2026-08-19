# LAVA —— 局部遗传相关 (`lava`)

[English](lava.md) | [中文](lava_zh.md)

LAVA（Werme et al. 2022）的纯 Rust 移植，用于从 GWAS 汇总统计和 LD 参考数据估计局部遗传相关。

## LD 参考

运行时配置了 VFS 存储时，locus 节点会从
`vfs:///data/mixer/resources/g1000_eur/stage` 读取按染色体拆分的 1000G EUR
PLINK 文件，并只为请求的染色体做本地 staging，然后交给同步 LAVA 加载器。
未配置 VFS 的部署仍可使用本机前缀；显式设置的
`PLINK_REF_PREFIX_TEMPLATE` 或 `LAVA_PLINK_REF_PREFIX_TEMPLATE` 优先于挂载面板。

## 交叉验证

全部 5 个交叉验证测试均通过，与 R 参考实现对齐。
