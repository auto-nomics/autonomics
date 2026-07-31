# GRCh37 Reference Genome Data Infrastructure

[English](reference_genome.md) | [中文](reference_genome_zh.md)

> **Namespace**: `reference`
> **Tables**: `grch37_contigs`, `grch37_genes`
> **Assembly**: GRCh37.p13 (hg19)
> **Gene annotation**: Ensembl release 87 (last GRCh37 update)
> **Build status**: Live

## Overview

The human reference genome **GRCh37** (also known as hg19) is loaded as a
reusable reference panel in the Iceberg data lake. It provides:

- **Contig metadata** — chromosome names, lengths, and MD5 checksums for
  integrity verification.
- **Gene annotation** — a fully structured, SQL-queryable table of all
  Ensembl gene models (genes, transcripts, exons, CDS, UTRs).

The raw FASTA and GTF files are archived to OSS for tools that require the
original file formats (BWA, minimap2, VEP, etc.).

## Source

| File | URL | Description |
|------|-----|-------------|
| `Homo_sapiens.GRCh37.dna.primary_assembly.fa.gz` | `ftp.ensembl.org/pub/grch37/current/fasta/homo_sapiens/dna/` | Primary assembly (no alt loci) |
| `Homo_sapiens.GRCh37.87.gtf.gz` | `ftp.ensembl.org/pub/grch37/current/gtf/homo_sapiens/` | Ensembl r87 gene annotation |

> **Why GRCh37?** Many GWAS summary statistics and LD reference panels in the
> data lake use GRCh37 coordinates. Maintaining a structured GRCh37 reference
> enables in-lake coordinate annotation without cross-assembly liftover.

## Schema

### `reference.grch37_contigs`

| Column   | Type   | Description |
|----------|--------|-------------|
| `contig` | string | Chromosome / scaffold name (1–22, X, Y, MT) |
| `length` | int32  | Sequence length in base pairs |
| `md5`    | string | MD5 of the uppercase sequence (matches `samtools faidx`) |

**Scale**: ~93 contigs (primary assembly: chromosomes 1–22, X, Y, MT + unplaced scaffolds).

### `reference.grch37_genes`

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

**Scale**: ~2.6M features total (genes, transcripts, exons, CDS, UTRs, etc.).

## Construction

**Tool**: `infra/sink_reference_genome/main.py`

```
Ensembl FTP
    │
    ├── FASTA ──(parse contigs)──► reference.grch37_contigs
    │                                   │
    ├── GTF ────(parse features)──► reference.grch37_genes
    │
    └── raw files ──(rclone)──► aliyun:autonomics-data/reference/grch37/
```

```bash
cd infra
uv sync
python -m sink_reference_genome.main --staging-dir ../reference/grch37 --mode overwrite
```

## Query Examples

```sql
-- Genome size
SELECT SUM(length) AS total_bp FROM reference.grch37_contigs;

-- All protein-coding genes on chromosome 22
SELECT gene_id, gene_name, start, end_pos
FROM reference.grch37_genes
WHERE contig = '22' AND feature = 'gene' AND gene_biotype = 'protein_coding'
ORDER BY start;

-- Exon count per chromosome
SELECT contig, COUNT(*) AS n_exons
FROM reference.grch37_genes
WHERE feature = 'exon'
GROUP BY contig ORDER BY contig;

-- Gene by rsid region lookup (join with GWAS data)
SELECT g.gene_name, g.start, g.end_pos, s.rsid, s.pval
FROM reference.grch37_genes g
JOIN gwas.my_study s
  ON s.chrom = g.contig
  AND s.pos BETWEEN g.start AND g.end_pos
WHERE g.feature = 'gene' AND g.gene_biotype = 'protein_coding';
```

## Downstream Consumers

| Consumer | Purpose |
|----------|---------|
| GWAS annotation nodes | Map SNPs to genes by genomic coordinates |
| MAGMA / gene-based analysis | Gene boundaries for gene-set tests |
| MiXeR / LAVA | Reference coordinates for locus definition |
| Any SQL tool | Gene/region queries without loading external files |

## OSS Archive

Raw FASTA + GTF files at `aliyun:autonomics-data/reference/grch37/`:

```bash
rclone copy aliyun:autonomics-data/reference/grch37/ reference/grch37/ -P
```

## Rebuild Conditions

| Change | Rebuild? |
|--------|----------|
| Add a new GWAS trait | No (reference is trait-independent) |
| GRCh37 archive updated by Ensembl | Yes (re-download + overwrite) |
| Switch to GRCh38 | New tables (`grch38_*`), keep GRCh37 alongside |
