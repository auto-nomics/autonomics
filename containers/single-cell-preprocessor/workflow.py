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


def opt_int(params: dict[str, Any], name: str, default: int) -> int:
    value = params.get(name, default)
    if isinstance(value, bool) or not isinstance(value, int):
        raise ContractError(f"parameter `{name}` must be an integer")
    return value


def delimiter_spec(value: Any) -> str | None:
    if not isinstance(value, str) or not value.strip():
        raise ContractError("parameter `delimiter` must be a nonempty string")
    normalized = value.lower()
    if normalized in {"auto", ""}:
        return None
    if normalized in {"tab", "\\t", "\t"}:
        return "\t"
    if normalized in {"comma", ","}:
        return ","
    if normalized in {"semicolon", ";"}:
        return ";"
    if normalized in {"pipe", "|"}:
        return "|"
    if len(value) == 1:
        return value
    raise ContractError(f"unsupported delimiter `{value}`")


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


def sparse_or_dense_axis_mean(value: Any, axis: int) -> np.ndarray:
    if sp.issparse(value):
        return np.asarray(value.mean(axis=axis)).ravel()
    return np.asarray(value, dtype=float).mean(axis=axis)


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


def dense_ingest(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    import scanpy as sc

    matrix_path = required_path(H5AD_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    orientation = require_str(params, "orientation").lower()
    if orientation not in {"genes_by_cells", "cells_by_genes"}:
        raise ContractError("orientation must be `genes_by_cells` or `cells_by_genes`")
    has_header = opt_bool(params, "has_header", True)
    delimiter = delimiter_spec(params.get("delimiter", "auto"))
    try:
        frame = pd.read_csv(
            matrix_path,
            index_col=0,
            header=0 if has_header else None,
            sep=delimiter,
            engine="python" if delimiter is None else "c",
        )
    except Exception as error:
        raise ContractError(f"cannot read dense matrix `{matrix_path}`: {error}") from error
    frame.index = frame.index.astype(str)
    frame.columns = frame.columns.astype(str)
    if frame.index.has_duplicates:
        raise ContractError("dense matrix contains duplicate row identifiers")
    if frame.columns.has_duplicates:
        raise ContractError("dense matrix contains duplicate column identifiers")

    if orientation == "cells_by_genes":
        frame = frame.T
    try:
        values = frame.apply(pd.to_numeric, errors="raise").to_numpy(dtype=np.float64)
    except Exception as error:
        raise ContractError(f"dense matrix contains non-numeric expression values: {error}") from error
    if not np.isfinite(values).all():
        raise ContractError("dense matrix contains missing or non-finite expression values")

    obs = pd.DataFrame(index=pd.Index(frame.index, name="cell_id"))
    var = pd.DataFrame(index=pd.Index(frame.columns, name="gene_symbol"))
    adata = ad.AnnData(X=values, obs=obs, var=var)
    sample_label = params.get("sample_label")
    condition_label = params.get("condition_label")
    if sample_label is not None:
        if not isinstance(sample_label, str) or not sample_label:
            raise ContractError("sample_label must be a nonempty string when provided")
        adata.obs["sample"] = sample_label
    if condition_label is not None:
        if not isinstance(condition_label, str) or not condition_label:
            raise ContractError("condition_label must be a nonempty string when provided")
        adata.obs["condition"] = condition_label
    adata.layers["counts"] = adata.X.copy()

    input_cells, input_genes = adata.n_obs, adata.n_vars
    min_genes = opt_int(params, "min_genes", 0)
    min_cells = opt_int(params, "min_cells", 0)
    if min(min_genes, min_cells) < 0:
        raise ContractError("min_genes and min_cells cannot be negative")
    if min_genes > 0:
        sc.pp.filter_cells(adata, min_genes=min_genes)
    if min_cells > 0:
        sc.pp.filter_genes(adata, min_cells=min_cells)
    if adata.n_obs == 0 or adata.n_vars == 0:
        raise ContractError("dense ingest filters removed every cell or gene")

    report = {
        "schema_version": "1.0",
        "operation": "dense_ingest",
        "input_cells": int(input_cells),
        "input_genes": int(input_genes),
        "output_cells": int(adata.n_obs),
        "output_genes": int(adata.n_vars),
        "orientation": orientation,
        "delimiter": params.get("delimiter", "auto"),
        "has_header": has_header,
        "min_genes": min_genes,
        "min_cells": min_cells,
        "sample_label": sample_label,
        "condition_label": condition_label,
    }
    write_json(report, report_path)
    return adata, output_path, report_path


def rank_genes_groups(params: dict[str, Any]) -> tuple[Path, Path, ad.AnnData]:
    import scanpy as sc

    input_path = required_path(H5AD_INPUT)
    parquet_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    output_path = required_output("AUTONOMICS_OUTPUT2")
    groupby = require_str(params, "groupby")
    method = require_str(params, "method", "wilcoxon")
    reference = require_str(params, "reference", "rest")
    n_genes = opt_int(params, "n_genes", 100)
    if method not in {"wilcoxon", "t-test", "t-test_overestim_var", "logreg"}:
        raise ContractError(f"unsupported rank_genes_groups method `{method}`")
    if n_genes <= 0:
        raise ContractError("n_genes must be positive")

    adata = read_h5ad(input_path)
    if groupby not in adata.obs:
        raise ContractError(f"H5AD obs has no `{groupby}` column")
    if adata.obs[groupby].isna().any():
        raise ContractError(f"H5AD obs column `{groupby}` contains missing cluster labels")
    groups = adata.obs[groupby].drop_duplicates()
    if reference != "rest" and reference not in set(groups.astype(str)):
        raise ContractError(f"reference cluster `{reference}` is not present in `{groupby}`")
    sc.tl.rank_genes_groups(
        adata,
        groupby=groupby,
        method=method,
        reference=reference,
        n_genes=n_genes,
        key_added="rank_genes_groups",
        use_raw=False,
    )
    result = adata.uns["rank_genes_groups"]
    names = result["names"]
    result_groups = list(names.dtype.names or [])
    records: list[dict[str, Any]] = []
    for rank in range(int(len(names))):
        for group in result_groups:
            record = {"group": str(group), "gene": str(names[rank][group]), "rank": rank + 1}
            for output_name, result_name in (
                ("score", "scores"),
                ("pvalue", "pvals"),
                ("pvalue_adj", "pvals_adj"),
            ):
                if result_name in result:
                    value = result[result_name][rank][group]
                    record[output_name] = None if pd.isna(value) else float(value)
            records.append(record)
    marker_table = pd.DataFrame.from_records(records)
    write_parquet(marker_table, parquet_path)
    report = {
        "schema_version": "1.0",
        "operation": "rank_genes_groups",
        "cells": int(adata.n_obs),
        "groupby": groupby,
        "method": method,
        "reference": reference,
        "n_genes": n_genes,
        "groups": result_groups,
        "marker_rows": int(len(marker_table)),
    }
    write_json(report, report_path)
    return parquet_path, report_path, adata


def cluster_mean_expression(params: dict[str, Any]) -> Path:
    input_path = required_path(H5AD_INPUT)
    output_path = required_output(PARQUET_OUTPUT)
    groupby = require_str(params, "groupby")
    normalize = require_str(params, "normalize", "cp10k").lower()
    include_percent = opt_bool(params, "include_percent_expressed", True)
    if normalize not in {"cp10k", "none"}:
        raise ContractError("normalize must be `cp10k` or `none`")
    adata = read_h5ad(input_path, backed="r")
    if groupby not in adata.obs:
        raise ContractError(f"H5AD obs has no `{groupby}` column")
    gene_names = [str(value) for value in adata.var_names]
    requested = params.get("genes", [])
    if not isinstance(requested, list) or any(not isinstance(gene, str) or not gene for gene in requested):
        raise ContractError("genes must be an array of nonempty strings")
    selected = requested or gene_names
    if len(set(selected)) != len(selected):
        raise ContractError("requested genes must be unique")
    missing = sorted(set(selected) - set(gene_names))
    if missing:
        raise ContractError(f"requested genes are absent from H5AD: {', '.join(missing)}")
    positions = [gene_names.index(gene) for gene in selected]

    subset = adata[:, positions].to_memory()
    use_counts = normalize == "cp10k" and "counts" in adata.layers
    matrix = subset.layers["counts"] if use_counts else subset.X
    if normalize == "cp10k":
        total_source = adata.layers["counts"] if use_counts else adata.X
        totals = sparse_or_dense_axis_sum(total_source, axis=1)
        if np.any(totals <= 0):
            raise ContractError("cp10k normalization requires positive cell totals")
        if sp.issparse(matrix):
            matrix = matrix.tocsr().astype(np.float64)
            scaling = np.reciprocal(totals) * 1e4
            matrix = sp.diags(scaling) @ matrix
        else:
            matrix = np.asarray(matrix, dtype=np.float64) * (1e4 / totals)[:, None]

    groups = adata.obs[groupby].astype(str)
    records: list[dict[str, Any]] = []
    for group in sorted(groups.unique()):
        mask = (groups == group).to_numpy()
        selected_matrix = matrix[mask]
        means = sparse_or_dense_axis_sum(selected_matrix, axis=0) / int(mask.sum())
        fractions = column_counts(selected_matrix) / int(mask.sum())
        for index, gene in enumerate(selected):
            record = {
                "cluster": group,
                "gene": gene,
                "mean_expression": float(means[index]),
            }
            if include_percent:
                record["pct_expressed"] = float(fractions[index] * 100)
            records.append(record)
    write_parquet(pd.DataFrame.from_records(records), output_path)
    return output_path


def normalize_log_cp10k(x: Any) -> Any:
    counts = np.asarray(x.sum(axis=1)).ravel()
    scale = np.zeros_like(counts, dtype=float)
    nonzero = counts > 0
    scale[nonzero] = 1e4 / counts[nonzero]
    if sp.issparse(x):
        return (sp.diags(scale) @ x).log1p()
    return np.log1p(x * scale[:, None])


def marker_annotate(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    input_path = required_path(H5AD_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    marker_sets = params.get("marker_sets")
    if not isinstance(marker_sets, dict) or not marker_sets:
        raise ContractError("marker_sets must be a nonempty object")
    groupby = params.get("groupby")
    if groupby is not None and (not isinstance(groupby, str) or not groupby.strip()):
        raise ContractError("groupby must be a nonempty string when provided")
    normalize = params.get("normalize", "log_cp10k")
    if normalize not in {"log_cp10k", "none"}:
        raise ContractError("normalize must be `log_cp10k` or `none`")
    min_score = params.get("min_score", 0.0)
    if isinstance(min_score, bool) or not isinstance(min_score, (int, float)):
        raise ContractError("min_score must be a number")
    unknown_label = require_str(params, "unknown_label", "Unknown")
    adata = read_h5ad(input_path)
    known = set(str(value) for value in adata.var_names)
    summaries: dict[str, dict[str, Any]] = {}
    indices: dict[str, np.ndarray] = {}
    for name, genes in marker_sets.items():
        if not isinstance(name, str) or not name:
            raise ContractError(f"marker set name `{name}` is empty")
        if not isinstance(genes, list) or any(not isinstance(gene, str) or not gene for gene in genes):
            raise ContractError(f"marker set `{name}` must be an array of nonempty strings")
        present = [gene for gene in genes if gene in known]
        if not present:
            raise ContractError(f"marker set `{name}` has no genes present in the H5AD")
        indices[name] = np.asarray([adata.var_names.get_loc(gene) for gene in present], dtype=int)
        summaries[name] = {
            "genes_requested": int(len(genes)),
            "genes_present": int(len(present)),
            "missing_genes": sorted(set(genes) - known),
        }
    x = adata.X
    if normalize == "log_cp10k":
        x = normalize_log_cp10k(x)
    names = list(indices)
    score_matrix = np.column_stack([
        sparse_or_dense_axis_mean(x[:, index], axis=1) for index in indices.values()
    ])
    rows = np.arange(score_matrix.shape[0])
    top_index = score_matrix.argmax(axis=1)
    top = score_matrix[rows, top_index]
    masked = score_matrix.copy()
    masked[rows, top_index] = -np.inf
    second = masked.max(axis=1)
    # With a single marker set there is no runner-up: report the margin as the
    # top score itself (report JSON forbids inf).
    margin = np.where(np.isfinite(second), top - second, top)
    cluster_labels: dict[str, str] | None = None
    if groupby is None:
        labels = np.where(top > min_score, np.asarray(names)[top_index], unknown_label).astype(str)
        cell_scores = top
        cell_margins = margin
    else:
        if groupby not in adata.obs:
            raise ContractError(f"H5AD obs has no `{groupby}` column")
        group_keys = adata.obs[groupby].astype(str).to_numpy()
        frame = pd.DataFrame(score_matrix, columns=names)
        frame["__group__"] = group_keys
        cluster_scores = frame.groupby("__group__", sort=True)[names].mean()
        cvals = cluster_scores.to_numpy(dtype=float)
        c_rows = np.arange(cvals.shape[0])
        c_top_index = cvals.argmax(axis=1)
        c_top = cvals[c_rows, c_top_index]
        c_masked = cvals.copy()
        c_masked[c_rows, c_top_index] = -np.inf
        c_second = c_masked.max(axis=1)
        c_margin = np.where(np.isfinite(c_second), c_top - c_second, c_top)
        c_labels = np.where(
            c_top > min_score, np.asarray(cluster_scores.columns)[c_top_index], unknown_label
        ).astype(str)
        label_by_group = dict(zip(cluster_scores.index.astype(str), c_labels))
        margin_by_group = dict(zip(cluster_scores.index.astype(str), c_margin))
        assigned = np.asarray([label_by_group[key] for key in group_keys])
        column_by_name = {name: position for position, name in enumerate(names)}
        # Labeled cells keep their own score for the assigned label; cells under
        # the unknown fallback report their own best score as evidence.
        fallback = assigned == unknown_label
        own = score_matrix[rows, np.asarray([column_by_name[label] for label in assigned])]
        labels = assigned
        cell_scores = np.where(fallback, top, own)
        cell_margins = np.asarray([margin_by_group[key] for key in group_keys])
        cluster_labels = {key: str(value) for key, value in label_by_group.items()}
    adata.obs["marker_label"] = labels
    adata.obs["marker_score"] = cell_scores.astype(float)
    adata.obs["marker_margin"] = cell_margins.astype(float)
    adata.uns["marker_annotate_params"] = {
        "normalize": normalize,
        "groupby": groupby,
        "min_score": float(min_score),
        "unknown_label": unknown_label,
    }
    report = {
        "schema_version": "1.0",
        "operation": "marker_annotate",
        "cells": int(adata.n_obs),
        "mode": "cluster" if groupby is not None else "per_cell",
        "groupby": groupby,
        "normalize": normalize,
        "min_score": float(min_score),
        "unknown_label": unknown_label,
        "marker_sets": summaries,
        "label_counts": {
            str(key): int(value)
            for key, value in adata.obs["marker_label"].value_counts().items()
        },
    }
    if cluster_labels is not None:
        report["cluster_labels"] = cluster_labels
    write_json(report, report_path)
    return adata, output_path, report_path


def ucell_score(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    from scipy.stats import rankdata

    input_path = required_path(H5AD_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    gene_sets = params.get("gene_sets")
    if not isinstance(gene_sets, dict) or not gene_sets:
        raise ContractError("gene_sets must be a nonempty object")
    adata = read_h5ad(input_path)
    known = set(str(value) for value in adata.var_names)
    summaries: dict[str, dict[str, Any]] = {}
    indices: dict[str, np.ndarray] = {}
    for name, genes in gene_sets.items():
        if not isinstance(name, str) or not name or name in adata.obs:
            raise ContractError(f"gene set name `{name}` is empty or already present in obs")
        if not isinstance(genes, list) or any(not isinstance(gene, str) or not gene for gene in genes):
            raise ContractError(f"gene set `{name}` must be an array of nonempty strings")
        present = [gene for gene in genes if gene in known]
        if not present:
            raise ContractError(f"gene set `{name}` has no genes present in the H5AD")
        indices[name] = np.asarray([adata.var_names.get_loc(gene) for gene in present], dtype=int)
        missing = sorted(set(genes) - known)
        summaries[name] = {
            "genes_requested": int(len(genes)),
            "genes_present": int(len(present)),
            "missing_genes": missing,
        }
    n_genes = int(adata.n_vars)
    scores = {name: np.empty(adata.n_obs, dtype=float) for name in indices}
    chunk_cells = max(1, min(1000, adata.n_obs))
    for start in range(0, adata.n_obs, chunk_cells):
        stop = min(start + chunk_cells, adata.n_obs)
        block = adata.X[start:stop]
        dense = (
            block.toarray().astype(np.float64)
            if sp.issparse(block)
            else np.asarray(block, dtype=np.float64)
        )
        # UCell: rank genes within each cell, rank 1 = highest expression
        # (average ranks break ties), then score = mean(margin/(margin+rank))
        # over the signature with margin = (n_genes - signature size) / 2.
        descending = (n_genes + 1) - rankdata(dense, axis=1, method="average")
        for name, index in indices.items():
            rank_margin = max(1.0, (n_genes - len(index)) / 2.0)
            contribution = rank_margin / (rank_margin + descending[:, index])
            scores[name][start:stop] = contribution.mean(axis=1)
    for name, values in scores.items():
        adata.obs[name] = values
        summaries[name]["mean"] = float(np.mean(values))
        summaries[name].update(percentile_summary(values))
    adata.uns["ucell_params"] = {
        "method": "ucell",
        "gene_sets": {name: int(len(index)) for name, index in indices.items()},
    }
    report = {
        "schema_version": "1.0",
        "operation": "ucell_score",
        "cells": int(adata.n_obs),
        "genes": n_genes,
        "gene_sets": summaries,
    }
    write_json(report, report_path)
    return adata, output_path, report_path


def gene_set_score(params: dict[str, Any]) -> tuple[ad.AnnData, Path, Path]:
    import scanpy as sc

    input_path = required_path(H5AD_INPUT)
    output_path = required_output(H5AD_OUTPUT)
    report_path = required_output(REPORT_OUTPUT)
    gene_sets = params.get("gene_sets")
    if not isinstance(gene_sets, dict) or not gene_sets:
        raise ContractError("gene_sets must be a nonempty object")
    ctrl_size = opt_int(params, "ctrl_size", 50)
    random_state = opt_int(params, "random_state", 0)
    if ctrl_size <= 0:
        raise ContractError("ctrl_size must be positive")
    adata = read_h5ad(input_path)
    known = set(str(value) for value in adata.var_names)
    summaries: dict[str, dict[str, Any]] = {}
    for name, genes in gene_sets.items():
        if not isinstance(name, str) or not name or name in adata.obs:
            raise ContractError(f"gene set name `{name}` is empty or already present in obs")
        if not isinstance(genes, list) or any(not isinstance(gene, str) or not gene for gene in genes):
            raise ContractError(f"gene set `{name}` must be an array of nonempty strings")
        present = [gene for gene in genes if gene in known]
        missing = sorted(set(genes) - known)
        if len(present) < 2:
            raise ContractError(f"gene set `{name}` has fewer than two genes present")
        effective_ctrl_size = max(1, min(ctrl_size, adata.n_vars - 1))
        sc.score_genes(
            adata,
            gene_list=present,
            score_name=name,
            ctrl_size=effective_ctrl_size,
            random_state=random_state,
        )
        score = adata.obs[name].to_numpy(dtype=float)
        summaries[name] = {
            "genes_requested": int(len(genes)),
            "genes_present": int(len(present)),
            "missing_genes": missing,
            "mean": float(np.mean(score)),
            **percentile_summary(score),
        }
    report = {
        "schema_version": "1.0",
        "operation": "gene_set_score",
        "cells": int(adata.n_obs),
        "ctrl_size": ctrl_size,
        "random_state": random_state,
        "gene_sets": summaries,
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
    elif workflow == "subset_by_obs":
        adata, h5ad_path, report_path = subset_by_obs(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "dense_ingest":
        adata, h5ad_path, report_path = dense_ingest(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "rank_genes_groups":
        parquet_path, _report_path, adata = rank_genes_groups(params)
        if not parquet_path.is_file():
            raise ContractError("rank_genes_groups did not produce Parquet output")
        write_h5ad(adata, required_output("AUTONOMICS_OUTPUT2"))
    elif workflow == "cluster_mean_expression":
        parquet_path = cluster_mean_expression(params)
        if not parquet_path.is_file():
            raise ContractError("cluster_mean_expression did not produce Parquet output")
    elif workflow == "gene_set_score":
        adata, h5ad_path, report_path = gene_set_score(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "marker_annotate":
        adata, h5ad_path, report_path = marker_annotate(params)
        write_h5ad(adata, h5ad_path)
    elif workflow == "ucell_score":
        adata, h5ad_path, report_path = ucell_score(params)
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
