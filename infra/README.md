# infra — Data Infrastructure Scripts

Reproducible setup for the Iceberg datalake that powers the `autonomics` data-engine. These scripts define schemas, ingest panel data, and manage catalog tables consumed by the Rust `datalake` crate and DAG nodes.

## Prerequisites

- **Python 3.13+** (managed via [uv](https://docs.astral.sh/uv/))
- **Iceberg REST catalog** running (e.g. [Apache Iceberg REST](https://iceberg.apache.org/docs/latest/rest-catalog-setup/))
- **Garage S3** (or any S3-compatible storage) for the warehouse backend
- **Hail** (optional, only needed for `main.py`)

## Unified Connection Layer

All Python infra scripts share a single Iceberg connection via the `datalake`
package (`infra/datalake/`). Import it anywhere:

```python
from datalake import get_catalog

catalog = get_catalog()       # singleton, reads ICEBERG_* env vars
table = catalog.load_table("ld_score.ukbb_eur")
```

The old `infra/ld_score/config.py` is now a thin re-export shim for backward
compatibility — new code should import from `datalake` directly.

### Environment Variables

| Variable | Default | Required | Description |
|---|---|---|---|
| `ICEBERG_REST_URI` | `http://localhost:8181/catalog` | no | REST catalog endpoint |
| `ICEBERG_S3_ENDPOINT` | `http://localhost:3900` | no | S3 / Garage endpoint |
| `ICEBERG_S3_ACCESS_KEY_ID` | — | **yes** | S3 access key |
| `ICEBERG_S3_SECRET_ACCESS_KEY` | — | **yes** | S3 secret key |
| `ICEBERG_S3_REGION` | `garage` | no | S3 region |
| `ICEBERG_S3_BUCKET` | `datalake` | no | Warehouse / bucket name |

## Quick Start

```bash
cd infra
uv sync                          # install dependencies
python -c "from datalake import get_catalog; print(get_catalog())"
```

## LD-Score Panel Scripts

Located in `infra/ld_score/`.

### Data Flow

```
Hail / external tool
        │
        ▼
   main.py ──(Hail Table → Parquet)──►  .parquet directory
                                           │
                                           ▼
   sink_parquet.py ──(read, reshape, append)──►  Iceberg table
                                                   ld_score.ukbb_eur
                                                        │
                                                        ▼  (consumed by Rust)
                                            LdscHsqNode (data-engine)
```

### `main.py` — Hail → Parquet

Exports a Hail Table of LD scores to a single-partition Parquet file. Requires `hail` to be installed.

```bash
python main.py --input ../UKBB.AFR.ldscore.ht --output ./UKBB.AFR.ldscore.parquet
```

### `sink_parquet.py` — Parquet → Iceberg

Reads a Parquet file, rebuilds Spark-flattened struct columns into proper nested structs, and appends the data to an Iceberg table.

```bash
# Ingest EUR panel
python sink_parquet.py --parquet-path ./UKBB.EUR.ldscore.parquet --table-name ukbb_eur

# Ingest AFR panel with overwrite
python sink_parquet.py --parquet-path ./UKBB.AFR.ldscore.parquet --table-name ukbb_afr --mode overwrite
```

### `datalake.py` — Catalog helper

Provides a `Datalake` class for ad-hoc catalog inspection. Also serves as a connectivity smoke-test.

```bash
python datalake.py               # smoke-test: prints namespaces + gwas.test
```

### `preview.py` — Table listing / scan

Lists tables in a namespace or scans a specific table to pandas.

```bash
python preview.py                              # list tables in ld_score namespace
python preview.py --table ukbb_eur --scan       # scan table to pandas
```

## LD-Score Iceberg Schema

Defined in `ld_score/schema.py`, this is the canonical schema for all LD-score panel tables:

| Field | Type | Description |
|---|---|---|
| `locus` | struct { contig: string, position: int32 } | Genomic position |
| `alleles` | list\<string\> | Alt alleles |
| `rsid` | string | dbSNP identifier |
| `AF` | float64 | Allele frequency |
| `ld_score` | float64 | LD score value |

The Rust `LdscHsqNode` consumes `rsid`, `ld_score`, and `locus.position` from this table via an SQL join.

> **S-LDSC note:** The current schema has a single `ld_score` column (univariate h²).
> Stratified LDSC (`LdscSldscNode`) requires multi-annotation LD scores (baselineLD v2.2,
> 97 columns). Until the lake has a multi-annotation panel table, the S-LDSC node reads
> from files instead. The reference panel is archived at
> `aliyun:autonomics-data/ldsc/s-ldsc-ref/` — restore with:
> ```bash
> rclone copy aliyun:autonomics-data/ldsc/s-ldsc-ref/ reference/ldsc_data/ -P
> ```

## Reference Genome Scripts

Located in `infra/sink_reference_genome/`.

Ingests the **GRCh37** (hg19) human reference genome from Ensembl into the
`reference` namespace:

- `reference.grch37_contigs` — chromosome metadata (name, length, MD5)
- `reference.grch37_genes` — structured gene annotation from GTF (~2.6M features)

Raw files are archived to `aliyun:autonomics-data/reference/grch37/`.

```bash
python -m sink_reference_genome.main --staging-dir ../reference/grch37 --mode overwrite
```
