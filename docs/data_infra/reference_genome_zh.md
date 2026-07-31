# 人类参考基因组数据基础设施

[English](reference_genome.md) | [中文](reference_genome_zh.md)

> **命名空间**: `reference`
> **数据表**: `grch37_contigs`、`grch37_genes`、`grch38_contigs`、`grch38_genes`
> **构建状态**: 已上线

## 概述

两个人类参考基因组组装 — **GRCh37** (hg19) 和 **GRCh38** (hg38) — 均已作为可复用参考面板载入 Iceberg 数据湖。每个组装提供：

- **Contig 元数据** — 染色体名称、长度和 MD5 校验码。
- **基因注释** — 全结构化、可 SQL 查询的 Ensembl 基因模型表（基因、转录本、外显子、CDS、UTR 等）。

原始 FASTA 和 GTF 文件已归档至 OSS，供需要原始文件格式的工具使用（BWA、minimap2、VEP 等）。

## 数据来源

| 组装 | FASTA | GTF | Ensembl 版本 |
|------|-------|-----|-------------|
| **GRCh37** | `Homo_sapiens.GRCh37.dna.chromosome.*.fa.gz` | `Homo_sapiens.GRCh37.87.gtf.gz` | 归档版 (r87，GRCh37 最后更新) |
| **GRCh38** | `Homo_sapiens.GRCh38.dna.chromosome.*.fa.gz` | `Homo_sapiens.GRCh38.116.gtf.gz` | Release 116 (GRCh38.p14) |

> **为什么两个都维护？** 数据湖中 GWAS 汇总统计和 LD 参考面板使用 GRCh37 坐标，而新数据集越来越多使用 GRCh38。同时维护两者使得湖内注释可以在任一坐标系下直接进行，无需跨组装 liftover。

## 规模

| 组装 | Contigs | 基因特征数 | 基因数 | FASTA 总量 |
|------|---------|-----------|--------|-----------|
| GRCh37 | 25 | 2,613,765 | 57,905 | ~829 MB |
| GRCh38 | 25 | 11,248,794 | 78,941 | ~838 MB |

## Schema

### `reference.{grch37|grch38}_contigs`

| 列名     | 类型   | 说明 |
|----------|--------|------|
| `contig` | string | 染色体名称（1–22, X, Y, MT） |
| `length` | int32  | 序列长度（bp） |
| `md5`    | string | 大写序列的 MD5（与 `samtools faidx` 一致） |

### `reference.{grch37|grch38}_genes`

| 列名                 | 类型   | 说明 |
|----------------------|--------|------|
| `contig`             | string | 染色体 |
| `source`             | string | 注释来源 |
| `feature`            | string | 特征类型（`gene`、`transcript`、`exon`、`CDS`、`UTR`…） |
| `start`              | int32  | 起始位置（1-based, 闭区间） |
| `end_pos`            | int32  | 终止位置（1-based, 闭区间） |
| `strand`             | string | `+` 或 `-` |
| `frame`              | string | 阅读框（`0`/`1`/`2`，或 null） |
| `gene_id`            | string | Ensembl 基因 ID（`ENSG…`） |
| `gene_name`          | string | 基因符号 |
| `gene_biotype`       | string | 基因类型（`protein_coding`、`lncRNA`…） |
| `transcript_id`      | string | Ensembl 转录本 ID（`ENST…`） |
| `transcript_name`    | string | 转录本名称 |
| `transcript_biotype` | string | 转录本类型 |
| `exon_id`            | string | Ensembl 外显子 ID（`ENSE…`） |
| `exon_number`        | int32  | 转录本内外显子编号 |
| `protein_id`         | string | Ensembl 蛋白 ID（`ENSP…`） |

## 构建流程

**工具**: `infra/sink_reference_genome/main.py`

```
Ensembl FTP
    │
    ├── FASTA ──(解析 contig)──► reference.{前缀}_contigs
    │
    ├── GTF ────(解析特征)──────► reference.{前缀}_genes
    │
    └── 原始文件 ──(rclone)─────► aliyun:autonomics-data/reference/{组装}/
```

```bash
cd infra
uv sync

# GRCh37
python -m sink_reference_genome.main --assembly grch37 \
    --staging-dir ../reference/grch37 --mode overwrite

# GRCh38
python -m sink_reference_genome.main --assembly grch38 \
    --staging-dir ../reference/grch38 --mode overwrite
```

## 查询示例

```sql
-- 比较 BRCA1 在两个组装中的坐标
SELECT 'GRCh37' AS asm, start, end_pos
FROM reference.grch37_genes
WHERE gene_name = 'BRCA1' AND feature = 'gene'
UNION ALL
SELECT 'GRCh38', start, end_pos
FROM reference.grch38_genes
WHERE gene_name = 'BRCA1' AND feature = 'gene';
-- 结果:
--   GRCh37  41196312  41277500
--   GRCh38  43044292  43170245

-- 22号染色体蛋白编码基因（GRCh38）
SELECT gene_id, gene_name, start, end_pos
FROM reference.grch38_genes
WHERE contig = '22' AND feature = 'gene' AND gene_biotype = 'protein_coding'
ORDER BY start;

-- 基因组大小对比
SELECT 'GRCh37' AS asm, SUM(length) AS total_bp FROM reference.grch37_contigs
UNION ALL
SELECT 'GRCh38', SUM(length) FROM reference.grch38_contigs;
```

## 下游消费者

| 消费者 | 用途 |
|--------|------|
| GWAS 注释节点 | 按基因组坐标将 SNP 映射到基因（任一组装） |
| MAGMA / 基因层面分析 | 基因集检验的基因边界 |
| MiXeR / LAVA | 位点定义的参考坐标 |
| 跨组装比对 | 比较 GRCh37 与 GRCh38 的基因坐标 |
| 任意 SQL 工具 | 无需加载外部文件的基因/区域查询 |

## OSS 归档

| 组装 | 路径 |
|------|------|
| GRCh37 | `aliyun:autonomics-data/reference/grch37/` |
| GRCh38 | `aliyun:autonomics-data/reference/grch38/` |

```bash
rclone copy aliyun:autonomics-data/reference/grch38/ reference/grch38/ -P
rclone copy aliyun:autonomics-data/reference/grch37/ reference/grch37/ -P
```

## 重建条件

| 变更 | 是否重建？ |
|------|-----------|
| 新增 GWAS 性状 | 否（参考面板与性状无关） |
| Ensembl 新版本（GRCh38） | 是（重新下载 + 覆盖） |
| GRCh37 归档被 Ensembl 更新 | 是（重新下载 + 覆盖） |
