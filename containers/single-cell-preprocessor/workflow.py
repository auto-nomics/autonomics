#!/usr/bin/env python3
"""File-to-file H5AD operations for the single-cell DAG ecosystem."""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any

import anndata as ad
import numpy as np
import pandas as pd
import scipy.sparse as sp


H5AD_INPUT = "AUTONOMICS_INPUT0"
MODEL_INPUT = "AUTONOMICS_INPUT1"
PARQUET_INPUT = "AUTONOMICS_INPUT1"
PARAMS_PATH = "AUTONOMICS_SINGLE_CELL_PARAMS"
WORKFLOW_ENV = "AUTONOMICS_SINGLE_CELL_WORKFLOW"
H5AD_OUTPUT = "AUTONOMICS_OUTPUT0"
REPORT_OUTPUT = "AUTONOMICS_OUTPUT1"
PARQUET_OUTPUT = "AUTONOMICS_OUTPUT0"


class ContractError(RuntimeError):
    pass


def required_path(name: str) -> Path:
    value = os.environ.get(name)
    if not value:
        raise ContractError(f"missing required environment variable: {name}")
    path = Path(value)
    if not path.is_file():
        raise ContractError(f"{name} does not name a readable file: {path}")
    return path


def required_output(name: str) -> Path:
    value = os.environ.get(name)
    if not value:
        raise ContractError(f"missing required environment variable: {name}")
    return Path(value)


def load_params() -> dict[str, Any]:
    path = required_path(PARAMS_PATH)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot read parameters `{path}`: {error}") from error
    if not isinstance(value, dict):
        raise ContractError("parameters must be a JSON object")
    return value


def require_str(params: dict[str, Any], name: str, default: str | None = None) -> str:
    value = params.get(name, default)
    if not isinstance(value, str) or not value.strip():
        raise ContractError(f"parameter `{name}` must be a nonempty string")
    return value


def opt_bool(params: dict[str, Any], name: str, default: bool) -> bool:
    value = params.get(name, default)
    if not isinstance(value, bool):
        raise ContractError(f"parameter `{name}` must be a boolean")
    return value


def ensure_h5ad_signature(path: Path) -> None:
    signature = b"\x89HDF\r\n\x1a\n"
    try:
        with path.open("rb") as handle:
            actual = handle.read(len(signature))
    except OSError as error:
        raise ContractError(f"cannot read H5AD `{path}`: {error}") from error
    if actual != signature:
        raise ContractError(f"`{path.name}` is not an HDF5-backed H5AD file")


def read_h5ad(path: Path, backed: str | None = None) -> ad.AnnData:
    ensure_h5ad_signature(path)
    adata = ad.read_h5ad(path, backed=backed)
    if adata.obs_names.has_duplicates:
        raise ContractError("H5AD contract violation: obs_names are not unique")
    if adata.var_names.has_duplicates:
        raise ContractError("H5AD contract violation: var_names are not unique")
    return adata


def write_h5ad(adata: ad.AnnData, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if sp.issparse(adata.X) and not sp.isspmatrix_csr(adata.X):
        adata.X = sp.csr_matrix(adata.X)
    adata.write_h5ad(path, compression="lzf")


def write_json(value: dict[str, Any], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, sort_keys=True, allow_nan=False)
        handle.write("\n")


def write_parquet(frame: pd.DataFrame, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    frame.to_parquet(path, index=False, engine="pyarrow")


def sparse_or_dense_axis_sum(value: Any, axis: int) -> np.ndarray:
    result = np.asarray(value.sum(axis=axis)).ravel()
    return np.asarray(result, dtype=float)


def row_counts(value: Any) -> np.ndarray:
    if sp.issparse(value):
        return np.asarray(value.getnnz(axis=1), dtype=float)
    return np.count_nonzero(np.asarray(value), axis=1).astype(float)


def column_counts(value: Any) -> np.ndarray:
    if sp.issparse(value):
        return np.asarray(value.getnnz(axis=0), dtype=float)
    return np.count_nonzero(np.asarray(value), axis=0).astype(float)


def percentile_summary(values: np.ndarray) -> dict[str, float]:
    clean = values[np.isfinite(values)]
    if clean.size == 0:
        return {name: 0.0 for name in ("min", "p25", "median", "p75", "max")}
    return {
        "min": float(np.min(clean)),
        "p25": float(np.percentile(clean, 25)),
        "median": float(np.percentile(clean, 50)),
        "p75": float(np.percentile(clean, 75)),
        "max": float(np.max(clean)),
    }


def qc_filter(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    input_path = required_path(H5AD_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    adata = read_h5ad(input_path)

    mt_pattern = re.compile(require_str(params, "mt_gene_pattern", "^MT-"), re.IGNORECASE)
    rb_pattern = re.compile(require_str(params, "rb_gene_pattern", "^RPL|^RPS"), re.IGNORECASE)
    var_names = pd.Index([str(value) for value in adata.var_names])
    mt_genes = var_names.str.contains(mt_pattern, regex=True)
    rb_genes = var_names.str.contains(rb_pattern, regex=True)
    total_counts = sparse_or_dense_axis_sum(adata.X, axis=1)
    n_genes = row_counts(adata.X)
    mt_counts = sparse_or_dense_axis_sum(adata.X[:, mt_genes], axis=1) if mt_genes.any() else np.zeros(adata.n_obs)
    rb_counts = sparse_or_dense_axis_sum(adata.X[:, rb_genes], axis=1) if rb_genes.any() else np.zeros(adata.n_obs)

    with np.errstate(divide="ignore", invalid="ignore"):
        pct_mt = np.divide(mt_counts, total_counts, out=np.zeros_like(total_counts), where=total_counts > 0) * 100
        pct_rb = np.divide(rb_counts, total_counts, out=np.zeros_like(total_counts), where=total_counts > 0) * 100
    adata.obs["n_genes_by_counts"] = n_genes
    adata.obs["total_counts"] = total_counts
    adata.obs["pct_counts_mt"] = pct_mt
    adata.obs["pct_counts_rb"] = pct_rb

    min_genes = int(params.get("min_genes", 0))
    max_genes = int(params.get("max_genes", 0))
    min_cells = int(params.get("min_cells", 0))
    max_cells = int(params.get("max_cells", 0))
    max_pct_mt = float(params.get("max_pct_mt", 100.0))
    max_pct_rb = float(params.get("max_pct_rb", 100.0))
    for name, value in (
        ("min_genes", min_genes),
        ("max_genes", max_genes),
        ("min_cells", min_cells),
        ("max_cells", max_cells),
    ):
        if value < 0:
            raise ContractError(f"parameter `{name}` cannot be negative")
    if max_pct_mt < 0 or max_pct_mt > 100 or max_pct_rb < 0 or max_pct_rb > 100:
        raise ContractError("percentage thresholds must be between 0 and 100")

    keep_cells = (
        (n_genes >= min_genes)
        & ((max_genes == 0) | (n_genes <= max_genes))
        & (pct_mt <= max_pct_mt)
        & (pct_rb <= max_pct_rb)
    )
    gene_counts = column_counts(adata.X)
    keep_genes = (gene_counts >= min_cells) & ((max_cells == 0) | (gene_counts <= max_cells))
    adata = adata[keep_cells, keep_genes].copy()
    if adata.n_obs == 0 or adata.n_vars == 0:
        raise ContractError("QC filters removed every cell or gene")

    effective_params = {
        "min_genes": min_genes,
        "max_genes": max_genes,
        "min_cells": min_cells,
        "max_cells": max_cells,
        "max_pct_mt": max_pct_mt,
        "max_pct_rb": max_pct_rb,
        "mt_gene_pattern": mt_pattern.pattern,
        "rb_gene_pattern": rb_pattern.pattern,
    }
    adata.uns["qc_params"] = effective_params
    report = {
        "schema_version": "1.0",
        "operation": "qc_filter",
        "input_cells": int(len(keep_cells)),
        "output_cells": int(adata.n_obs),
        "input_genes": int(len(keep_genes)),
        "output_genes": int(adata.n_vars),
        "filters": effective_params,
        "metrics": {
            "n_genes_by_counts": percentile_summary(n_genes),
            "total_counts": percentile_summary(total_counts),
            "pct_counts_mt": percentile_summary(pct_mt),
            "pct_counts_rb": percentile_summary(pct_rb),
        },
    }
    write_json(report, report_path)
    return adata, output_path, report_path


def pca_neighbors_umap_leiden(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    import scanpy as sc

    input_path = required_path(H5AD_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    adata = read_h5ad(input_path)

    n_pcs = int(params.get("n_pcs", 30))
    n_neighbors = int(params.get("n_neighbors", 15))
    n_top_genes = int(params.get("n_top_genes", 2000))
    resolution = float(params.get("resolution", 0.5))
    min_dist = float(params.get("min_dist", 0.5))
    random_state = int(params.get("random_state", 0))
    normalize = opt_bool(params, "normalize", True)
    log1p = opt_bool(params, "log1p", True)
    subset_hvg = opt_bool(params, "subset_hvg", False)
    scale = opt_bool(params, "scale", False)
    if min(n_pcs, n_neighbors, n_top_genes) <= 0 or resolution <= 0 or not 0 < min_dist < 1:
        raise ContractError("embedding parameters must be positive and min_dist must be in (0, 1)")
    if adata.n_obs <= n_neighbors:
        raise ContractError("n_neighbors must be smaller than the number of cells")
    if adata.n_vars <= 1:
        raise ContractError("at least two genes are required for embedding")

    pca_computed = False
    if "X_pca" not in adata.obsm:
        if normalize:
            if "counts" not in adata.layers:
                adata.layers["counts"] = adata.X.copy()
            sc.pp.normalize_total(adata, target_sum=1e4)
        if log1p:
            sc.pp.log1p(adata)
        max_hvg = max(1, min(n_top_genes, adata.n_vars))
        sc.pp.highly_variable_genes(
            adata,
            n_top_genes=max_hvg,
            flavor="seurat",
            subset=subset_hvg,
        )
        if scale:
            sc.pp.scale(adata, max_value=10)
            zero_center = True
        else:
            zero_center = False
        max_pcs = max(1, min(n_pcs, adata.n_vars - 1, adata.n_obs - 1))
        sc.tl.pca(
            adata,
            n_comps=max_pcs,
            zero_center=zero_center,
            svd_solver="arpack",
            random_state=random_state,
        )
        pca_computed = True
    else:
        available_pcs = int(adata.obsm["X_pca"].shape[1])
        if available_pcs < n_pcs:
            raise ContractError(
                f"existing X_pca has {available_pcs} components, but {n_pcs} were requested"
            )

    sc.pp.neighbors(adata, n_neighbors=n_neighbors, n_pcs=n_pcs, random_state=random_state)
    sc.tl.umap(adata, min_dist=min_dist, random_state=random_state)
    sc.tl.leiden(
        adata,
        resolution=resolution,
        key_added="leiden",
        random_state=random_state,
        flavor="igraph",
        n_iterations=2,
        directed=False,
    )
    effective_params = {
        "n_pcs": n_pcs,
        "n_neighbors": n_neighbors,
        "n_top_genes": n_top_genes,
        "resolution": resolution,
        "min_dist": min_dist,
        "random_state": random_state,
        "normalize": normalize,
        "log1p": log1p,
        "subset_hvg": subset_hvg,
        "scale": scale,
    }
    adata.uns["pca_neighbors_umap_leiden_params"] = effective_params
    cluster_counts = adata.obs["leiden"].value_counts().sort_index()
    report = {
        "schema_version": "1.0",
        "operation": "pca_neighbors_umap_leiden",
        "cells": int(adata.n_obs),
        "genes": int(adata.n_vars),
        "pca_computed": pca_computed,
        "n_pcs_used": int(adata.obsm["X_pca"].shape[1]),
        "n_clusters": int(len(cluster_counts)),
        "cluster_counts": {str(key): int(value) for key, value in cluster_counts.items()},
        "params": effective_params,
    }
    write_json(report, report_path)
    return adata, output_path, report_path


def celltypist_annotate(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    import celltypist
    from celltypist import models

    input_path = required_path(H5AD_INPUT)
    model_path = required_path(MODEL_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    majority_voting = opt_bool(params, "majority_voting", False)
    adata = read_h5ad(input_path)
    model = models.Model.load(str(model_path))
    annotations = celltypist.annotate(
        adata,
        model=model,
        majority_voting=majority_voting,
        over_clustering="leiden" if "leiden" in adata.obs else None,
    )
    annotated = annotations.to_adata(insert_labels=True, overwrite=True)
    label_column = "majority_voting" if majority_voting and "majority_voting" in annotated.obs else "predicted_labels"
    if label_column not in annotated.obs:
        raise ContractError(f"CellTypist result did not contain `{label_column}`")
    annotated.obs["celltypist_label"] = annotated.obs[label_column].astype(str)
    confidence_column = "conf_score"
    if confidence_column not in annotated.obs:
        raise ContractError("CellTypist result did not contain confidence scores")
    annotated.obs["celltypist_conf_score"] = annotated.obs[confidence_column].astype(float)
    annotated.uns["celltypist_params"] = {
        "model_file": model_path.name,
        "majority_voting": majority_voting,
    }
    confidence = annotated.obs["celltypist_conf_score"].to_numpy(dtype=float)
    report = {
        "schema_version": "1.0",
        "operation": "celltypist_annotate",
        "cells": int(annotated.n_obs),
        "model_file": model_path.name,
        "majority_voting": majority_voting,
        "label_column": label_column,
        "confidence": percentile_summary(confidence),
        "label_counts": {
            str(key): int(value)
            for key, value in annotated.obs["celltypist_label"].value_counts().items()
        },
    }
    write_json(report, report_path)
    return annotated, output_path, report_path


def safe_projection_column(key: str, dimension: int) -> str:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", key).strip("_")
    if not safe:
        raise ContractError(f"obsm key `{key}` cannot be projected to a column name")
    return f"obsm_{safe}_{dimension}"


def obs_to_parquet(params: dict[str, Any]) -> tuple[Path, Path]:
    input_path = required_path(H5AD_INPUT)
    output_path = required_output(PARQUET_OUTPUT)
    include_obsm = params.get("include_obsm", [])
    if not isinstance(include_obsm, list) or any(not isinstance(key, str) or not key for key in include_obsm):
        raise ContractError("include_obsm must be an array of nonempty strings")
    adata = read_h5ad(input_path, backed="r")
    if "cell_id" in adata.obs.columns:
        raise ContractError("obs already contains a reserved `cell_id` column")
    frame = adata.obs.copy()
    frame.insert(0, "cell_id", pd.Index(adata.obs_names).astype(str))
    for key in include_obsm:
        if key not in adata.obsm:
            raise ContractError(f"obsm key `{key}` is not present")
        embedding = np.asarray(adata.obsm[key])
        if embedding.ndim != 2 or embedding.shape[0] != adata.n_obs:
            raise ContractError(f"obsm key `{key}` is not a cell-aligned two-dimensional matrix")
        for dimension in range(embedding.shape[1]):
            frame[safe_projection_column(key, dimension)] = embedding[:, dimension]
    write_parquet(frame, output_path)
    return output_path, input_path


def subset_by_obs(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    input_path = required_path(H5AD_INPUT)
    selection_path = required_path(PARQUET_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    join_column = require_str(params, "join_column", "cell_id")
    try:
        selection = pd.read_parquet(selection_path, engine="pyarrow")
    except Exception as error:
        raise ContractError(f"cannot read selection Parquet `{selection_path}`: {error}") from error
    if join_column not in selection.columns:
        raise ContractError(f"selection Parquet has no `{join_column}` column")
    requested = selection[join_column].astype("string")
    if requested.isna().any() or requested.str.len().eq(0).any():
        raise ContractError(f"selection column `{join_column}` contains missing or empty IDs")
    requested = requested.drop_duplicates()
    adata = read_h5ad(input_path, backed="r")
    if join_column == "cell_id":
        obs_ids = pd.Index(adata.obs_names).astype(str)
    elif join_column in adata.obs.columns:
        obs_ids = adata.obs[join_column].astype(str)
    else:
        raise ContractError(f"H5AD obs has no `{join_column}` column")
    keep = obs_ids.isin(pd.Index(requested.astype(str)))
    matched = int(keep.sum())
    if matched == 0:
        raise ContractError("selection Parquet matched no cells in the H5AD file")
    adata = adata[keep].to_memory()
    adata.uns["subset_params"] = {"join_column": join_column, "requested_cells": int(len(requested))}
    report = {
        "schema_version": "1.0",
        "operation": "subset_by_obs",
        "join_column": join_column,
        "input_cells": int(len(keep)),
        "output_cells": int(adata.n_obs),
        "requested_unique_cells": int(len(requested)),
        "missing_cells": int(len(requested) - matched),
    }
    write_json(report, report_path)
    return adata, output_path, report_path


def run() -> None:
    workflow = os.environ.get(WORKFLOW_ENV, "").lower()
    params = load_params()
    if workflow == "qc_filter":
        adata, h5ad_path, report_path = qc_filter(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "pca_neighbors_umap_leiden":
        adata, h5ad_path, report_path = pca_neighbors_umap_leiden(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "celltypist_annotate":
        adata, h5ad_path, report_path = celltypist_annotate(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "obs_to_parquet":
        parquet_path, _ = obs_to_parquet(params)
        if not parquet_path.is_file():
            raise ContractError("obs projection did not produce Parquet output")
    elif workflow == "subset_by_obs":
        adata, h5ad_path, report_path = subset_by_obs(params)
        write_h5ad(adata, h5ad_path)
    else:
        raise ContractError(f"unsupported single-cell workflow `{workflow}`")


def main() -> int:
    try:
        run()
    except ContractError as error:
        print(f"single_cell_workflow: {error}", file=sys.stderr)
        return 2
    except Exception as error:
        print(f"single_cell_workflow: {error}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
