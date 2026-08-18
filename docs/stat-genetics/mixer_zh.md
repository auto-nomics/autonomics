# MiXeR

MiXeR 对 GWAS 汇总统计拟合单变量（`fit1`）和双变量（`fit2`）
spike-and-slab 因果混合模型。Autonomics 直接使用原始 gsa-MiXeR v2.2.1
Python/C++ 引擎，以保证数值保真，而不是在 Rust 中重新实现优化器和代价函数。

## 节点

`univariate_mixer` 接收一个包含 `rsid`、`A1`、`A2`、`N`、`Z` 列的汇总统计表，
运行 `mixer.py fit1`，返回包含 `pi`、`sig2_beta`、`sig2_zero`、`h2`、因果变异
数量、AIC、BIC 和 cost 的单行结果。

`bivariate_mixer` 接收两个 trait 的汇总统计以及各自 fit1 参数，运行
`mixer.py fit2`，返回共享/特异多基因性、遗传相关、Dice、两个 h2 和 cost。

## Reference Bundle

两个节点都使用语义化 `reference` ID，默认为 `g1000_eur`：

```text
/mnt/data/mixer/resources/g1000_eur/bundle.json
```

bundle 记录 gsa-MiXeR 源码 revision、GRCh37/EUR 元数据、逐染色体 `.bim`、
LD 和 tag-SNP 模板路径，以及 `libbgmg.so` 的 SHA-256。DAG spec 不暴露裸
engine 和 panel 路径。

容器将资源根目录挂载到相同主机路径，并使用带 NumPy、SciPy、pandas、Boost
和 OpenMP 运行库的管理式 Python。部署可通过 `MIXER_RESOURCE_ROOT` 和
`MIXER_PYTHON` 覆盖默认值。
