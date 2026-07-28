# 数据基础设施索引

> 本目录索引所有持久化在 Iceberg 数据湖中的数据基础设施。
> 每个基础设施对应 `docs/data_infra/` 下一篇文档，记录其数据结构、构建方式、所需原料。

## 依赖关系

```
1000G VCF (raw genotype)
    │
    ├──→ [AF 面板] af.eur_af              （等位基因频率）
    │
    └──→ [LD matrix] ld_matrix.eur_chr{N}  （LD r² 矩阵）
              │
              ├── af.eur_af ──→ [tag 面板] mixer.eur_tag_panel   （tag SNP 列表）
              │                [LD 子图] mixer.eur_subgraph       （tag 诱导 LD 子图）
              │                        │
              │                        └──→ MiXeR univariate/bivariate 节点
              │
              └──→ [LD score] ld_score.*  （LD score 回归用）

GWAS VCF (公开汇总统计)
    │
    └──→ [GWAS sumstats] gwas.*            （Z/N/rsid 汇总统计）
              │
              └──→ MiXeR / LDSC / MR 节点
```

## 已有文档

| 基础设施 | 表 | 文档 | 状态 |
|----------|-----|------|------|
| **LD matrix** | `ld_matrix.eur_chr{1..22}` | [ld_matrix.md](data_infra/ld_matrix.md) | ✅ 已上线 |
| **LD 子图** | `mixer.eur_subgraph` | [subgraph.md](data_infra/subgraph.md) | ✅ 已上线（待修复 tag 一致性） |

## 待补文档

| 基础设施 | 表 | 预期文档 | 优先级 |
|----------|-----|----------|--------|
| **AF 面板** | `af.eur_af` | `data_infra/af.md` | 高（所有节点依赖） |
| **GWAS sumstats** | `gwas.*` | `data_infra/gwas_sumstats.md` | 高（MiXeR/LDSC/MR 输入） |
| **tag 面板** | `mixer.eur_tag_panel` | `data_infra/tag_panel.md` | 中（与子图同源） |
| **LD score** | `ld_score.*` | `data_infra/ld_score.md` | 中（LDSC 专用） |
| **datalake** | Iceberg REST catalog | `data_infra/datalake.md` | 低（基础设施层） |

## 快速参考

### LD matrix

- **表**：`iceberg.ld_matrix.eur_chr{N}`（per-chrom，22 张表）
- **列**：`chrom_a, pos_a, id_a, chrom_b, pos_b, id_b, unphased_r2`
- **构建**：1000G VCF → PLINK2 QC + `--r2` → zstd TSV → `sink_ld_matrix` → Iceberg
- **参数**：MAF≥0.01, GENO≤0.05, 窗口 10Mb, r²≥0.01
- **详见**：[ld_matrix.md](data_infra/ld_matrix.md)

### LD 子图（tag-induced）

- **表**：`iceberg.mixer.eur_subgraph`（全基因组合并）
- **列**：`chrom, id_a, id_b, r2, h_a, h_b`
- **构建**：precompute_tags（逐染色体选 tag + 筛 tag 诱导边）→ merge_subgraph（转 rsid 合并）→ lake_cli upload
- **规模**：267M 行，2.3 GB
- **详见**：[subgraph.md](data_infra/subgraph.md)

### AF 面板（待补）

- **表**：`iceberg.af.eur_af`
- **列**：`id, chrom, alt_freq`
- **用途**：MAF 计算、universe 构建、杂合度 h

### GWAS sumstats（待补）

- **表**：`iceberg.gwas.{trait_id}`（per-trait）
- **列**：`rsid, chrom, effect_size, std_error, sample_size, ...`
- **用途**：Z-score（β/SE）、样本量 N、universe 构建

## 工具索引

| 工具 | 用途 | 位置 |
|------|------|------|
| `precompute_tags` | 逐染色体预算 tag 面板 + 子图 | `src/bin/precompute_tags.rs` |
| `merge_subgraph` | 合并 per-chrom 子图 + 转 rsid | `src/bin/merge_subgraph.rs` |
| `lake_cli` | 数据湖查询 / 管理 / 上传 | `src/bin/lake_cli.rs` |
| `sink_ld_matrix` | LD matrix TSV → Iceberg 入库 | `infra/sink_ld_matrix/` |
| `ld_matrix.sh` | 1000G VCF → PLINK2 LD 计算 | `infra/thousand_genomes/ld_matrix.sh` |
