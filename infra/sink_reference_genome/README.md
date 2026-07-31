# sink_reference_genome — Human Reference Genome Ingest

Loads the **GRCh37** (hg19) and/or **GRCh38** (hg38) human reference genome
from Ensembl into the Iceberg data lake as queryable tables, and archives the
raw files to OSS.

## Source Data

| Assembly | FASTA | GTF | Ensembl Release |
|----------|-------|-----|-----------------|
| **GRCh37** (hg19) | `Homo_sapiens.GRCh37.dna.chromosome.*.fa.gz` | `Homo_sapiens.GRCh37.87.gtf.gz` | Archive (r87, last GRCh37 update) |
| **GRCh38** (hg38) | `Homo_sapiens.GRCh38.dna.chromosome.*.fa.gz` | `Homo_sapiens.GRCh38.116.gtf.gz` | Release 116 |

## Data Flow

```
Ensembl FTP
        │
        ▼
   main.py --assembly {grch37|grch38}
        │
        ├──(parse FASTA)──►  contig metadata (contig, length, md5)
        │                        │
        │                        ▼
        │               Iceberg: reference.{prefix}_contigs
        │
        ├──(parse GTF)──────►  gene annotation (structured features)
        │                        │
        │                        ▼
        │               Iceberg: reference.{prefix}_genes
        │
        └──(rclone copy)────────►  aliyun:autonomics-data/reference/{assembly}/
```

## Usage

```bash
cd infra
uv sync

# GRCh37 (Ensembl archive, release 87)
python -m sink_reference_genome.main --assembly grch37 \
    --staging-dir ../reference/grch37 --mode overwrite

# GRCh38 (Ensembl release 116)
python -m sink_reference_genome.main --assembly grch38 \
    --staging-dir ../reference/grch38 --mode overwrite

# Use pre-downloaded per-chromosome files
python -m sink_reference_genome.main --assembly grch38 \
    --fasta-dir ../reference/grch38 \
    --gtf ../reference/grch38/Homo_sapiens.GRCh38.116.gtf.gz \
    --mode overwrite

# Archive raw files only (no Iceberg ingest)
python -m sink_reference_genome.main --assembly grch38 --archive-only \
    --staging-dir ../reference/grch38
```

## Output Tables

### `reference.{grch37|grch38}_contigs`

One row per chromosome / scaffold.

| Column   | Type   | Description |
|----------|--------|-------------|
| `contig` | string | Chromosome name (1–22, X, Y, MT) |
| `length` | int32  | Sequence length (bp) |
| `md5`    | string | MD5 checksum of the uppercase sequence |

### `reference.{grch37|grch38}_genes`

One row per GTF feature (gene, transcript, exon, CDS, UTR, …).

| Column                 | Type   | Description |
|------------------------|--------|-------------|
| `contig`               | string | Chromosome |
| `source`               | string | Annotation source |
| `feature`              | string | Feature type (`gene`, `transcript`, `exon`, `CDS`, …) |
| `start`                | int32  | Start position (1-based, inclusive) |
| `end_pos`              | int32  | End position (1-based, inclusive) |
| `strand`               | string | `+` or `-` |
| `frame`                | string | Reading frame (`0`, `1`, `2`, or null) |
| `gene_id`              | string | Ensembl gene ID (`ENSG…`) |
| `gene_name`            | string | Gene symbol |
| `gene_biotype`         | string | Biotype (`protein_coding`, `lncRNA`, …) |
| `transcript_id`        | string | Ensembl transcript ID (`ENST…`) |
| `transcript_name`      | string | Transcript name |
| `transcript_biotype`   | string | Transcript biotype |
| `exon_id`              | string | Ensembl exon ID (`ENSE…`) |
| `exon_number`          | int32  | Exon number within transcript |
| `protein_id`           | string | Ensembl protein ID (`ENSP…`) |

> **`end_pos`** avoids the SQL reserved word `end`.

## Scale

| Assembly | Contigs | Gene features | Genes | Release |
|----------|---------|---------------|-------|---------|
| GRCh37   | 25      | 2,613,765     | 57,905  | r87 |
| GRCh38   | 25      | 11,248,794    | 78,941  | r116 |

## Query Examples

```sql
-- Compare BRCA1 coordinates across assemblies
SELECT 'GRCh37' AS assembly, start, end_pos
FROM reference.grch37_genes
WHERE gene_name = 'BRCA1' AND feature = 'gene'
UNION ALL
SELECT 'GRCh38', start, end_pos
FROM reference.grch38_genes
WHERE gene_name = 'BRCA1' AND feature = 'gene';

-- Protein-coding genes on chromosome 22 (GRCh38)
SELECT gene_id, gene_name, start, end_pos
FROM reference.grch38_genes
WHERE contig = '22' AND feature = 'gene' AND gene_biotype = 'protein_coding'
ORDER BY start;

-- Total genome size
SELECT 'GRCh37' AS assembly, SUM(length) AS total_bp FROM reference.grch37_contigs
UNION ALL
SELECT 'GRCh38', SUM(length) FROM reference.grch38_contigs;
```

## OSS Archive

| Assembly | Path |
|----------|------|
| GRCh37 | `aliyun:autonomics-data/reference/grch37/` |
| GRCh38 | `aliyun:autonomics-data/reference/grch38/` |

```bash
rclone copy aliyun:autonomics-data/reference/grch38/ reference/grch38/ -P
```
