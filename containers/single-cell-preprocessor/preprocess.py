#!/usr/bin/env python3
"""Validate and ingest 10x-style MatrixMarket inputs into AnnData."""

from __future__ import annotations

import gzip
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import TextIO

import anndata as ad
import pandas as pd


MATRIX_ENV = "AUTONOMICS_INPUT0"
BARCODES_ENV = "AUTONOMICS_INPUT1"
FEATURES_ENV = "AUTONOMICS_INPUT2"
METADATA_ENV = "AUTONOMICS_INPUT3"
REPORT_ENV = "AUTONOMICS_OUTPUT0"
H5AD_ENV = "AUTONOMICS_OUTPUT1"


class ContractError(RuntimeError):
    pass


@dataclass(frozen=True)
class MatrixHeader:
    field: str
    rows: int
    columns: int
    nonzero: int


def required_path(name: str) -> Path:
    value = os.environ.get(name)
    if not value:
        raise ContractError(f"missing required environment variable: {name}")
    return Path(value)


def open_text(path: Path) -> TextIO:
    if path.name.lower().endswith(".gz"):
        return gzip.open(path, "rt", encoding="ascii")
    return path.open("rt", encoding="ascii")


def parse_matrix_header(path: Path) -> MatrixHeader:
    with open_text(path) as handle:
        banner = handle.readline().strip()
        if not banner.startswith("%%MatrixMarket"):
            raise ContractError(f"{path.name}: not a MatrixMarket file")
        parts = banner.split()
        if len(parts) != 5:
            raise ContractError(f"{path.name}: malformed MatrixMarket banner")
        if parts[1] != "matrix" or parts[2] != "coordinate":
            raise ContractError(
                f"{path.name}: expected MatrixMarket coordinate matrix, got "
                f"{parts[1]}/{parts[2]}"
            )
        if parts[3] not in {"integer", "real", "double"}:
            raise ContractError(
                f"{path.name}: unsupported MatrixMarket field `{parts[3]}`"
            )
        if parts[4] != "general":
            raise ContractError(
                f"{path.name}: unsupported MatrixMarket symmetry `{parts[4]}`"
            )
        dimensions = handle.readline().split()
        if len(dimensions) != 3:
            raise ContractError(f"{path.name}: malformed MatrixMarket dimensions")
        try:
            values = tuple(int(value) for value in dimensions)
        except ValueError as error:
            raise ContractError(f"{path.name}: non-integer dimensions") from error
        if any(value < 0 for value in values):
            raise ContractError(f"{path.name}: negative dimensions")
        return MatrixHeader(field=parts[3], rows=values[0], columns=values[1], nonzero=values[2])


def read_barcodes(path: Path) -> pd.DataFrame:
    frame = pd.read_csv(
        path,
        sep="\t",
        header=None,
        names=["cell_barcode"],
        dtype={"cell_barcode": str},
    )
    if frame.empty:
        raise ContractError(f"{path.name}: barcode file is empty")
    if frame["cell_barcode"].isna().any():
        raise ContractError(f"{path.name}: barcode file contains a missing value")
    if frame["cell_barcode"].str.len().eq(0).any():
        raise ContractError(f"{path.name}: barcode file contains an empty value")
    if frame["cell_barcode"].duplicated().any():
        raise ContractError(f"{path.name}: barcode file contains duplicate cell IDs")
    return frame


def read_features(path: Path) -> pd.DataFrame:
    frame = pd.read_csv(
        path,
        sep="\t",
        header=None,
        names=["gene_id", "gene_symbol", "feature_type"][:3],
        dtype=str,
    )
    if frame.empty:
        raise ContractError(f"{path.name}: feature file is empty")
    if frame.isna().any().any():
        raise ContractError(f"{path.name}: feature file contains a missing value")
    if frame["gene_id"].str.len().eq(0).any() or frame["gene_symbol"].str.len().eq(0).any():
        raise ContractError(f"{path.name}: feature file contains an empty identifier")
    if frame["gene_id"].duplicated().any():
        raise ContractError(f"{path.name}: feature file contains duplicate gene IDs")
    return frame


def read_metadata(path: Path, barcodes: pd.DataFrame) -> pd.DataFrame:
    frame = pd.read_csv(path, sep=r"\s+", engine="python", index_col=0, dtype=str)
    if frame.empty:
        raise ContractError(f"{path.name}: metadata file has no data rows")
    metadata_ids = frame.index.astype(str)
    if metadata_ids.has_duplicates:
        raise ContractError(f"{path.name}: metadata file contains duplicate cell IDs")
    barcode_ids = barcodes["cell_barcode"]
    if not set(metadata_ids) == set(barcode_ids):
        missing_from_metadata = len(set(barcode_ids) - set(metadata_ids))
        missing_from_barcodes = len(set(metadata_ids) - set(barcode_ids))
        raise ContractError(
            "cell IDs do not align: "
            f"{missing_from_metadata} missing from metadata, "
            f"{missing_from_barcodes} absent from barcodes"
        )
    return frame.loc[barcode_ids].copy()


def make_adata(metadata: pd.DataFrame, features: pd.DataFrame) -> ad.AnnData:
    var = features.set_index("gene_symbol", drop=False)
    adata = ad.AnnData(obs=metadata, var=var)
    if adata.var_names.has_duplicates:
        adata.var_names_make_unique()
    return adata


def write_report(path: Path, report: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, sort_keys=True, allow_nan=False)
        handle.write("\n")


def run() -> None:
    matrix_path = required_path(MATRIX_ENV)
    barcodes_path = required_path(BARCODES_ENV)
    features_path = required_path(FEATURES_ENV)
    metadata_path = required_path(METADATA_ENV)
    report_path = required_path(REPORT_ENV)
    h5ad_path = required_path(H5AD_ENV)
    operation = os.environ.get("AUTONOMICS_SINGLE_CELL_OPERATION", "inspect").lower()
    if operation not in {"inspect", "ingest"}:
        raise ContractError(f"unsupported operation `{operation}`")
    try:
        min_genes = int(os.environ.get("AUTONOMICS_SINGLE_CELL_MIN_GENES", "0"))
        min_cells = int(os.environ.get("AUTONOMICS_SINGLE_CELL_MIN_CELLS", "0"))
        normalize = os.environ.get(
            "AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL", "false"
        ).lower() in {"1", "true", "yes"}
    except ValueError as error:
        raise ContractError("min_genes and min_cells must be integers") from error
    if min_genes < 0 or min_cells < 0:
        raise ContractError("min_genes and min_cells cannot be negative")

    header = parse_matrix_header(matrix_path)
    barcodes = read_barcodes(barcodes_path)
    features = read_features(features_path)
    metadata = read_metadata(metadata_path, barcodes)
    expected_cells = header.columns
    if len(barcodes) != expected_cells:
        raise ContractError(
            f"matrix declares {expected_cells} cells, barcode file has {len(barcodes)}"
        )
    if header.rows != len(features):
        raise ContractError(
            f"matrix declares {header.rows} genes, feature file has {len(features)}"
        )

    adata = make_adata(metadata, features)
    report: dict[str, object] = {
        "schema_version": "1.0",
        "operation": operation,
        "matrix": {
            "format": "matrix_market_coordinate",
            "field": header.field,
            "genes": header.rows,
            "cells": header.columns,
            "nonzero": header.nonzero,
            "bytes": matrix_path.stat().st_size,
            "gzip": matrix_path.name.lower().endswith(".gz"),
            "expression_loaded": False,
        },
        "alignment": {
            "cells": len(barcodes),
            "genes": len(features),
            "metadata_rows": len(metadata),
            "cell_ids_aligned": True,
            "cell_id_order_preserved": metadata.index.equals(barcodes["cell_barcode"]),
        },
        "qc": {
            "min_genes": min_genes,
            "min_cells": min_cells,
            "normalize_total": normalize,
        },
    }

    if operation == "ingest":
        import scanpy as sc

        raw_matrix = sc.read_mtx(matrix_path)
        if raw_matrix.shape != (header.rows, header.columns):
            raise ContractError(
                f"loaded matrix shape {raw_matrix.shape} does not match header "
                f"({header.rows}, {header.columns})"
            )
        adata = raw_matrix.T.copy()
        adata.obs_names = pd.Index(barcodes["cell_barcode"], name="cell_barcode")
        adata.var_names = pd.Index(features["gene_symbol"], name="gene_symbol")
        if adata.var_names.has_duplicates:
            adata.var_names_make_unique()
        adata.obs = metadata
        if min_genes > 0:
            sc.pp.filter_cells(adata, min_genes=min_genes)
        if min_cells > 0:
            sc.pp.filter_genes(adata, min_cells=min_cells)
        if normalize:
            adata.layers["counts"] = adata.X.copy()
            sc.pp.normalize_total(adata)
            sc.pp.log1p(adata)
        report["matrix"]["expression_loaded"] = True
        report["matrix"]["output_genes"] = adata.n_vars
        report["matrix"]["output_cells"] = adata.n_obs

    h5ad_path.parent.mkdir(parents=True, exist_ok=True)
    adata.write_h5ad(h5ad_path, compression="lzf")
    write_report(report_path, report)


def main() -> int:
    try:
        run()
    except ContractError as error:
        print(f"single_cell_preprocessor: {error}", file=sys.stderr)
        return 2
    except (OSError, ValueError) as error:
        print(f"single_cell_preprocessor: {error}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
