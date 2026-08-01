# sink_dbsnp — dbSNP155 Variant Data Ingest

Loads **dbSNP build 155** variant data into the Iceberg data lake as a single
queryable table `reference.dbsnp155`, containing ~756M variants with positions
on both GRCh37 and GRCh38.

## Source Data

| File | Location | Description |
|------|----------|-------------|
| Per-chromosome Parquet | `dbSNP155/v155/CHR={1-22,X,Y,MT}/part-0.parquet` | Variant data with both build positions |
| `dbSNP155_v0.9.tar` | 17 GB tar archive | Original download for archival |

### Source Parquet Schema

| Column | Type | Description |
|--------|------|-------------|
| `RSID` | int32 | dbSNP rs number (numeric part) |
| `POS_38` | int32 | GRCh38 genomic position |
| `POS_37` | int32 | GRCh37 genomic position |
| `REF_38` | string | GRCh38 reference allele |
| `REF_37` | string | GRCh37 reference allele |
| `ALT_38` | string | GRCh38 alternate allele(s) |
| `ALT_37` | string | GRCh37 alternate allele(s) |

## Usage

```bash
cd infra
uv sync

# Full ingest (all chromosomes, ~756M rows)
python -m sink_dbsnp.main --data-dir /mnt/disk2/dataset/dbSNP155/v155

# Single chromosome (for testing)
python -m sink_dbsnp.main --data-dir /mnt/disk2/dataset/dbSNP155/v155 --chrom 22

# Ingest + archive tar to OSS
python -m sink_dbsnp.main \
    --data-dir /mnt/disk2/dataset/dbSNP155/v155 \
    --archive-tar /mnt/disk2/dataset/dbSNP155_v0.9.tar
```

## Output Table

### `reference.dbsnp155`

One row per variant (~756M total).

| Column   | Type   | Description |
|----------|--------|-------------|
| `rsid`   | int64  | dbSNP rs number (numeric part, e.g. `171` = rs171) |
| `chrom`  | string | Chromosome (1–22, X, Y, MT) |
| `pos_37` | int32  | GRCh37 position |
| `pos_38` | int32  | GRCh38 position |
| `ref_37` | string | GRCh37 reference allele |
| `ref_38` | string | GRCh38 reference allele |
| `alt_37` | string | GRCh37 alternate allele(s) |
| `alt_38` | string | GRCh38 alternate allele(s) |

## Query Examples

```sql
-- Look up a variant by rsID
SELECT * FROM reference.dbsnp155 WHERE rsid = 171;

-- Cross-assembly coordinate lookup
SELECT rsid, chrom, pos_37, pos_38
FROM reference.dbsnp155
WHERE chrom = '22' AND pos_37 BETWEEN 20000000 AND 21000000
LIMIT 10;

-- Count variants per chromosome
SELECT chrom, COUNT(*) AS n_variants
FROM reference.dbsnp155
GROUP BY chrom ORDER BY chrom;
```

## OSS Archive

```bash
rclone copy aliyun:autonomics-data/reference/dbsnp155/dbSNP155_v0.9.tar . -P
```
