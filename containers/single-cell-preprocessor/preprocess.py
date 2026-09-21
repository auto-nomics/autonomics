#!/usr/bin/env python3
"""Validate and ingest 10x-style MatrixMarket inputs into AnnData."""

from __future__ import annotations

import gzip
import hashlib
import json
import os
import platform
import sys
import warnings
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


def _check_exists(path: Path, label: str) -> None:
    if not path.exists():
        raise ContractError(f"{label} file does not exist: {path}")
    if not path.is_file():
        raise ContractError(f"{label} is not a regular file: {path}")
    if path.stat().st_size == 0:
        raise ContractError(f"{label} file is empty (0 bytes): {path}")


def _file_probe(path: Path) -> dict[str, object]:
    """Collect diagnostic metadata for error messages."""
    info: dict[str, object] = {"bytes": path.stat().st_size}
    raw = path.read_bytes()[:512]
    info["preview"] = raw.decode("utf-8", errors="replace").splitlines()[:3]
    info["line_count"] = raw.count(b"\n")
    return info


def _wrap_parse_error(path: Path, error: Exception, phase: str) -> ContractError:
    probe = _file_probe(path)
    return ContractError(
        f"{path.name}: {phase} failed ({type(error).__name__}): {error}; "
        f"file probe: {json.dumps(probe, ensure_ascii=False)}"
    )


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


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
        dimensions: list[str] | None = None
        for line in handle:
            if line.lstrip().startswith("%"):
                continue
            dimensions = line.split()
            break
        if dimensions is None:
            raise ContractError(f"{path.name}: missing MatrixMarket dimensions")
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
    try:
        frame = pd.read_csv(
            path,
            sep="\t",
            header=None,
            names=["cell_barcode"],
            dtype={"cell_barcode": str},
            keep_default_na=False,
        )
    except (pd.errors.EmptyDataError, pd.errors.ParserError, UnicodeDecodeError) as e:
        raise _wrap_parse_error(path, e, "barcode parse") from e
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
    try:
        frame = pd.read_csv(path, sep="\t", header=None, dtype=str, keep_default_na=False)
    except (pd.errors.EmptyDataError, pd.errors.ParserError, UnicodeDecodeError) as e:
        raise _wrap_parse_error(path, e, "feature parse") from e
    if frame.shape[1] not in {2, 3}:
        raise ContractError(
            f"{path.name}: feature file must have two or three columns, "
            f"got {frame.shape[1]}"
        )
    frame.columns = ["gene_id", "gene_symbol", "feature_type"][: frame.shape[1]]
    if frame.empty:
        raise ContractError(f"{path.name}: feature file is empty")
    if frame.isna().any().any():
        raise ContractError(f"{path.name}: feature file contains a missing value")
    if frame["gene_id"].str.len().eq(0).any() or frame["gene_symbol"].str.len().eq(0).any():
        raise ContractError(f"{path.name}: feature file contains an empty identifier")
    if frame["gene_id"].duplicated().any():
        raise ContractError(f"{path.name}: feature file contains duplicate gene IDs")
    return frame


def _detect_separator(path: Path) -> str:
    """Return a pd.read_csv sep value based on the metadata_separator env var."""
    mode = os.environ.get("AUTONOMICS_SINGLE_CELL_METADATA_SEP", "auto").lower()
    if mode == "tab":
        return "\t"
    if mode == "comma":
        return ","
    # auto: look at the first non-blank line
    with open_text(path) as handle:
        for line in handle:
            if line.strip():
                return "\t" if "\t" in line else ","
    return "\t"


def read_metadata(path: Path, barcodes: pd.DataFrame) -> tuple[pd.DataFrame, bool]:
    sep = _detect_separator(path)
    try:
        frame = pd.read_csv(
            path,
            sep=sep,
            engine="python",
            index_col=0,
            dtype=str,
            keep_default_na=False,
        )
    except (pd.errors.EmptyDataError, pd.errors.ParserError, UnicodeDecodeError) as e:
        raise _wrap_parse_error(path, e, "metadata parse") from e
    if frame.empty or frame.shape[1] == 0:
        probe = _file_probe(path)
        raise ContractError(
            f"{path.name}: metadata parsed to 0 rows x {frame.shape[1]} cols; "
            f"file probe: {json.dumps(probe, ensure_ascii=False)}"
        )
    frame.index = frame.index.astype(str)
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
    order_preserved = metadata_ids.equals(pd.Index(barcode_ids))
    return frame.loc[barcode_ids].copy(), order_preserved


def make_adata(metadata: pd.DataFrame, features: pd.DataFrame) -> ad.AnnData:
    var = features.set_index("gene_symbol")
    adata = ad.AnnData(obs=metadata, var=var)
    if adata.var_names.has_duplicates:
        adata.var_names_make_unique()
    return adata


def write_report(path: Path, report: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, sort_keys=True, allow_nan=False)
        handle.write("\n")


def parse_nonnegative_int(name: str, default: str = "0") -> int:
    value = os.environ.get(name, default)
    try:
        parsed = int(value)
    except ValueError as error:
        raise ContractError(f"{name} must be an integer") from error
    if parsed < 0:
        raise ContractError(f"{name} cannot be negative")
    return parsed


def parse_bool(name: str, default: str = "false") -> bool:
    value = os.environ.get(name, default).lower()
    if value in {"1", "true", "yes"}:
        return True
    if value in {"0", "false", "no"}:
        return False
    raise ContractError(f"{name} must be a boolean")


def _run() -> tuple[dict[str, object], Path, Path]:
    matrix_path = required_path(MATRIX_ENV)
    barcodes_path = required_path(BARCODES_ENV)
    features_path = required_path(FEATURES_ENV)
    metadata_path = required_path(METADATA_ENV)
    report_path = required_path(REPORT_ENV)
    h5ad_path = required_path(H5AD_ENV)
    _check_exists(matrix_path, "matrix")
    _check_exists(barcodes_path, "barcode")
    _check_exists(features_path, "feature")
    _check_exists(metadata_path, "metadata")
    operation = os.environ.get("AUTONOMICS_SINGLE_CELL_OPERATION", "inspect").lower()
    if operation not in {"inspect", "ingest"}:
        raise ContractError(f"unsupported operation `{operation}`")
    min_genes = parse_nonnegative_int("AUTONOMICS_SINGLE_CELL_MIN_GENES")
    min_cells = parse_nonnegative_int("AUTONOMICS_SINGLE_CELL_MIN_CELLS")
    normalize = parse_bool("AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL")
    if operation == "inspect" and (min_genes > 0 or min_cells > 0 or normalize):
        raise ContractError(
            "filtering and normalization require operation `ingest`; "
            "`inspect` is header-only"
        )

    header = parse_matrix_header(matrix_path)
    barcodes = read_barcodes(barcodes_path)
    features = read_features(features_path)
    metadata, metadata_order_preserved = read_metadata(metadata_path, barcodes)
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
        "tool_versions": {
            "python": platform.python_version(),
            "pandas": pd.__version__,
            "anndata": ad.__version__,
        },
        "inputs": [
            {"name": p.name, "sha256": _sha256(p), "bytes": p.stat().st_size}
            for p in [matrix_path, barcodes_path, features_path, metadata_path]
        ],
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
            "cell_id_order_preserved": metadata_order_preserved,
        },
        "qc": {
            "min_genes": min_genes,
            "min_cells": min_cells,
            "normalize_total": normalize,
        },
    }

    if operation == "ingest":
        import scanpy as sc

        report["tool_versions"]["scanpy"] = sc.__version__  # type: ignore[attr-defined]

        raw_matrix = sc.read_mtx(matrix_path)
        if raw_matrix.shape != (header.rows, header.columns):
            raise ContractError(
                f"loaded matrix shape {raw_matrix.shape} does not match header "
                f"({header.rows}, {header.columns})"
            )
        adata = raw_matrix.T.copy()
        adata.obs_names = pd.Index(barcodes["cell_barcode"], name="cell_barcode")
        adata.var = features.set_index("gene_symbol")
        adata.var.index.name = "gene_symbol"
        if adata.var_names.has_duplicates:
            adata.var_names_make_unique()
        adata.obs = metadata
        if min_genes > 0:
            sc.pp.filter_cells(adata, min_genes=min_genes)
        if min_cells > 0:
            sc.pp.filter_genes(adata, min_cells=min_cells)
        if normalize:
            adata.layers["counts"] = adata.X.copy()
            sc.pp.normalize_total(adata, target_sum=1e4)
            sc.pp.log1p(adata)
        report["matrix"]["expression_loaded"] = True
        report["matrix"]["output_genes"] = adata.n_vars
        report["matrix"]["output_cells"] = adata.n_obs

    h5ad_path.parent.mkdir(parents=True, exist_ok=True)
    adata.write_h5ad(h5ad_path, compression="lzf")
    return report, report_path, h5ad_path


def run() -> None:
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        report, report_path, h5ad_path = _run()
    report["warnings"] = [
        f"{w.category.__name__}: {w.message}" for w in caught
    ]
    report["outputs"] = [
        {"name": p.name, "sha256": _sha256(p), "bytes": p.stat().st_size}
        for p in [report_path, h5ad_path] if p.is_file()
    ]
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
