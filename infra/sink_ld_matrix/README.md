# sink_ld_matrix

Sinks the 1000G LD matrix (zstd-compressed TSV, produced by
`infra/thousand_genomes/ld_matrix_unphased_r2.sh`) into Apache Iceberg — one
table per chromosome under the `ld_matrix` namespace, prefixed by population.

## Layout

- `iceberg.ld_matrix.<pop>_chr1` … `<pop>_chr22` — e.g. `eur_chr1`, `eas_chr1`,
  `afr_chr1`, `sas_chr1`, `amr_chr1`. One Iceberg table per (population, chr).

Each chromosome table has the schema:

| column         | type    | source TSV header |
|----------------|---------|-------------------|
| `chrom_a`      | Int64   | `#CHROM_A`        |
| `pos_a`        | Int64   | `POS_A`           |
| `id_a`         | Utf8    | `ID_A`            |
| `chrom_b`      | Int64   | `#CHROM_B`        |
| `pos_b`        | Int64   | `POS_B`           |
| `id_b`         | Utf8    | `ID_B`            |
| `unphased_r2`  | Float64 | `UNPHASED_R2`     |

Column names are assigned *positionally* via `CsvReadOptions::schema`, not by
name — the source TSV's first header is `#CHROM_A`, and DataFusion treats `#`
as a qualifier separator, so name-based renaming fails silently. `#` is also
rejected by the Iceberg field-name spec, so names are lowercased. `pos_a`/
`pos_b` are safe — iceberg-rust reserves only the bare `pos`.

## How it works

- **Concurrency**: up to `available_parallelism()` chromosomes are sunk at once,
  each running its own single-threaded zstd-decode → parquet-encode pipeline.
  Override with `SINK_CONCURRENCY=N` (e.g. `2` to be kind to a shared disk).
- **Idempotent**: tables that already have a committed snapshot are skipped. A
  table created by a killed run (empty — no snapshot yet) is populated on the
  next run. To force a rewrite, drop the table manually first.
- Each chromosome owns its own `SessionContext`; the iceberg catalog handle is
  shared via `Arc`.

## Run

```bash
cargo run -p sink_ld_matrix                    # sink all 5 populations (EUR EAS AFR SAS AMR)
cargo run -p sink_ld_matrix -- EAS             # sink one population
cargo run -p sink_ld_matrix -- EAS AFR SAS     # sink specific populations
```

Population names are case-insensitive (normalized to uppercase). The data
directory is resolved automatically:
1. Canonical: `unphased_r2/<POP>/ld/` (produced by `ld_matrix_unphased_r2.sh`)
2. Legacy:    `<pop_lower>/ld/` (original EUR run)

Output lines: `[start]`, `[done]`/`[skip]`/`[FAIL]` per chromosome, plus a
per-population `summary: N written, M skipped, K failed`.

### Verify

```bash
# Fast, metadata-only: which tables have committed data? (no data scan)
cargo run -p sink_ld_matrix --example check_tables

# Per-chromosome row counts + grand total (scans the data)
cargo run -p sink_ld_matrix --example verify_all
```
