"""Ingest baselineLD v2.2 LD-score panel files into Iceberg.

Reads the per-chromosome ``.l2.ldscore.gz`` (multi-annotation, 97 columns) and
``.l2.M_5_50`` files from the 1000G reference bundle, builds a wide PyArrow
Table (one row per SNP, one column per annotation), and appends it to an
Iceberg table in the ``ld_score`` namespace.

The weight LD scores (``--w-ld``) are also read and joined into the same table
as a ``w_ld`` column, so a downstream S-LDSC node gets ref_ld + w_ld in a
single scan.

Usage
-----
    python main.py \
        --ref-ld-chr ../../reference/ldsc_data/baselineLD. \
        --w-ld-chr ../../reference/ldsc_data/weights.hm3_noMHC. \
        --table-name baselineLD_v2_2_eur \
        --mode overwrite
"""

from __future__ import annotations

import argparse
import gzip
import os
import sys

import numpy as np
import pandas as pd
import pyarrow as pa

from datalake import get_catalog

# ---------------------------------------------------------------------------
# File parsing
# ---------------------------------------------------------------------------


def read_ldscore_chr(prefix: str) -> pd.DataFrame:
    """Read per-chromosome ``{prefix}{1..22}.l2.ldscore.gz`` and concatenate.

    Returns a DataFrame with columns ``CHR, SNP, BP`` + N annotation columns
    (column names end with ``L2``).
    """
    frames = []
    for chrom in range(1, 23):
        path = f"{prefix}{chrom}.l2.ldscore.gz"
        if not os.path.exists(path):
            raise FileNotFoundError(f"Missing chromosome file: {path}")
        df = pd.read_csv(path, sep=r"\s+")
        frames.append(df)
        print(f"  {path}: {len(df)} SNPs, {len(df.columns)} cols")

    combined = pd.concat(frames, ignore_index=True)
    print(f"Total: {len(combined)} SNPs across {len(frames)} chromosomes")
    return combined


def read_m_5_50_chr(prefix: str) -> pd.DataFrame:
    """Read per-chromosome ``{prefix}{1..22}.l2.M_5_50`` and sum across chrs.

    Returns a DataFrame with columns ``annotation`` and ``m_5_50`` (length =
    number of annotations).
    """
    # Read chromosome 1 to get annotation names
    with open(f"{prefix}1.l2.M_5_50") as f:
        values = list(map(float, f.readline().split()))

    # Sum remaining chromosomes
    for chrom in range(2, 23):
        path = f"{prefix}{chrom}.l2.M_5_50"
        if not os.path.exists(path):
            continue
        with open(path) as f:
            chr_values = list(map(float, f.readline().split()))
        values = [a + b for a, b in zip(values, chr_values)]

    # Read annotation names from the ldscore header
    with gzip.open(f"{prefix}1.l2.ldscore.gz", "rt") as f:
        header = f.readline().split()
    annot_names = [h for h in header if h not in ("CHR", "SNP", "BP")]

    assert len(annot_names) == len(values), (
        f"M_5_50 has {len(values)} values but ldscore has {len(annot_names)} "
        f"annotation columns"
    )

    return pd.DataFrame({"annotation": annot_names, "m_5_50": values})


def read_weights_chr(prefix: str) -> pd.DataFrame:
    """Read per-chromosome weight LD scores (single ``L2`` column).

    Returns a DataFrame with columns ``SNP, w_ld``.
    """
    frames = []
    for chrom in range(1, 23):
        path = f"{prefix}{chrom}.l2.ldscore.gz"
        if not os.path.exists(path):
            raise FileNotFoundError(f"Missing weight file: {path}")
        df = pd.read_csv(path, sep=r"\s+")
        frames.append(df[["SNP", "L2"]])
    combined = pd.concat(frames, ignore_index=True).rename(columns={"L2": "w_ld"})
    print(f"Weight LD: {len(combined)} SNPs")
    return combined


# ---------------------------------------------------------------------------
# PyArrow / Iceberg schema
# ---------------------------------------------------------------------------


def build_arrow_table(
    ldscore: pd.DataFrame,
    weights: pd.DataFrame,
) -> pa.Table:
    """Build a wide PyArrow Table with nested ``locus`` struct + annotation cols.

    Schema:
        locus    struct { contig: string, position: int32 }
        rsid     string
        <97 annotation columns>   float64
        w_ld     float64
    """
    # Join ldscore with weights on SNP
    merged = ldscore.merge(weights, on="SNP", how="inner")

    annot_cols = [c for c in ldscore.columns if c not in ("CHR", "SNP", "BP")]

    # Build locus struct
    locus = pa.StructArray.from_arrays(
        [
            pa.array(merged["CHR"].astype(str), type=pa.string()),
            pa.array(merged["BP"].astype("int32"), type=pa.int32()),
        ],
        names=["contig", "position"],
    )

    columns = [
        pa.field(
            "locus",
            pa.struct(
                [
                    pa.field("contig", pa.string()),
                    pa.field("position", pa.int32()),
                ]
            ),
        ),
        pa.field("rsid", pa.string()),
    ]
    arrays: list[pa.Array | pa.StructArray] = [
        locus,
        pa.array(merged["SNP"], type=pa.string()),
    ]

    for col in annot_cols:
        columns.append(pa.field(col, pa.float64()))
        arrays.append(pa.array(merged[col], type=pa.float64()))

    columns.append(pa.field("w_ld", pa.float64()))
    arrays.append(pa.array(merged["w_ld"], type=pa.float64()))

    schema = pa.schema(columns)
    return pa.Table.from_arrays(arrays, schema=schema)


def build_m_table(m_df: pd.DataFrame) -> pa.Table:
    """Build the companion M_5_50 table (annotation → m_5_50)."""
    return pa.Table.from_pandas(m_df, preserve_index=False)


# ---------------------------------------------------------------------------
# Iceberg ingest
# ---------------------------------------------------------------------------


def create_or_load_table(catalog, full_table: str, schema):
    if catalog.table_exists(full_table):
        existing = catalog.load_table(full_table)
        if existing.schema() == schema:
            print(f"Loading existing table: {full_table}")
            return existing
        print(f"Schema mismatch, dropping and recreating: {full_table}")
        catalog.drop_table(full_table)
    print(f"Creating table: {full_table}")
    return catalog.create_table(full_table, schema=schema)


def sink_ldscore(
    ref_ld_chr: str,
    w_ld_chr: str,
    table_name: str,
    namespace: str,
    mode: str,
) -> None:
    # 1. Parse files
    print("=== Reading reference LD scores ===")
    ldscore = read_ldscore_chr(ref_ld_chr)

    print("=== Reading M_5_50 ===")
    m_df = read_m_5_50_chr(ref_ld_chr)

    print("=== Reading weight LD scores ===")
    weights = read_weights_chr(w_ld_chr)

    # 2. Build Arrow tables
    print("=== Building Arrow table ===")
    arrow_ldscore = build_arrow_table(ldscore, weights)
    print(
        f"LD-score table: {arrow_ldscore.num_rows} rows x "
        f"{len(arrow_ldscore.column_names)} cols"
    )
    arrow_m = build_m_table(m_df)
    print(f"M_5_50 table: {arrow_m.num_rows} rows")

    # 3. Ingest to Iceberg
    print("=== Connecting to Iceberg ===")
    catalog = get_catalog()
    print("Catalog:", catalog.name)
    catalog.create_namespace_if_not_exists(namespace=namespace)

    # LD-score panel table
    full_ld = f"{namespace}.{table_name}"
    # Convert pa.schema to Iceberg schema
    from pyiceberg.schema import Schema
    from pyiceberg.types import (
        NestedField,
        StringType,
        DoubleType,
        IntegerType,
        StructType,
    )

    annot_cols = [c for c in ldscore.columns if c not in ("CHR", "SNP", "BP")]
    field_id = [1]  # mutable counter
    iceberg_fields = []

    iceberg_fields.append(
        NestedField(
            field_id[0],
            "locus",
            StructType(
                NestedField(field_id[0] + 1, "contig", StringType(), required=False),
                NestedField(field_id[0] + 2, "position", IntegerType(), required=False),
            ),
            required=False,
        )
    )
    field_id[0] += 2
    iceberg_fields.append(
        NestedField(field_id[0], "rsid", StringType(), required=False)
    )
    field_id[0] += 1
    for col in annot_cols:
        iceberg_fields.append(
            NestedField(field_id[0], col, DoubleType(), required=False)
        )
        field_id[0] += 1
    iceberg_fields.append(
        NestedField(field_id[0], "w_ld", DoubleType(), required=False)
    )
    field_id[0] += 1

    iceberg_schema = Schema(*iceberg_fields)

    iceberg_table = create_or_load_table(catalog, full_ld, iceberg_schema)
    if mode == "overwrite" and iceberg_table.snapshots():
        iceberg_table.overwrite(arrow_ldscore)
    else:
        iceberg_table.append(arrow_ldscore)
    print(f"Ingested {arrow_ldscore.num_rows} rows → {full_ld}")

    # M_5_50 companion table
    full_m = f"{namespace}.{table_name}_m"
    m_schema = Schema(
        NestedField(1, "annotation", StringType(), required=False),
        NestedField(2, "m_5_50", DoubleType(), required=False),
    )
    m_table = create_or_load_table(catalog, full_m, m_schema)
    if mode == "overwrite" and m_table.snapshots():
        m_table.overwrite(arrow_m)
    else:
        m_table.append(arrow_m)
    print(f"Ingested {arrow_m.num_rows} rows → {full_m}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Ingest baselineLD LD-score panel into Iceberg"
    )
    parser.add_argument(
        "--ref-ld-chr",
        required=True,
        help="Prefix for per-chromosome baselineLD files (e.g. 'data/baselineLD.')",
    )
    parser.add_argument(
        "--w-ld-chr",
        required=True,
        help="Prefix for per-chromosome weight LD files (e.g. 'data/weights.')",
    )
    parser.add_argument(
        "--table-name",
        default="baselineLD_v2_2_eur",
        help="Iceberg table name",
    )
    parser.add_argument(
        "--namespace",
        default="ld_score",
        help="Iceberg namespace",
    )
    parser.add_argument(
        "--mode",
        choices=["append", "overwrite"],
        default="overwrite",
        help="Write mode",
    )
    args = parser.parse_args()

    sink_ldscore(
        ref_ld_chr=args.ref_ld_chr,
        w_ld_chr=args.w_ld_chr,
        table_name=args.table_name,
        namespace=args.namespace,
        mode=args.mode,
    )


if __name__ == "__main__":
    main()
