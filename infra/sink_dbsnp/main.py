"""Ingest dbSNP155 variant data into the Iceberg data lake.

Reads per-chromosome Parquet files from the dbSNP155 dataset (which contains
both GRCh37 and GRCh38 coordinates) and loads them into a single Iceberg table
``reference.dbsnp155``.

Each row is one variant with positions for both assemblies:

- ``rsid`` (int64) — dbSNP rs number
- ``chrom`` (string) — chromosome (1–22, X, Y, MT)
- ``pos_37`` / ``pos_38`` (int32) — genomic position on each assembly
- ``ref_37`` / ``ref_38`` (string) — reference allele
- ``alt_37`` / ``alt_38`` (string) — alternate allele(s)

Usage
-----
    # Full ingest from local parquet directory
    python main.py --data-dir /mnt/disk2/dataset/dbSNP155/v155 --mode append

    # Single chromosome (for testing / resuming)
    python main.py --data-dir /mnt/disk2/dataset/dbSNP155/v155 --chrom 22

    # Skip specific chromosomes
    python main.py --data-dir /mnt/disk2/dataset/dbSNP155/v155 --skip-chrom 1,2,3
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

from datalake import get_catalog

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

NAMESPACE = "reference"
TABLE_NAME = "dbsnp155"

CHROMOSOMES = [*map(str, range(1, 23)), "X", "Y", "MT"]

# Target Iceberg schema
SCHEMA = pa.schema(
    [
        pa.field("rsid", pa.int64()),
        pa.field("chrom", pa.string()),
        pa.field("pos_37", pa.int32()),
        pa.field("pos_38", pa.int32()),
        pa.field("ref_37", pa.string()),
        pa.field("ref_38", pa.string()),
        pa.field("alt_37", pa.string()),
        pa.field("alt_38", pa.string()),
    ]
)

# rclone archive destination
ARCHIVE_REMOTE = "aliyun:autonomics-data/reference/dbsnp155"


# ---------------------------------------------------------------------------
# Parquet reading + reshaping
# ---------------------------------------------------------------------------


def load_chrom_parquet(data_dir: Path, chrom: str) -> pa.Table:
    """Read a per-chromosome parquet file and reshape to the target schema.

    Source columns: RSID (int32), POS_38, POS_37, REF_38, REF_37, ALT_38, ALT_37
    Target columns: rsid (int64), chrom (string), pos_37, pos_38, ref_37, ref_38, alt_37, alt_38
    """
    parquet_path = data_dir / f"CHR={chrom}" / "part-0.parquet"
    if not parquet_path.exists():
        raise FileNotFoundError(f"Missing parquet: {parquet_path}")

    raw = pq.read_table(parquet_path)
    n = raw.num_rows

    # Build the target table column by column
    arrays = {
        "rsid": raw.column("RSID").cast(pa.int64()),
        "chrom": pa.array([chrom] * n, type=pa.string()),
        "pos_37": raw.column("POS_37"),
        "pos_38": raw.column("POS_38"),
        "ref_37": raw.column("REF_37"),
        "ref_38": raw.column("REF_38"),
        "alt_37": raw.column("ALT_37"),
        "alt_38": raw.column("ALT_38"),
    }

    return pa.Table.from_arrays(
        [arrays[col] for col in SCHEMA.names], schema=SCHEMA
    )


# ---------------------------------------------------------------------------
# Iceberg ingest
# ---------------------------------------------------------------------------


def _pa_to_iceberg_schema(pa_schema: pa.Schema):
    from pyiceberg.schema import Schema
    from pyiceberg.types import (
        IntegerType,
        LongType,
        NestedField,
        StringType,
    )

    _type_map = {
        pa.string(): StringType(),
        pa.int32(): IntegerType(),
        pa.int64(): LongType(),
    }

    fields = []
    for i, field in enumerate(pa_schema):
        iceberg_type = _type_map.get(field.type, StringType())
        fields.append(NestedField(i + 1, field.name, iceberg_type, required=False))
    return Schema(*fields)


def create_or_load_table(catalog, full_table: str, pa_schema: pa.Schema):
    iceberg_schema = _pa_to_iceberg_schema(pa_schema)
    if catalog.table_exists(full_table):
        print(f"  Loading existing table: {full_table}")
        return catalog.load_table(full_table)
    print(f"  Creating table: {full_table}")
    return catalog.create_table(full_table, schema=iceberg_schema)


def sink_dbsnp(
    data_dir: Path,
    namespace: str,
    mode: str,
    chroms: list[str] | None,
    skip_chroms: set[str] | None,
) -> None:
    """Ingest per-chromosome dbSNP155 parquets into Iceberg."""
    chroms = chroms or CHROMOSOMES
    skip_chroms = skip_chroms or set()

    print("=== Connecting to Iceberg ===")
    catalog = get_catalog()
    print("  Catalog:", catalog.name)
    catalog.create_namespace_if_not_exists(namespace=namespace)

    full = f"{namespace}.{TABLE_NAME}"
    tbl = create_or_load_table(catalog, full, SCHEMA)
    print(f"  Table: {full}")

    total_rows = 0
    t_start = time.time()

    for chrom in chroms:
        if chrom in skip_chroms:
            print(f"  [skip] chr{chrom}")
            continue

        t0 = time.time()
        print(f"  [chr{chrom}] reading parquet…")
        arrow_tbl = load_chrom_parquet(data_dir, chrom)
        n = arrow_tbl.num_rows
        print(f"  [chr{chrom}] {n:,} rows, appending to Iceberg…")

        tbl.append(arrow_tbl)
        total_rows += n
        elapsed = time.time() - t0
        print(f"  [chr{chrom}] done ({elapsed:.1f}s, cumulative {total_rows:,} rows)")

    total_elapsed = time.time() - t_start
    print(f"\n=== Ingest complete: {total_rows:,} rows in {total_elapsed:.0f}s ===")


# ---------------------------------------------------------------------------
# OSS archival
# ---------------------------------------------------------------------------


def archive_tar_to_oss(tar_path: Path) -> None:
    """Upload the dbSNP155 tar archive to OSS via rclone."""
    import os
    import subprocess

    print(f"=== Archiving {tar_path.name} to {ARCHIVE_REMOTE} ===")
    rclone = os.environ.get("RCLONE", "rclone")
    try:
        subprocess.run([rclone, "version"], capture_output=True, check=True)
    except (FileNotFoundError, subprocess.CalledProcessError):
        print("  [warn] rclone not found — skipping archive")
        return

    dest = f"{ARCHIVE_REMOTE}/{tar_path.name}"
    print(f"  {tar_path} ({tar_path.stat().st_size / 1e9:.1f} GB) → {dest}")
    subprocess.run([rclone, "copyto", str(tar_path), dest, "-P"], check=True)
    print(f"  done")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Ingest dbSNP155 variant data into Iceberg"
    )
    parser.add_argument(
        "--data-dir",
        type=Path,
        required=True,
        help="Directory with CHR=*/part-0.parquet files",
    )
    parser.add_argument(
        "--namespace",
        default=NAMESPACE,
        help=f"Iceberg namespace (default: {NAMESPACE})",
    )
    parser.add_argument(
        "--mode",
        choices=["append", "overwrite"],
        default="append",
        help="Write mode (default: append — overwrite not supported per-chrom)",
    )
    parser.add_argument(
        "--chrom",
        type=str,
        nargs="+",
        default=None,
        help="Specific chromosome(s) to ingest (default: all)",
    )
    parser.add_argument(
        "--skip-chrom",
        type=str,
        default="",
        help="Comma-separated chromosomes to skip",
    )
    parser.add_argument(
        "--archive-tar",
        type=Path,
        default=None,
        help="Path to dbSNP155_v0.9.tar to archive to OSS",
    )
    args = parser.parse_args()

    skip_set = set(args.skip_chrom.split(",")) if args.skip_chrom else None

    sink_dbsnp(
        data_dir=args.data_dir.resolve(),
        namespace=args.namespace,
        mode=args.mode,
        chroms=args.chrom,
        skip_chroms=skip_set,
    )

    if args.archive_tar:
        archive_tar_to_oss(args.archive_tar.resolve())

    print("\n=== Done ===")


if __name__ == "__main__":
    main()
