# SuSiE-RSS

`susie_rss` 对包含 `snp`、`chrom`、`z`，以及可选 `n`、`a1`、`a2` 列的 GWAS
汇总统计执行贝叶斯精细定位。拟合引擎为纯 Rust SuSiE-RSS 移植，并与 susieR
金标准输出交叉验证。

## Signed LD Reference

节点使用语义化 `reference` ID，默认为 `g1000_eur`，并复用已部署的 gsa-MiXeR
bundle：

```text
/mnt/data/mixer/resources/g1000_eur/bundle.json
```

bundle 记录 GRCh37 EUR panel、匹配的逐染色体 BIM、signed Pearson-r LD 矩阵，
以及 `libbgmg.so` checksum。执行时节点通过 gsa-MiXeR 引擎查询输入 locus 的
signed LD pair，并直接把相关矩阵交给 SuSiE，绝不使用 GWAS z-score 符号推断
LD 方向。

输入 SNP ID 必须与 reference BIM ID 匹配。当前 1000 Genomes panel 使用
`21:9411410:C:T` 这类 variant ID。自定义 signed-LD bundle 可通过语义化
`reference` ID 选择；DAG spec 不暴露裸 LD 路径。
省略 `a1`/`a2` 时，会从 `chr:pos:a1:a2` ID 推断等位基因；非坐标形式 SNP ID
必须提供显式 allele 列。重复 ID 和只部分匹配 reference 的 locus 会被拒绝，
不会静默当作对角矩阵处理。

`r2_min` 按 `r * r` 过滤稀疏 signed LD pair。对多个变异的 locus，如果没有任何
LD pair 与输入重合，节点会报错，而不是静默使用对角矩阵。
