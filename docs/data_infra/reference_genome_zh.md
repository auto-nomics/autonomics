# GRCh37 参考基因组数据基础设施

[English](reference_genome.md) | [中文](reference_genome_zh.md)

> **命名空间**: `reference`
> **数据表**: `grch37_contigs`、`grch37_genes`
> **组装版本**: GRCh37.p13 (hg19)
> **基因注释**: Ensembl release 87（GRCh37 最后一次更新）
> **构建状态**: 已上线

## 概述

人类参考基因组 **GRCh37**（又称 hg19）已作为可复用参考面板载入 Iceberg 数据湖，提供：

- **Contig 元数据** — 染色体名称、长度和 MD5 校验码，用于数据完整性验证。
- **基因注释** — 全结构化、可 SQL 查询的 Ensembl 基因模型表（基因、转录本、外显子、CDS、UTR 等）。

原始 FASTA 和 GTF 文件已归档至 OSS，供需要原始文件格式的工具使用（BWA、minimap2、VEP 等）。

## 数据来源

| 文件 | URL | 说明 |
|------|-----|------|
| `Homo_sapiens.GRCh37.dna.primary_assembly.fa.gz` | `ftp.ensembl.org/pub/grch37/current/fasta/homo_sapiens/dna/` | 主组装序列（不含 alt loci） |
| `Homo_sapiens.GRCh37.87.gtf.gz` | `ftp.ensembl.org/pub/grch37/current/gtf/homo_sapiens/` | Ensembl r87 基因注释 |

> **为什么用 GRCh37？** 数据湖中许多 GWAS 汇总统计和 LD 参考面板使用 GRCh37 坐标。维护结构化的 GRCh37 参考可以在湖内直接进行坐标注释，无需跨组装 liftover。

## Schema

### `reference.grch37_contigs`

| 列名     | 类型   | 说明 |
|----------|--------|------|
| `contig` | string | 染色体/scaffold 名称（1–22, X, Y, MT） |
| `length` | int32  | 序列长度（bp） |
| `md5`    | string | 大写序列的 MD5（与 `samtools faidx` 一致） |

**规模**: ~93 条 contig（主组装：1–22 号染色体、X、Y、MT + 未定位 scaffold）。

### `reference.grch37_genes`

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

**规模**: ~260 万条特征（基因、转录本、外显子、CDS、UTR 等）。

## 构建流程

**工具**: `infra/sink_reference_genome/main.py`

```
Ensembl FTP
    │
    ├── FASTA ──(解析 contig)──► reference.grch37_contigs
    │                                  │
    ├── GTF ────(解析特征)──────► reference.grch37_genes
    │
    └── 原始文件 ──(rclone)─────► aliyun:autonomics-data/reference/grch37/
```

```bash
cd infra
uv sync
python -m sink_reference_genome.main --staging-dir ../reference/grch37 --mode overwrite
```

## 查询示例

```sql
-- 基因组总大小
SELECT SUM(length) AS total_bp FROM reference.grch37_contigs;

-- 22号染色体上所有蛋白编码基因
SELECT gene_id, gene_name, start, end_pos
FROM reference.grch37_genes
WHERE contig = '22' AND feature = 'gene' AND gene_biotype = 'protein_coding'
ORDER BY start;

-- 各染色体外显子数量
SELECT contig, COUNT(*) AS n_exons
FROM reference.grch37_genes
WHERE feature = 'exon'
GROUP BY contig ORDER BY contig;
```

## 下游消费者

| 消费者 | 用途 |
|--------|------|
| GWAS 注释节点 | 按基因组坐标将 SNP 映射到基因 |
| MAGMA / 基因层面分析 | 基因集检验的基因边界 |
| MiXeR / LAVA | 位点定义的参考坐标 |
| 任意 SQL 工具 | 无需加载外部文件的基因/区域查询 |

## OSS 归档

原始 FASTA + GTF 文件位于 `aliyun:autonomics-data/reference/grch37/`：

```bash
rclone copy aliyun:autonomics-data/reference/grch37/ reference/grch37/ -P
```

## 重建条件

| 变更 | 是否重建？ |
|------|-----------|
| 新增 GWAS 性状 | 否（参考面板与性状无关） |
| Ensembl 更新 GRCh37 归档 | 是（重新下载 + 覆盖） |
| 切换到 GRCh38 | 新建表（`grch38_*`），保留 GRCh37 并存 |
