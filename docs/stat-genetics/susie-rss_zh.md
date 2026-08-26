# SuSiE-RSS

`susie_rss_container` 使用官方 `susieR` R 包对 GWAS 汇总统计执行贝叶斯
精细定位。注册节点在临时 k3s Job 中运行 `susieR` 0.16.6，并挂载目录化的
`mixer.g1000_eur` signed-LD 面板。

## 运行时与参考面板

`mixer.g1000_eur` 包固定了 GRCh37 EUR 面板、逐染色体 PLINK BIM、signed
Pearson-r LD 矩阵以及 `libbgmg.so` checksum。容器通过 gsa-MiXeR 引擎查询
signed LD pair，并把相关矩阵直接传给 `susieR::susie_rss()`；不会用 GWAS
z-score 符号重建 LD 方向。

对齐使用完整逐染色体 BIM。包内另有每染色体约 254 个 SNP 的
`snps/g1000_eur_chrN.snps` 提取列表；它们是 mixer 遗留产物，不是本节点
的输入约束。

## 输入契约

输入是一个 tab 分隔 File，列定义如下：

| 列 | 必填 | 含义 |
| --- | --- | --- |
| `snp` | 是 | 可与 mixer BIM 匹配的变异键 |
| `chrom` | 是 | 染色体；所有行必须属于同一染色体 |
| `z` | 是 | GWAS z-score；缺失值会被拒绝 |
| `n` | 否 | locus 样本量；`spec.n` 优先 |
| `a1` | 否 | LD 对齐用第一个等位基因 |
| `a2` | 否 | LD 对齐用第二个等位基因 |

推荐直接使用 `sink_file`：

```json
{
  "path": "/locus.tsv",
  "format": "tsv",
  "mode": "overwrite"
}
```

容器端按 `read.delim` 语义读取，不会嗅探 CSV。逗号分隔文件的第一行会被
解析成单一列名，随后报 `input is missing required columns: snp, chrom, z`；
这个报错没有指出真正的分隔符问题。

上游应保证 `snp` 唯一，例如按 `snp` 分区后用 `ROW_NUMBER()` 去重。
SuSiE-RSS 是单 locus 模型；大区域应拆分为每次最多数千个变异。设置
`r2_min > 0` 可以减少用于构造稠密矩阵的 LD pair。

## SNP 键格式

默认面板使用 GRCh37 位置型 BIM ID：

```text
chr:pos:allele1:allele2
21:36119111:G:T
```

`snp` 列必须使用这些字面 ID。rsID 无法对齐，会得到类似
`N input SNP(s) did not align to the signed-LD reference` 的错误。

省略 `a1` 和 `a2` 时，它们从 ID 最后两个冒号分段推断。显式等位基因列
允许正反两种方向；signed-LD 查询会为翻转的 allele pair 调整相关系数符号。

## 输出

每次运行会把不可变产物发布到
`/artifacts/susie_rss_container/<job-id>/`，并带内容指纹。端口当前按位置
区分，尚无稳定名称：

| 端口 | 产物 | 内容 |
| --- | --- | --- |
| 0 | `susie_rss.tsv` | `snp`、`pip`、`cs`、`alpha`、`mu`、`mu2`、`lbf` |
| 1 | `susie_rss.RDS` | 可用 `readRDS()` 复分析的官方 fit 对象 |
| 2 | `susie_rss.log` | 变异数、迭代数、收敛状态与 credible set |

TSV 中 `cs=0` 表示该变异不在任何报告的 credible set；`cs=k`（`k>=1`）
表示属于第 k 个 credible set，对应单效应 `L_k`。`alpha` 是该变异在最优
单效应下的包含概率。后验矩应满足 `mu * mu <= mu2`。

## 失败行为与参数

列缺失、z 缺失、多染色体、未知 SNP 键，以及多变异 locus 没有任何可重叠
LD pair，都会使运行失败。重复 SNP 键即使底层引擎碰巧接受，也不是有效输入；
应在 upstream 去重。默认容器超时为 1800 秒。

| 参数 | 默认值 | 含义 |
| --- | ---: | --- |
| `l` | `10` | 非零效应数上限 |
| `estimate_prior_method` | `"optim"` | `optim`、`EM` 或 `simple` |
| `estimate_residual_variance` | `false` | 每个 IBSS 迭代估计残差方差 |
| `estimate_prior_variance` | `true` | 逐效应估计先验方差 |
| `coverage` | `0.95` | credible set 目标覆盖率 |
| `min_abs_corr` | `0.5` | credible set 纯度的最小绝对相关 |
| `scaled_prior_variance` | `0.2` | 初始 scaled prior variance |
| `z_method` | `"wald"` | `wald` 或 `score` |
| `r2_min` | `0.0` | signed LD pair 的最小 `r*r` |
| `n` | `null` | 样本量覆盖；否则读输入列 `n` |
| `check_null_threshold` | `0.0` | `susie_rss` check-null 阈值 |
| `max_iter` | `100` | 最大 IBSS 迭代数 |
| `artifact_prefix` | `/artifacts/susie_rss_container` | 不可变输出前缀 |
| `timeout_secs` | `1800` | 容器超时 |

## 数值回归基线

在相同 223-SNP 合成 locus 和相同参数下，旧原生 Rust 实现与官方容器版
一致到 8-9 位有效数字：

| SNP | 指标 | 原生 Rust | 容器版 |
| --- | --- | ---: | ---: |
| `21:46783784:G:T` | `pip` / `lbf` | `1.0` / `66.225242` | `1` / `66.225242` |
| `21:46777491:C:T` | `pip` / `mu` | `0.99999899` / `-0.0133920760` | `0.99999899` / `-0.0133920761` |
| `21:45513808:C:T` | `pip` | `0.16537501` | `0.16537501` |

真实 AF GWAS chr21:36.12Mb 的 `rs2834618` 可作为负向对齐用例。转换为
`21:36119111:G:T` 且 `z=-8.43` 后，运行得到接近 1.0 的 PIP、一个 95%
credible set、`lbf=34.2`，并在 14 次迭代后收敛。

## rsID 转换 SQL

rsID 汇总统计应按染色体、GRCh37 坐标和等位基因对 join。下面的模式同时处理
`source_file` 把无表头 BIM 首行当作列名的问题：

```sql
WITH bim AS (
  SELECT "21:9411410:C:T" AS vid,
         CAST("21" AS BIGINT) AS rchrom,
         CAST("9411410" AS BIGINT) AS rbp,
         "T" AS ral1,
         "C" AS ral2
  FROM port_1
), joined AS (
  SELECT bim.vid AS snp,
         af."CHR" AS chrom,
         af."Z" AS z,
         CAST(af."N" AS DOUBLE) AS n,
         af."A1" AS a1,
         af."A2" AS a2,
         ROW_NUMBER() OVER (
           PARTITION BY bim.vid
           ORDER BY ABS(af."Z") DESC
         ) AS rn
  FROM port_0 af
  JOIN bim
    ON af."CHR" = bim.rchrom
   AND af."BP" = bim.rbp
   AND ((af."A1" = bim.ral1 AND af."A2" = bim.ral2)
     OR (af."A1" = bim.ral2 AND af."A2" = bim.ral1))
  WHERE af."BP" BETWEEN 36070000 AND 36170000
)
SELECT snp, chrom, z, n, a1, a2
FROM joined
WHERE rn = 1;
```

如果目标 locus 包含 BIM 第一行，需要显式补回该行；`bim` 中带引号的列名
正是被当作表头消耗掉的首行值。
