# MiXeR

MiXeR 对 GWAS 汇总统计拟合单变量（`fit1`）和双变量（`fit2`）
spike-and-slab 因果混合模型。`univariate_mixer` 使用 Rust 原生优化器和代价
函数。reference bundle 中的 `libbgmg.so` 仍是加载期依赖，用于解码二进制
`.ld`、对齐汇总统计并计算 randprune 权重；不会启动 Python。

## 节点

`univariate_mixer` 接收一个包含 `rsid`、`A1`、`A2`、`N`、`Z` 列的汇总统计表，
运行 Rust `fit1` 引擎，返回包含 `pi`、`sig2_beta`、`sig2_zero`、`h2`、因果变异
数量、AIC、BIC 和 cost 的单行结果。

`bivariate_mixer` 接收两个 trait 的汇总统计以及各自 fit1 参数，运行
`mixer.py fit2`，返回共享/特异多基因性、遗传相关、Dice、两个 h2 和 cost。

## Reference Bundle

两个节点都使用语义化 `reference` ID，默认为 `g1000_eur`：

```text
/data/mixer/resources/g1000_eur/bundle.json
```

bundle 记录 gsa-MiXeR 源码 revision、GRCh37/EUR 元数据、逐染色体 `.bim`、
LD 和 tag-SNP 模板路径，以及 `libbgmg.so` 的 SHA-256。DAG spec 不暴露裸
engine 和 panel 路径。

`univariate_mixer` 通过 `libloading` 从 Rust 直接加载 `libbgmg.so`。由于
libbgmg 的 calculator context 保存在进程级 manager 中，FFI 调用会被串行化。
节点提取 LD、AF、tag 索引、z/N 和权重后释放 C++ context，并把每个 tag 的 LD
行折叠成 `mixer::fit1` 所需充分统计量。

容器将资源根目录挂载到相同主机路径。`bivariate_mixer` 仍使用带 NumPy、
SciPy、pandas、Boost 和 OpenMP 运行库的管理式 Python；
`univariate_mixer` 不启动 Python。为兼容既有主机，解析还会尝试
`/mnt/data/mixer/resources`。`MIXER_RESOURCE_ROOT` 可覆盖部署查找，并支持冒号
分隔的候选列表；`MIXER_PYTHON` 可覆盖管理式 Python。配置根目录优先，其后仍会
尝试内置根目录。第一个通过校验和检查的有效 bundle 会被采用。
