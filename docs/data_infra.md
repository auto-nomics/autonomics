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
```

## Existing docs

| Infrastructure | Table | Doc | Status |
|----------------|-------|-----|--------|
| **LD matrix** | `ld_matrix.eur_chr{1..22}` | [ld_matrix.md](data_infra/ld_matrix.md) | ✅ Live |
| **LD subgraph** | `mixer.eur_subgraph` | [subgraph.md](data_infra/subgraph.md) | ✅ Live (tag consistency fix pending) |

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
