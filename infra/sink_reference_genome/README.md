# sink_reference_genome — GRCh37 Reference Genome Ingest

Loads the **GRCh37** (hg19) human reference genome from Ensembl into the Iceberg
data lake as two queryable tables, and archives the raw files to OSS.

## Source Data

| File | Source | Description |
|------|--------|-------------|
| `Homo_sapiens.GRCh37.dna.primary_assembly.fa.gz` | Ensembl FTP `grch37/current/fasta/` | Primary assembly FASTA (no alt loci) |
| `Homo_sapiens.GRCh37.87.gtf.gz` | Ensembl FTP `grch37/current/gtf/` | Gene annotation (Ensembl r87 on GRCh37) |

The GRCh37 archive is gene annotation from Ensembl **release 87** (the last
release with GRCh37 updates); the genome build itself is **GRCh37.p13**.

## Data Flow

```
Ensembl FTP (grch37/current)
        │
        ▼
   main.py ──(parse FASTA)──►  contig metadata (contig, length, md5)
        │                        │
        │                        ▼
        │                   Iceberg: reference.grch37_contigs
        │
        ├──(parse GTF)──────►  gene annotation (structured features)
        │                        │
        │                        ▼
        │                   Iceberg: reference.grch37_genes
        │
        └──(rclone copy)────────►  aliyun:autonomics-data/reference/grch37/
```

## Usage

```bash
cd infra
uv sync

# Full pipeline: download + parse + ingest + archive
python -m sink_reference_genome.main --staging-dir ../reference/grch37 --mode overwrite

# Use pre-downloaded files (skip download)
python -m sink_reference_genome.main \
    --fasta ../reference/grch37/Homo_sapiens.GRCh37.dna.primary_assembly.fa.gz \
    --gtf   ../reference/grch37/Homo_sapiens.GRCh37.87.gtf.gz \
    --mode overwrite

# GTF only (skip FASTA ingest)
python -m sink_reference_genome.main --skip-fasta --staging-dir ../reference/grch37

# Archive raw files only (no Iceberg ingest)
python -m sink_reference_genome.main --archive-only --staging-dir ../reference/grch37
```

## Output Tables

### `reference.grch37_contigs`

One row per chromosome / scaffold in the primary assembly.

| Column   | Type   | Description |
|----------|--------|-------------|
| `contig` | string | Chromosome name (1–22, X, Y, MT) |
| `length` | int32  | Sequence length (bp) |
| `md5`    | string | MD5 checksum of the uppercase sequence |

### `reference.grch37_genes`

One row per GTF feature (gene, transcript, exon, CDS, UTR, …).

| Column                 | Type   | Description |
|------------------------|--------|-------------|
| `contig`               | string | Chromosome |
| `source`               | string | Annotation source (e.g. `ensembl`) |
| `feature`              | string | Feature type (`gene`, `transcript`, `exon`, `CDS`, …) |
| `start`                | int32  | Start position (1-based, inclusive) |
| `end_pos`              | int32  | End position (1-based, inclusive) |
| `strand`               | string | `+` or `-` |
| `frame`                | string | Reading frame (`0`, `1`, `2`, or null) |
| `gene_id`              | string | Ensembl gene ID (`ENSG…`) |
| `gene_name`            | string | Gene symbol (e.g. `TSPAN6`) |
| `gene_biotype`         | string | Biotype (`protein_coding`, `lncRNA`, …) |
| `transcript_id`        | string | Ensembl transcript ID (`ENST…`) |
| `transcript_name`      | string | Transcript name |
| `transcript_biotype`   | string | Transcript biotype |
| `exon_id`              | string | Ensembl exon ID (`ENSE…`) |
| `exon_number`          | int32  | Exon number within transcript |
| `protein_id`           | string | Ensembl protein ID (`ENSP…`) |

> **`end_pos`** avoids the SQL reserved word `end`.

## Query Examples

```sql
-- All protein-coding genes on chromosome 22
SELECT gene_id, gene_name, start, end_pos
FROM reference.grch37_genes
WHERE contig = '22' AND feature = 'gene' AND gene_biotype = 'protein_coding'
ORDER BY start;

-- Total exon count per chromosome
SELECT contig, COUNT(*) AS n_exons
FROM reference.grch37_genes
WHERE feature = 'exon'
GROUP BY contig ORDER BY contig;

-- Genome size
SELECT SUM(length) AS total_bp FROM reference.grch37_contigs;
```

## OSS Archive

Raw files are stored at `aliyun:autonomics-data/reference/grch37/`. Restore:

```bash
rclone copy aliyun:autonomics-data/reference/grch37/ reference/grch37/ -P
```
