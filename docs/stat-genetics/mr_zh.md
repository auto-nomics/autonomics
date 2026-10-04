# TwoSampleMR (`twosamplemr_container`)

[English](mr.md) | [中文](mr_zh.md)

容器化 DAG 节点直接运行官方 [TwoSampleMR](https://github.com/MRCIEU/TwoSampleMR) R 包。旧版 Rust 移植 `two_sample_mr` 节点已移除。

## 功能

- Wald ratio
- IVW 族估计器
- MR-Egger
- 中位数和众数估计器
- `harmonise_data`
- Steiger 过滤
- 异质性/多效性检验

官方镜像固定 TwoSampleMR 0.7.9、R 4.5.1 与 PLINK2 2.0.0-a.6.26。工具变量 clumping 使用目录化 1000G EUR 二进制参考，不在运行时调用 OpenGWAS API。包装节点输出官方 MR 表、harmonised 表、RDS 完整结果和执行日志。
