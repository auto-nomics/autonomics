---
title: LAVA 计算所需输入数据
aliases: [LAVA input data, LAVA 输入数据]
tags:
  - lava
  - genetic-correlation
  - bioinformatics
  - gwas
source: bio_crates/lava/src/input.rs, plink.rs
---

# LAVA 计算所需输入数据

> [!info] 概览
> 一次 LAVA（局部遗传相关性）分析需要 **5 类输入**：GWAS 汇总统计、表型清单、PLINK 参考面板、locus 定义、样本重叠矩阵。前 3 类必需，后 2 类视情况。

入口函数：`input::process_input(...)` → 返回 `Input` 对象。

---

## 1. GWAS 汇总统计文件

> 每个表型一份文件，空格 / Tab 分隔，由 `read_sumstats_file` 读取。

### 必需列

| 字段 | 接受的列名（取首个匹配） |
|------|--------------------------|
| SNP id | `SNP` · `ID` · `SNPID_UKB` · `SNPID` · `MarkerName` · `RSID` · `RSID_UKB` |
| 效应等位 A1 | `A1` · `ALT` |
| 另一等位 A2 | `A2` · `REF` |
| 样本量 N | `N` · `NMISS` · `N_analyzed` |

### 统计量列（二选一）

| 方式 | 列名 | 说明 |
|------|------|------|
| 直接给 Z | `Z` · `T` · `STAT` · `Zscore` | 直接使用，按 `min_pval` 截断 |
| 反推 Z | `B`/`BETA` · `OR` · `logOdds` **+** `P` | `Z = -qnorm(P/2) × sign` |

> [!note] Z 截断
> 所有 Z 值被 `±qnorm(min_pval/2)` 夹断（`format.z`），避免极端 p 导致数值溢出。

> [!tip] N 全局覆盖
> 若所有 SNP 样本量相同，可用 `n_override` 参数传入固定 N，此时表头无需 `N` 列。

### 可选列

- `GENE` —— 基因注释，原样保留。
- `OR` —— odds ratio，反推 Z 时符号规则：`>1 → +`，`<1 → −`。

### 行过滤

含 `NA`（`stat`/`n` 为 NaN）、`n ≤ 0`、SNP id 为空的行被丢弃。

---

## 2. input-info 文件（表型清单）

> 空格 / Tab 分隔，由 `read_input_info` 读取。

### 必需列

```
phenotype   cases   controls   filename
```

### 可选列

- `prevalence` —— 二分类表型的群体患病率（用于 liability 转换）。

### 派生量

- `n = cases + controls`
- `prop_cases = cases / n`
- `binary = (prop_cases 非 NaN) && (prop_cases ≠ 1)`

> [!example] 示例
> ```
> phenotype     cases    controls   filename
> scz           0        0          scz.sumstats.gz
> height        0        0          height.sumstats.gz
> ```

`phenos` 参数可选子集/重排；`input_dir` 可给所有 `filename` 拼公共前缀。

---

## 3. PLINK 参考面板

> 一套 `.bed / .bim / .fam`，由 `plink::load_reference(ref_prefix)` 加载。

| 文件 | 提供内容 |
|------|----------|
| `.bim` | SNP / chr / pos / **a1 / a2**（等位基因对齐基准） |
| `.fam` | 样本数 `n_indiv` |
| `.bed` | 基因型（按每个 locus 由 `load_plink` 现取现算 LD） |

> [!important] 参考即基准
> `.bim` 的等位方向是对齐所有表型统计量的**标准**。基因型在 locus 层面按需读取，算出该区域 LD 相关与频率矩阵后，送入 `decompose_ld` 做 SVD / 特征值降维。

---

## 4. locus 定义文件（可选 —— 按区域分析）

> 空格 / Tab 分隔，由 `read_loci` 读取。两种格式任选其一：

| 格式 | 必需列 |
|------|--------|
| 坐标式 | `LOC` · `CHR` · `START` · `STOP` |
| 显式 SNP | `LOC` · `SNPS`（SNP 用 `;` 分隔，自动转小写） |

> [!note] 必含 `LOC`
> 至少要有 `LOC`，且满足坐标式或显式 SNP 之一，否则报错。

---

## 5. 样本重叠矩阵（可选 —— 多表型样本重叠时）

> 空格 / Tab 方阵，由 `process_sample_overlap` 读取并 `cov2cor`。

- 行名、列名均为**表型名**，元素为重叠样本数。
- 内部转为相关矩阵，进入非中心 Wishart 推断。

> [!warning] 单表型自动跳过
> 表型数 `p == 1` 时即使不传也不会报错（`process_input` 内部判空）。

---

## 入口函数

```rust
pub fn process_input(
    input_info_file: &Path,        // 上面 #2
    sample_overlap_file: Option<&Path>,  // #5
    ref_prefix: &Path,             // #3
    phenos: Option<&[String]>,     // None=全部 / Some=子集
    input_dir: Option<&str>,       // 给 filename 拼前缀
) -> Result<Input>
```

返回的 `Input` 含：

- `sum_stats` —— 各表型对齐后的统计量
- `analysis_snps` —— 跨数据集共同 SNP（参考顺序）
- `unalignable_snps` —— 对齐失败剔除的 SNP
- `bim_index` —— SNP id → `.bim` 行号（供按 locus 取 LD）
- `sample_overlap` / `reference`

---

## 数据流

```
input.info ─┐
sum.stats ──┼─► process_input ─► Input
PLINK ref ──┤        │ (harmonize + align)
overlap ────┘        ▼
                per-locus:
                load_plink ─► LD ─► decompose_ld ─► locus / analysis
                                                          (locus & analysis 当前为 stub)
```

> [!warning] 实现进度
> 输入处理 + LD 分解 + 对齐管线已完成；`locus` / `analysis` / `wishart` / `ci` / `pcor` 仍为 stub，统计推断尚未实现。

---

## 相关

- [[LAVA 移植总览]]
- LAVA 原版：<https://github.com/josefinwerme/LAVA>
