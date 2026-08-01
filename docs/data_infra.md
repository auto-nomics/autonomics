# Data Infrastructure Index

[English](data_infra.md) | [中文](data_infra_zh.md)

> This directory indexes all data infrastructure persisted in the Iceberg data lake.
> Each infrastructure asset has a dedicated document under `docs/data_infra/` recording its schema, construction pipeline, and required inputs.

## Dependency graph

```
1000G VCF (raw genotype)
    │
    ├──→ [AF panel] af.eur_af              (allele frequencies)
    │
    └──→ [LD matrix] ld_matrix.eur_chr{N}  (LD r² matrix)
              │
              ├── af.eur_af ──→ [tag panel] mixer.eur_tag_panel   (tag SNP list)
              │                [LD subgraph] mixer.eur_subgraph   (tag-induced LD subgraph)
              │                        │
              │                        └──→ MiXeR univariate/bivariate nodes
              │
              └──→ [LD score] ld_score.*  (for LD Score Regression)

GWAS VCF (public summary statistics)
    │
    └──→ [GWAS sumstats] gwas.*            (Z/N/rsid summary statistics)
              │
              └──→ MiXeR / LDSC / MR nodes

Ensembl GRCh37 / GRCh38 (reference genome)
    │
    ├──→ [Contigs] reference.grch{37,38}_contigs    (chromosome metadata)
    └──→ [Gene annotation] reference.grch{37,38}_genes (structured GTF features)
              │
              └──→ SNP-to-gene mapping, gene-based analysis

dbSNP build 155 (variant catalog)
    │
    └──→ [Variants] reference.dbsnp155   (~928M variants, dual-assembly coords)
              │
              └──→ rsID ↔ position lookup (GRCh37 + GRCh38)
```

## Existing docs

| Infrastructure | Table | Doc | Status |
|----------------|-------|-----|--------|
| **LD matrix** | `ld_matrix.eur_chr{1..22}` | [ld_matrix.md](data_infra/ld_matrix.md) | ✅ Live |
| **LD subgraph** | `mixer.eur_subgraph` | [subgraph.md](data_infra/subgraph.md) | ✅ Live (tag consistency fix pending) |
| **Reference genome** | `reference.grch{37,38}_contigs`, `reference.grch{37,38}_genes` | [reference_genome.md](data_infra/reference_genome.md) | ✅ Live |
| **dbSNP155** | `reference.dbsnp155` | [reference_genome.md](data_infra/reference_genome.md) | ✅ Live |

## Pending docs

| Infrastructure | Table | Expected doc | Priority |
|----------------|-------|--------------|----------|
| **AF panel** | `af.eur_af` | `data_infra/af.md` | High (all nodes depend on it) |
| **GWAS sumstats** | `gwas.*` | `data_infra/gwas_sumstats.md` | High (MiXeR/LDSC/MR input) |
| **tag panel** | `mixer.eur_tag_panel` | `data_infra/tag_panel.md` | Medium (same source as subgraph) |
| **LD score** | `ld_score.*` | `data_infra/ld_score.md` | Medium (LDSC-specific) |
| **datalake** | Iceberg REST catalog | `data_infra/datalake.md` | Low (infrastructure layer) |

## Quick reference

### LD matrix

- **Table**: `iceberg.ld_matrix.eur_chr{N}` (per-chrom, 22 tables)
- **Columns**: `chrom_a, pos_a, id_a, chrom_b, pos_b, id_b, unphased_r2`
- **Pipeline**: 1000G VCF → PLINK2 QC + `--r2` → zstd TSV → `sink_ld_matrix` → Iceberg
- **Parameters**: MAF≥0.01, GENO≤0.05, window 10 Mb, r²≥0.01
- **Details**: [ld_matrix.md](data_infra/ld_matrix.md)

### LD subgraph (tag-induced)

- **Table**: `iceberg.mixer.eur_subgraph` (genome-wide merged)
- **Columns**: `chrom, id_a, id_b, r2, h_a, h_b`
- **Pipeline**: precompute_tags (per-chrom tag selection + tag-induced edge filtering) → merge_subgraph (rsid conversion + merge) → lake_cli upload
- **Scale**: 267M rows, 2.3 GB
- **Details**: [subgraph.md](data_infra/subgraph.md)

### Reference genome (GRCh37 + GRCh38)

- **Tables**: `iceberg.reference.grch{37,38}_contigs`, `iceberg.reference.grch{37,38}_genes`
- **Contigs**: `contig, length, md5` (25 rows each, chromosomes 1–22, X, Y, MT)
- **Genes**: 16 structured columns from Ensembl GTF
  - GRCh37 r87: 2,613,765 features (57,905 genes)
  - GRCh38 r116: 11,248,794 features (78,941 genes)
- **Pipeline**: Ensembl FTP → per-chromosome FASTA + GTF → `sink_reference_genome --assembly {grch37|grch38}` → Iceberg
- **Archive**: `aliyun:autonomics-data/reference/grch{37,38}/`
- **Details**: [reference_genome.md](data_infra/reference_genome.md)

### dbSNP155 variants

- **Table**: `iceberg.reference.dbsnp155`
- **Columns**: `rsid (int64), chrom (string), pos_37, pos_38, ref_37, ref_38, alt_37, alt_38`
- **Scale**: ~928M variants across 25 chromosomes
- **Pipeline**: dbSNP155 Parquet → `sink_dbsnp` → Iceberg (per-chromosome append)
- **Archive**: `aliyun:autonomics-data/reference/dbsnp155/dbSNP155_v0.9.tar`
- **Details**: [reference_genome.md](data_infra/reference_genome.md)

### AF panel (pending)

- **Table**: `iceberg.af.eur_af`
- **Columns**: `id, chrom, alt_freq`
- **Used for**: MAF computation, universe construction, heterozygosity h

### GWAS sumstats (pending)

- **Table**: `iceberg.gwas.{trait_id}` (per-trait)
- **Columns**: `rsid, chrom, effect_size, std_error, sample_size, ...`
- **Used for**: Z-score (β/SE), sample size N, universe construction

## Tool index

| Tool | Purpose | Location |
|------|---------|----------|
| `precompute_tags` | Per-chrom precompute of tag panel + subgraph | `src/bin/precompute_tags.rs` |
| `merge_subgraph` | Merge per-chrom subgraph + convert to rsid | `src/bin/merge_subgraph.rs` |
| `lake_cli` | Data lake query / management / upload | `src/bin/lake_cli.rs` |
| `sink_ld_matrix` | LD matrix TSV → Iceberg ingest | `infra/sink_ld_matrix/` |
| `ld_matrix.sh` | 1000G VCF → PLINK2 LD computation | `infra/thousand_genomes/ld_matrix.sh` |
| `sink_reference_genome` | GRCh37/GRCh38 FASTA + GTF → Iceberg ingest | `infra/sink_reference_genome/` |
| `sink_dbsnp` | dbSNP155 Parquet → Iceberg ingest | `infra/sink_dbsnp/` |
