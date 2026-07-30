# Tag-Induced LD Subgraph Data Infrastructure

[English](subgraph.md) | [中文](subgraph_zh.md)

> **Namespace**: `iceberg.mixer`  
> **Table**: `eur_subgraph` (genome-wide merged into a single table)  
> **Population**: EUR (1000 Genomes Phase 3)  
> **Build status**: Live, 267,215,170 rows (267M LD edges)

## Schema

A single Iceberg table; each row is a tag-induced LD edge:

| Column   | Type     | Description                                  |
|----------|----------|----------------------------------------------|
| `chrom`  | Int64    | Chromosome number (1–22)                     |
| `id_a`   | Utf8     | rsid of SNP A                                |
| `id_b`   | Utf8     | rsid of SNP B                                |
| `r2`     | Float32  | Unphased LD r²                               |
| `h_a`    | Float32  | Heterozygosity of SNP A: 2·maf·(1-maf)       |
| `h_b`    | Float32  | Heterozygosity of SNP B                      |

**Semantics**: each row is an LD pair with r²≥0.05 where **at least one endpoint is a tag**
(tag-induced). Non-tag↔non-tag pairs are not in the table — the fold does not need them.

**rsid identification**: rsid strings (not integer indices) are used because precompute and
consumer nodes (mixer nodes) have different index spaces (full AF panel vs. universe);
integer indices cannot align across processes.

**h precomputed**: heterozygosity is computed from AF during precompute and stored, so
consumers do not query the AF table for h.

## Construction

Three-step pipeline: **per-chrom precompute** → **merge + rsid conversion** → **Iceberg upload**.

```
af.eur_af + ld_matrix.eur_chr{N}
    │
    ▼  precompute_tags2 <N>  (per-chrom, run one at a time)
select_tags → tag set
    │
    ▼  filter tag-induced edges (either endpoint is a tag, r²≥0.05)
chr{N}_subgraph.parquet  (UInt32 local index + h)
    │
    ▼  merge_subgraph  (read AF to rebuild rsid list, local idx → rsid)
eur_subgraph.parquet  (rsid + chrom + h, single file)
    │
    ▼  lake_cli upload --parquet --overwrite
iceberg.mixer.eur_subgraph  (SQL queryable)
```

### Step 1: per-chrom precompute

**Tool**: `cargo run -p data-engine --bin precompute_tags2 -- {N}`

For each chromosome:
1. Read AF (`af.eur_af WHERE chrom=N`) → rsids / maf_vec / rsid→idx / h_vec.
2. Read LD once (`ld_matrix.eur_chr{N} WHERE r²≥0.05`) → materialize `Vec<(a,b,r²)>`.
3. Filter r²>0.8 from materialized data to build adjacency CSR → `select_tags` → tag set.
4. Filter tag-induced edges from materialized data (either endpoint is a tag) → `chr{N}_subgraph.parquet`.

Subset is allocated by per-chrom SNP proportion (`GLOBAL_SUBSET × chr_snps / total_snps`).

**Run all chromosomes**:
```bash
for c in $(seq 1 22); do
    cargo run -p data-engine --bin precompute_tags2 -- $c
done
```

Output: `chr1_subgraph.parquet` … `chr22_subgraph.parquet` (per-chrom, UInt32 local index + h_a/h_b).

### Step 2: merge + rsid conversion

**Tool**: `cargo run -p data-engine --bin merge_subgraph -- . eur_subgraph.parquet`

For each chromosome:
1. Read AF to rebuild the rsid list (same scan order as precompute_tags2).
2. Read `chr{N}_subgraph.parquet`'s `a_idx/b_idx` (UInt32 local index).
3. Look up `rsids[a_idx]` → rsid string.
4. Output `(chrom, id_a, id_b, r2, h_a, h_b)`.

Output: `eur_subgraph.parquet` (single file, rsid format, ~2.3 GB).

### Step 3: Iceberg upload

```bash
cargo run -p data-engine --bin lake_cli -- \
    upload eur_subgraph.parquet mixer.eur_subgraph --parquet --overwrite
```

## Required inputs

| Input                    | Source                                         | Notes                          |
|--------------------------|------------------------------------------------|--------------------------------|
| `af.eur_af`              | Iceberg data lake (AF infrastructure)          | MAF / rsid list / heterozygosity |
| `ld_matrix.eur_chr{N}`   | Iceberg data lake (LD matrix infrastructure)   | r² LD pairs (precompute scan)  |
| `mixer::extract::select_tags` | bio_crates/mixer                           | Tag selection algorithm (MAF+subset+pruning) |

## Key parameters

| Parameter       | Default    | Location             | Description                          |
|-----------------|------------|----------------------|--------------------------------------|
| MAF_MIN         | 0.05       | precompute_tags2     | Lower bound on tag candidate MAF     |
| R2_PRUNE        | 0.8        | precompute_tags2     | LD pruning threshold (for tag selection) |
| R2_MIN          | 0.05       | precompute_tags2     | Minimum subgraph r² (for fold)       |
| GLOBAL_SUBSET   | 2,000,000  | precompute_tags2     | Genome-wide subset budget            |
| SEED            | 123        | precompute_tags2     | Random seed (reproducible)           |

## Downstream consumers

| Consumer             | Query                                                    | Purpose                     |
|----------------------|----------------------------------------------------------|-----------------------------|
| univariate_mixer     | `SELECT ... FROM eur_subgraph WHERE chrom={N}`          | LD fold (replaces ld_matrix) |
| bivariate_mixer      | same                                                     | LD fold                     |
| Any SQL tool         | `SELECT COUNT(*) FROM mixer.eur_subgraph`                | Data exploration            |

## Verification

```bash
# Row count
cargo run -p data-engine --bin lake_cli -- count mixer.eur_subgraph
# → mixer.eur_subgraph: 267215170 rows

# Schema
cargo run -p data-engine --bin lake_cli -- schema mixer.eur_subgraph

# Sample
cargo run -p data-engine --bin lake_cli -- query \
    "SELECT * FROM iceberg.mixer.eur_subgraph WHERE chrom = 22 LIMIT 10"
```

## Scale reference

| Metric           | Value             |
|------------------|-------------------|
| Total rows       | 267,215,170       |
| File size        | ~2.3 GB (Parquet) |
| Chromosomes      | 22                |
| Largest chrom    | chr6 (35.6M edges) |
| Smallest chrom   | chr21 (2.9M edges) |
| Total tags       | ~2M (genome-wide)  |

## Rebuild conditions

Rebuild when any of the following changes:

| Change                     | Rebuild? |
|----------------------------|----------|
| Change GWAS trait          | **No** (subgraph is GWAS-independent) |
| Change MAF/r²/subset/seed  | **Yes**  |
| Change ref panel / AF / LD | **Yes**  |
| Add new chromosome         | **Yes** (run precompute for the new chrom + re-merge) |
