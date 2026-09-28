# MiXeR

MiXeR 对 GWAS 汇总统计拟合单变量（`fit1`）和双变量（`fit2`）
spike-and-slab 因果混合模型。执行路径是官方容器节点
`mixer_fit1_container` 与 `mixer_fit2_container`，它们在隔离的 rootless Podman
容器中
运行 `precimed/gsa-mixer` v2.2.1 CLI（源码提交
`ea2a445912f83e5767d67372b6075912ed5655d8`）。不再保留任何 Rust 原生
MiXeR 节点。

## 节点

`mixer_fit1_container` 接收一个 LDSC 兼容的 GWAS 汇总统计文件（gzip 或纯文本），
以不可变 VFS File 输出官方 `fit1` JSON 和完整日志。
`mixer_fit2_container` 按顺序接收 trait1 sumstats、trait2 sumstats、
trait1 fit1 JSON、trait2 fit1 JSON，输出官方双变量 JSON 与日志。
两者固定绑定 catalog 包 `wjixiang/catalog-mixer-g1000-eur-rsid`，调用方不需要选择镜像或挂载面板。

生产包为 `wjixiang/catalog-mixer-g1000-eur-rsid`，版本 `v2.2.1-rsid1`，摘要
`sha256:46a73e2e4faac1fc216e1d5918257546c87dae727a0f7a55cca39d87bdf4b1ca`。
包含 GRCh37 EUR BIM、未修改的 LD 矩阵和 rsID 寻址的 tag-SNP 模板。ID 由
坐标面板与 `wjixiang/catalog-plink-ref-1000g-eur-binary` 按染色体、位置和等位基因精确映射；
9,588,757 个 BIM 变异中 6,844,696 个、18,269 个 tag SNP 中 7,906 个获得
rsID 地址。官方 chr21-22 迁移 fixture 保留在
`containers/mixer/fixtures/mixer-test-data`；其 fit1 结果与上游基线逐位
一致，fit2 基线在镜像锁定的 Python 3.10/NumPy 1.23.3/SciPy 1.9.1 环境下可复现。

镜像发布为 `ghcr.io/auto-nomics/autonomics/mixer:2.2.1`，不可变 manifest 摘要是
`sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7`。
包装节点使用同一摘要，并按固定的 GHCR 命名空间解析镜像地址。
