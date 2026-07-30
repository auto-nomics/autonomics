# LD Matrix Data Infrastructure

[English](ld_matrix.md) | [中文](ld_matrix_zh.md)

> **Namespace**: `iceberg.ld_matrix`  
> **Table**: `eur_chr1` … `eur_chr22` (one table per chromosome)  
> **Population**: EUR (1000 Genomes Phase 3)  
> **Build status**: Live, chr1–22 fully ingested

## Schema

One Iceberg table per chromosome:

| Column          | Type     | Description                         | Source TSV header |
|-----------------|----------|-------------------------------------|-------------------|
| `chrom_a`       | Int64    | Chromosome of SNP A                 | `#CHROM_A`        |
| `pos_a`         | Int64    | Physical position of SNP A (bp)     | `POS_A`           |
| `id_a`          | Utf8     | rsid of SNP A                       | `ID_A`            |
| `chrom_b`       | Int64    | Chromosome of SNP B                 | `#CHROM_B`        |
| `pos_b`         | Int64    | Physical position of SNP B (bp)     | `POS_B`           |
| `id_b`          | Utf8     | rsid of SNP B                       | `ID_B`            |
| `unphased_r2`   | Float64  | Unphased LD r² (0–1)               | `UNPHASED_R2`    |

**Semantics**: each row is an LD pair (r²). Only pairs with `r² ≥ LD_R2_MIN` (default 0.01) are stored —
sparse storage, no r²≈0 pairs.

**No integer SNP IDs in-table** — SNPs are identified by rsid strings. Integer indices are
built at runtime by consumers, not persisted (see consumer docs).

## Construction

The full pipeline has two stages: **upstream PLINK2 computation** → **Iceberg ingest**.

```
1000G VCF (raw)
    │
    ▼  infra/thousand_genomes/ld_matrix.sh
PLINK2 QC + --r2 computation
    │
    ▼
zstd TSV (per-chrom sparse LD matrix)
    │
    ▼  infra/sink_ld_matrix (Rust binary)
iceberg.ld_matrix.eur_chr{N} (per-chrom table)
```

### Stage 1: upstream PLINK2 computation

**Script**: `infra/thousand_genomes/ld_matrix.sh`

```bash
./ld_matrix.sh EUR          # Run EUR population chr1–22
POPS="EUR" CHRS="21 22" ./ld_matrix.sh   # Specify
```

**Steps**:
1. Extract EUR sample list (FID/IID) from the 1000G panel file.
2. Convert VCF to PLINK bfile (`.bed/.bim/.fam`) per chromosome.
3. QC filtering:
   - `--maf 0.01` (MAF ≥ 1%, statistical power floor for 1000G N=503)
   - `--geno 0.05` (SNP missing rate ≤ 5%)
4. PLINK2 `--r2` LD computation:
   - Window `10 Mb` (LD decay distance; r²≈0 beyond)
   - Threshold `r² ≥ 0.01` (only meaningful pairs stored)
5. Output: one `.ld.vcor.zst` (zstd-compressed TSV) per chromosome.

**Output directory**: `/mnt/disk2/dataset/1000g_plink/eur/ld/`

### Stage 2: Iceberg ingest

**Tool**: `infra/sink_ld_matrix` (`cargo run -p sink_ld_matrix`)

Reads zstd TSV, assigns column names **positionally** (`CsvReadOptions::schema`), writes to
Iceberg table. Each chromosome is written independently; concurrency = CPU core count
(`SINK_CONCURRENCY=N` to tune).

- **Idempotent**: tables with existing snapshots are skipped; empty tables from killed runs
  are auto-completed on the next run.
- **Rewrite**: `drop_table` first, then re-run.

**Column-name gotcha**: the source TSV first column `#CHROM_A` contains `#`, which DataFusion
treats as a qualifier separator → name-based renaming silently fails. A **positional schema**
is used instead, and `#` is rejected by Iceberg field-name rules → lowercased to `chrom_a`.

## Required inputs

| Input               | Location / source                                     | Notes                          |
|---------------------|-------------------------------------------------------|--------------------------------|
| 1000G VCF           | `/mnt/disk2/dataset/1000g_genotype_data/`             | 1000G Phase 3 whole-genome genotyping |
| Sample panel        | `integrated_call_samples_v3.20130502.ALL.panel`       | Population→sample mapping (EUR/AFR/…) |
| PLINK2              | System PATH                                           | LD computation (`--r2`)        |
| zstd                | System PATH                                           | TSV compression                |
| Iceberg REST catalog| Data lake config (`Datalake::new()`)                  | Table storage backend          |

## Key parameters

| Parameter     | Value  | Location             | Description                              |
|----------------|--------|----------------------|------------------------------------------|
| MAF_MIN        | 0.01   | ld_matrix.sh         | Minimum SNP MAF (QC filter)              |
| GENO_MAX       | 0.05   | ld_matrix.sh         | Maximum SNP missing rate (QC filter)     |
| LD_WINDOW_KB   | 10000  | ld_matrix.sh         | LD computation window (10 Mb)            |
| LD_R2_MIN      | 0.01   | ld_matrix.sh         | Storage threshold (r² < 0.01 not stored) |
| THREADS        | 8      | ld_matrix.sh         | PLINK2 thread count                      |

## Downstream consumers

| Consumer             | Query                                        | Purpose                     |
|----------------------|----------------------------------------------|-----------------------------|
| MiXeR extract        | `WHERE unphased_r2 > 0.8`                    | Tag selection (LD pruning)  |
| MiXeR LD fold        | `WHERE unphased_r2 >= 0.05`                  | Sufficient statistics (m1/m2) |
| LDSC                 | Full scan                                    | LD score computation        |
| precompute_tags      | `WHERE unphased_r2 > 0.8` + `>= 0.05`        | Precompute tag panel + subgraph |

## Verification

```bash
# Check table existence + row count
cargo run -p data-engine --bin lake_cli -- count ld_matrix.eur_chr22

# View schema
cargo run -p data-engine --bin lake_cli -- schema ld_matrix.eur_chr22

# Sample
cargo run -p data-engine --bin lake_cli -- query \
    "SELECT * FROM iceberg.ld_matrix.eur_chr22 WHERE unphased_r2 > 0.8 LIMIT 10"
```

## Scale reference

| Chromosome | SNP count (AF panel) | LD pairs (r²≥0.01) | Iceberg table name    |
|------------|----------------------|--------------------|-----------------------|
| chr1       | ~176k                | ~tens of M         | `eur_chr1`            |
| chr22      | ~32k                 | ~a few M           | `eur_chr22`           |
| Genome-wide| ~10.7M               | ~hundreds of M     | chr1–chr22 (22 tables) |
