# Human Reference Genome Data Infrastructure

[English](reference_genome.md) | [中文](reference_genome_zh.md)

> **Namespace**: `reference`
> **Tables**: `grch37_contigs`, `grch37_genes`, `grch38_contigs`, `grch38_genes`
> **Build status**: Live

## Overview

Both human reference genome assemblies — **GRCh37** (hg19) and **GRCh38** (hg38)
— are loaded as reusable reference panels in the Iceberg data lake. Each
assembly provides:

- **Contig metadata** — chromosome names, lengths, and MD5 checksums.
- **Gene annotation** — a fully structured, SQL-queryable table of all Ensembl
  gene models (genes, transcripts, exons, CDS, UTRs).

The raw FASTA and GTF files are archived to OSS for tools that require the
original file formats (BWA, minimap2, VEP, etc.).

## Source

| Assembly | FASTA | GTF | Ensembl Release |
|----------|-------|-----|-----------------|
| **GRCh37** | `Homo_sapiens.GRCh37.dna.chromosome.*.fa.gz` | `Homo_sapiens.GRCh37.87.gtf.gz` | Archive (r87, last GRCh37 update) |
| **GRCh38** | `Homo_sapiens.GRCh38.dna.chromosome.*.fa.gz` | `Homo_sapiens.GRCh38.116.gtf.gz` | Release 116 (GRCh38.p14) |

> **Why both?** GWAS summary statistics and LD reference panels in the data lake
> use GRCh37 coordinates. Newer datasets increasingly use GRCh38. Maintaining
> both enables in-lake annotation for either coordinate system without
> cross-assembly liftover.

## Scale

| Assembly | Contigs | Gene features | Genes | Total FASTA |
|----------|---------|---------------|-------|-------------|
| GRCh37   | 25      | 2,613,765     | 57,905  | ~829 MB |
| GRCh38   | 25      | 11,248,794    | 78,941  | ~838 MB |

## Schema

### `reference.{grch37|grch38}_contigs`

| Column   | Type   | Description |
|----------|--------|-------------|
| `contig` | string | Chromosome / scaffold name (1–22, X, Y, MT) |
| `length` | int32  | Sequence length in base pairs |
| `md5`    | string | MD5 of the uppercase sequence (matches `samtools faidx`) |

### `reference.{grch37|grch38}_genes`

| Column               | Type   | Description |
|----------------------|--------|-------------|
| `contig`             | string | Chromosome |
| `source`             | string | Annotation source |
| `feature`            | string | Feature type (`gene`, `transcript`, `exon`, `CDS`, `UTR`, …) |
| `start`              | int32  | Start position (1-based, inclusive) |
| `end_pos`            | int32  | End position (1-based, inclusive) |
| `strand`             | string | `+` or `-` |
| `frame`              | string | Reading frame (`0`/`1`/`2`, or null) |
| `gene_id`            | string | Ensembl gene ID (`ENSG…`) |
| `gene_name`          | string | Gene symbol |
| `gene_biotype`       | string | Gene biotype (`protein_coding`, `lncRNA`, …) |
| `transcript_id`      | string | Ensembl transcript ID (`ENST…`) |
| `transcript_name`    | string | Transcript name |
| `transcript_biotype` | string | Transcript biotype |
| `exon_id`            | string | Ensembl exon ID (`ENSE…`) |
| `exon_number`        | int32  | Exon number within transcript |
| `protein_id`         | string | Ensembl protein ID (`ENSP…`) |

## Construction

**Tool**: `infra/sink_reference_genome/main.py`

```
Ensembl FTP
    │
    ├── FASTA ──(parse contigs)──► reference.{prefix}_contigs
    │
    ├── GTF ────(parse features)──► reference.{prefix}_genes
    │
    └── raw files ──(rclone)──► aliyun:autonomics-data/reference/{assembly}/
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

## Query Examples

```sql
-- Compare BRCA1 coordinates across assemblies
SELECT 'GRCh37' AS asm, start, end_pos
FROM reference.grch37_genes
WHERE gene_name = 'BRCA1' AND feature = 'gene'
UNION ALL
SELECT 'GRCh38', start, end_pos
FROM reference.grch38_genes
WHERE gene_name = 'BRCA1' AND feature = 'gene';
-- Result:
--   GRCh37  41196312  41277500
--   GRCh38  43044292  43170245

-- Protein-coding genes on chromosome 22 (GRCh38)
SELECT gene_id, gene_name, start, end_pos
FROM reference.grch38_genes
WHERE contig = '22' AND feature = 'gene' AND gene_biotype = 'protein_coding'
ORDER BY start;

-- Genome size comparison
SELECT 'GRCh37' AS asm, SUM(length) AS total_bp FROM reference.grch37_contigs
UNION ALL
SELECT 'GRCh38', SUM(length) FROM reference.grch38_contigs;
```

## Downstream Consumers

| Consumer | Purpose |
|----------|---------|
| GWAS annotation nodes | Map SNPs to genes by genomic coordinates (either assembly) |
| MAGMA / gene-based analysis | Gene boundaries for gene-set tests |
| MiXeR / LAVA | Reference coordinates for locus definition |
| Cross-assembly alignment | Compare gene coordinates between GRCh37 and GRCh38 |
| Any SQL tool | Gene/region queries without loading external files |

## OSS Archive

| Assembly | Path |
|----------|------|
| GRCh37 | `aliyun:autonomics-data/reference/grch37/` |
| GRCh38 | `aliyun:autonomics-data/reference/grch38/` |

```bash
rclone copy aliyun:autonomics-data/reference/grch38/ reference/grch38/ -P
rclone copy aliyun:autonomics-data/reference/grch37/ reference/grch37/ -P
```

## Rebuild Conditions

| Change | Rebuild? |
|--------|----------|
| Add a new GWAS trait | No (reference is trait-independent) |
| New Ensembl release (GRCh38) | Yes (re-download + overwrite) |
| GRCh37 archive updated by Ensembl | Yes (re-download + overwrite) |
