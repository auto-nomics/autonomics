#!/usr/bin/env python3
"""Pathology Stage-A/B container entrypoint.

Whole-slide-image ingestion, QC, deterministic patch sampling, foundation-
model embedding, batch-domain checks, IHC quantification, and QuPath
annotation export. The container reads paths from AUTONOMICS_INPUT*, writes
only declared files, and never fetches remote data — embedding weights must
be staged locally by the operator (UNI-class checkpoints are license-gated).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import sys
from pathlib import Path
from typing import Any, Sequence

import h5py
import numpy as np
import pandas as pd
import tiffslide
from PIL import Image
from skimage.color import rgb2hed, rgb2hsv
from skimage.filters import laplace
from skimage.measure import approximate_polygon, find_contours
from skimage.morphology import binary_closing, binary_opening, disk, remove_small_objects

IMAGE_SUFFIXES = {".png", ".tif", ".tiff", ".jpg", ".jpeg", ".bmp", ".webp"}
DEFAULT_IMAGE_NET_MEAN = (0.485, 0.456, 0.406)
DEFAULT_IMAGE_NET_STD = (0.229, 0.224, 0.225)


def input_paths(index: int) -> list[Path]:
    value = os.environ.get(f"AUTONOMICS_INPUT{index}", "")
    paths = [Path(part) for part in value.split(",") if part]
    if not paths:
        raise RuntimeError(f"AUTONOMICS_INPUT{index} is empty")
    return paths


def output_path(index: int) -> Path:
    return Path(os.environ[f"AUTONOMICS_OUTPUT{index}"])


def output_dir(index: int) -> Path:
    path = output_path(index)
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return f"sha256:{digest.hexdigest()}"


def settings_env(name: str) -> dict[str, Any]:
    return json.loads(os.environ.get(name, "{}"))


def write_json(index: int, payload: dict[str, Any]) -> None:
    output_dir(index).write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def open_slide(path: Path) -> tiffslide.SlideProps:
    if not path.is_file():
        raise RuntimeError(f"WSI input must be a single file; got `{path}`")
    return tiffslide.open_slide(str(path))


def slide_mpp(slide: tiffslide.SlideProps) -> tuple[float | None, float | None]:
    properties = slide.properties
    candidates_x = [
        "openslide.mpp-x",
        "tiffslide.mpp-x",
        "mirax.general.properties.scanner.slide_mpp_x",
        "hamamatsu.SourceLens",
    ]
    candidates_y = ["openslide.mpp-y", "tiffslide.mpp-y"]
    mpp_x = _first_float(properties, candidates_x)
    mpp_y = _first_float(properties, candidates_y)
    return mpp_x, mpp_y


def _first_float(properties: dict[str, Any], keys: Sequence[str]) -> float | None:
    for key in keys:
        value = properties.get(key)
        if value is None:
            continue
        try:
            parsed = float(value)
        except (TypeError, ValueError):
            continue
        if parsed > 0.0 and math.isfinite(parsed):
            return parsed
    return None


def pick_level(slide: tiffslide.SlideProps, max_downsample: float) -> int:
    """Highest-resolution level whose downsample does not exceed the cap."""
    best = 0
    for level, downsample in enumerate(slide.level_downsamples):
        if downsample <= max_downsample:
            best = level
    return best


def read_rgb(slide: tiffslide.SlideProps, location: tuple[int, int], level: int, size: tuple[int, int]) -> np.ndarray:
    region = slide.read_region(location, level, size).convert("RGB")
    return np.asarray(region, dtype=np.uint8)


def tissue_mask_from_rgb(rgb: np.ndarray, saturation_threshold: float, value_floor: float, value_ceiling: float) -> np.ndarray:
    hsv = rgb2hsv(rgb.astype(np.float32) / 255.0)
    saturation, value = hsv[..., 1], hsv[..., 2]
    return (
        (saturation > saturation_threshold)
        & (value > value_floor)
        & (value < value_ceiling)
    )


def working_tissue_mask(
    slide: tiffslide.SlideProps,
    working_level: int,
    max_working_width: int,
    open_radius: int,
    close_radius: int,
    min_object_px: int,
    saturation_threshold: float,
    value_floor: float,
    value_ceiling: float,
) -> tuple[np.ndarray, int]:
    """Binary tissue mask at a working level, downscaled to a bounded width.

    Returns the mask and its width in pixels; level-0 coordinates map to mask
    pixels via `mask_width / level0_width`.
    """
    width, height = slide.level_dimensions[working_level]
    scale = max(1, math.ceil(width / max_working_width))
    working = read_rgb(slide, (0, 0), working_level, (width, height))
    if scale > 1:
        working = np.asarray(
            Image.fromarray(working).resize((width // scale, height // scale), Image.BILINEAR)
        )
    mask = tissue_mask_from_rgb(
        working, saturation_threshold, value_floor, value_ceiling
    )
    if open_radius > 0:
        mask = binary_opening(mask, disk(open_radius))
    if close_radius > 0:
        mask = binary_closing(mask, disk(close_radius))
    if min_object_px > 0:
        mask = remove_small_objects(mask, min_size=min_object_px)
    return mask, mask.shape[1]


def positive_int(settings: dict[str, Any], key: str, default: int, minimum: int = 1) -> int:
    value = int(settings.get(key, default))
    if value < minimum:
        raise RuntimeError(f"{key} must be >= {minimum}")
    return value


def unit_interval(settings: dict[str, Any], key: str, default: float) -> float:
    value = float(settings.get(key, default))
    if not (math.isfinite(value) and 0.0 <= value <= 1.0):
        raise RuntimeError(f"{key} must lie in [0, 1]")
    return value


def slide_summary(slide: tiffslide.SlideProps, path: Path) -> dict[str, Any]:
    mpp_x, mpp_y = slide_mpp(slide)
    return {
        "source_path": str(path),
        "source_hash": sha256(path),
        "driver": slide.__class__.__name__,
        "dimensions": [int(value) for value in slide.dimensions],
        "level_count": int(slide.level_count),
        "level_dimensions": [[int(w), int(h)] for w, h in slide.level_dimensions],
        "level_downsamples": [float(value) for value in slide.level_downsamples],
        "mpp_x": mpp_x,
        "mpp_y": mpp_y,
        "mpp_known": mpp_x is not None,
        "vendor": slide.properties.get("openslide.vendor")
        or slide.properties.get("tiffslide.vendor"),
    }


def wsi_ingest() -> None:
    settings = settings_env("PATHOLOGY_WSI_SETTINGS")
    path = input_paths(0)[0]
    slide = open_slide(path)
    summary = slide_summary(slide, path)

    max_thumbnail = positive_int(settings, "max_thumbnail_width", 2048, minimum=64)
    width, height = slide.dimensions
    scale = max_thumbnail / max(width, height)
    thumbnail_size = (max(1, int(width * scale)), max(1, int(height * scale)))
    thumbnail = slide.get_thumbnail(thumbnail_size)
    output_dir(0)
    thumbnail.save(output_path(0), format="PNG")
    write_json(1, {"slide": summary, "thumbnail_size": list(thumbnail.size)})


def wsi_qc() -> None:
    settings = settings_env("PATHOLOGY_QC_SETTINGS")
    path = input_paths(0)[0]
    slide = open_slide(path)
    summary = slide_summary(slide, path)

    level = positive_int(settings, "level", -1, minimum=-1)
    if level == -1:
        level = pick_level(slide, float(settings.get("max_downsample", 16.0)))
    if level >= slide.level_count:
        raise RuntimeError(f"level {level} exceeds level_count {slide.level_count}")
    tile_size = positive_int(settings, "tile_size", 512, minimum=64)
    max_tiles = positive_int(settings, "max_tiles", 400, minimum=1)
    saturation_threshold = unit_interval(settings, "saturation_threshold", 0.2)
    value_floor = unit_interval(settings, "value_floor", 0.15)
    value_ceiling = unit_interval(settings, "value_ceiling", 0.92)
    focus_threshold = float(settings.get("focus_threshold", 40.0))
    min_slide_tissue_fraction = unit_interval(settings, "min_slide_tissue_fraction", 0.05)

    width, height = slide.level_dimensions[level]
    stride_x = max(1, math.ceil(width / tile_size / max_tiles) if width > tile_size * max_tiles else 1)
    stride_y = max(1, math.ceil(height / tile_size / max_tiles) if height > tile_size * max_tiles else 1)
    stride = max(stride_x, stride_y)
    downsample = slide.level_downsamples[level]

    rows: list[dict[str, Any]] = []
    for ty in range(0, max(1, height - tile_size + 1), tile_size * stride):
        for tx in range(0, max(1, width - tile_size + 1), tile_size * stride):
            rgb = read_rgb(slide, (int(tx * downsample), int(ty * downsample)), level, (tile_size, tile_size))
            tissue = tissue_mask_from_rgb(rgb, saturation_threshold, value_floor, value_ceiling)
            gray = rgb.astype(np.float32) / 255.0
            gray = 0.299 * gray[..., 0] + 0.587 * gray[..., 1] + 0.114 * gray[..., 2]
            rows.append(
                {
                    "tile_x_level": int(tx),
                    "tile_y_level": int(ty),
                    "level": level,
                    "tissue_fraction": float(tissue.mean()),
                    "focus_laplacian_variance": float(laplace(gray).var()),
                    "mean_r": float(rgb[..., 0].mean()),
                    "mean_g": float(rgb[..., 1].mean()),
                    "mean_b": float(rgb[..., 2].mean()),
                }
            )
    if not rows:
        raise RuntimeError("wsi-qc sampled no tiles; slide level is smaller than one tile")

    frame = pd.DataFrame(rows)
    tissue_fraction = float(frame["tissue_fraction"].mean())
    median_focus = float(frame["focus_laplacian_variance"].median())
    blank_fraction = float((frame["tissue_fraction"] < 0.01).mean())
    low_focus_fraction = float(
        (frame.loc[frame["tissue_fraction"] > 0.1, "focus_laplacian_variance"] < focus_threshold).mean()
    )
    checks = {
        "tissue_present": tissue_fraction >= min_slide_tissue_fraction,
        "focus_acceptable": math.isnan(low_focus_fraction) or low_focus_fraction < 0.5,
        "not_mostly_blank": blank_fraction < 0.9,
    }
    pd.DataFrame(rows).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "slide": summary,
            "level": level,
            "tile_size": tile_size,
            "n_tiles": len(frame),
            "mean_tissue_fraction": tissue_fraction,
            "median_focus_laplacian_variance": median_focus,
            "blank_tile_fraction": blank_fraction,
            "low_focus_tile_fraction": low_focus_fraction,
            "focus_threshold": focus_threshold,
            "checks": checks,
            "status": "valid" if all(checks.values()) else "qc_flagged",
            "settings": settings,
        },
    )


def patch_sample() -> None:
    settings = settings_env("PATHOLOGY_PATCH_SETTINGS")
    path = input_paths(0)[0]
    slide = open_slide(path)
    summary = slide_summary(slide, path)

    patch_level = positive_int(settings, "level", 0, minimum=0)
    if patch_level >= slide.level_count:
        raise RuntimeError(f"level {patch_level} exceeds level_count {slide.level_count}")
    patch_size = positive_int(settings, "patch_size", 256, minimum=32)
    max_patches = positive_int(settings, "max_patches", 5000, minimum=1)
    min_tissue_fraction = unit_interval(settings, "min_tissue_fraction", 0.5)
    seed = int(settings.get("seed", 0))
    mask_level_max_downsample = float(settings.get("mask_max_downsample", 64.0))
    max_working_width = positive_int(settings, "mask_max_width", 4096, minimum=256)
    open_radius = positive_int(settings, "mask_open_radius", 3, minimum=0)
    close_radius = positive_int(settings, "mask_close_radius", 3, minimum=0)
    min_object_px = positive_int(settings, "mask_min_object_px", 500, minimum=0)
    saturation_threshold = unit_interval(settings, "saturation_threshold", 0.2)
    value_floor = unit_interval(settings, "value_floor", 0.15)
    value_ceiling = unit_interval(settings, "value_ceiling", 0.92)

    mask_level = pick_level(slide, mask_level_max_downsample)
    mask, mask_width = working_tissue_mask(
        slide,
        mask_level,
        max_working_width,
        open_radius,
        close_radius,
        min_object_px,
        saturation_threshold,
        value_floor,
        value_ceiling,
    )
    Image.fromarray(mask.astype(np.uint8) * 255).save(output_dir(1), format="PNG")

    level0_width = slide.dimensions[0]
    mask_scale = mask_width / level0_width
    patch_step = int(round(patch_size * slide.level_downsamples[patch_level]))

    candidates: list[dict[str, Any]] = []
    level_width, level_height = slide.level_dimensions[patch_level]
    for y in range(0, level_height - patch_size + 1, patch_step):
        for x in range(0, level_width - patch_size + 1, patch_step):
            mx0 = int(x * slide.level_downsamples[patch_level] * mask_scale)
            mx1 = int((x * slide.level_downsamples[patch_level] + patch_size * slide.level_downsamples[patch_level]) * mask_scale)
            my0 = int(y * slide.level_downsamples[patch_level] * mask_scale)
            my1 = int((y * slide.level_downsamples[patch_level] + patch_size * slide.level_downsamples[patch_level]) * mask_scale)
            mx0, my0 = max(0, min(mx0, mask.shape[1])), max(0, min(my0, mask.shape[0]))
            mx1, my1 = max(mx0 + 1, min(mx1, mask.shape[1])), max(my0 + 1, min(my1, mask.shape[0]))
            window = mask[my0:my1, mx0:mx1]
            fraction = float(window.mean()) if window.size else 0.0
            if fraction >= min_tissue_fraction:
                candidates.append(
                    {
                        "x": int(x * slide.level_downsamples[patch_level]),
                        "y": int(y * slide.level_downsamples[patch_level]),
                        "level": patch_level,
                        "size_px": patch_size,
                        "tissue_fraction": fraction,
                    }
                )
    if not candidates:
        raise RuntimeError(
            "no patch satisfied the tissue-fraction threshold; lower min_tissue_fraction"
        )

    rng = np.random.default_rng(seed)
    order = rng.permutation(len(candidates))
    selected = [candidates[index] for index in order[:max_patches]]
    selected.sort(key=lambda row: (row["y"], row["x"]))
    for patch_id, row in enumerate(selected):
        row["patch_id"] = f"p{patch_id:06d}"
    frame = pd.DataFrame(selected)[["patch_id", "x", "y", "level", "size_px", "tissue_fraction"]]
    frame.to_parquet(output_dir(0), index=False)

    mpp_x = summary["mpp_x"]
    write_json(
        2,
        {
            "slide": summary,
            "mask_level": mask_level,
            "mask_width": int(mask_width),
            "mask_scale_level0_to_mask": mask_scale,
            "tissue_mask_area_fraction": float(mask.mean()),
            "patch_level": patch_level,
            "patch_size": patch_size,
            "patch_size_um": (patch_size * mpp_x) if mpp_x else None,
            "n_candidates": len(candidates),
            "n_selected": len(frame),
            "max_patches": max_patches,
            "min_tissue_fraction": min_tissue_fraction,
            "seed": seed,
            "columns": list(frame.columns),
            "coordinate_convention": "level-0 top-left pixels, read_region semantics",
        },
    )


def load_embedding_model(model_paths: Sequence[Path]) -> tuple[Any, dict[str, Any]]:
    """Create a timm feature extractor from locally staged weights.

    The model bundle is a set of files: an optional `model_config.json`
    (`{"arch": ..., "img_size": ..., "timm_kwargs": {...}}`) plus exactly one
    `.pth`/`.pt`/`.safetensors` checkpoint. Nothing is ever downloaded: gated
    checkpoints (UNI-class) must be accepted and staged by the operator.
    """
    import timm
    import torch

    config: dict[str, Any] = {}
    config_path = next((path for path in model_paths if path.name == "model_config.json"), None)
    if config_path is not None:
        config = json.loads(config_path.read_text())
    weights = [
        path
        for path in model_paths
        if path.suffix.lower() in {".pth", ".pt", ".safetensors"}
    ]
    if len(weights) != 1:
        raise RuntimeError(
            "model input must contain exactly one .pth/.pt/.safetensors checkpoint; "
            f"found {len(weights)}"
        )
    arch = str(config.get("arch", "vit_large_patch16_224"))
    img_size = int(config.get("img_size", 224))
    timm_kwargs = dict(config.get("timm_kwargs", {}))
    timm_kwargs.setdefault("init_values", 1e-5)
    timm_kwargs.setdefault("dynamic_img_size", True)
    model = timm.create_model(
        arch,
        pretrained=False,
        num_classes=0,
        img_size=img_size,
        checkpoint_path=str(weights[0]),
        **timm_kwargs,
    )
    model.eval()
    provenance = {
        "arch": arch,
        "img_size": img_size,
        "timm_kwargs": timm_kwargs,
        "checkpoint": weights[0].name,
        "checkpoint_sha256": sha256(weights[0]),
        "timm_version": timm.__version__,
        "torch_version": torch.__version__,
    }
    return model, provenance


def wsi_embed() -> None:
    import torch

    settings = settings_env("PATHOLOGY_EMBED_SETTINGS")
    slide_path = input_paths(0)[0]
    patch_table = input_paths(1)[0]
    model_paths = input_paths(2)
    slide = open_slide(slide_path)
    summary = slide_summary(slide, slide_path)

    frame = pd.read_parquet(patch_table) if patch_table.suffix.lower() == ".parquet" else pd.read_csv(patch_table)
    required = {"patch_id", "x", "y", "level", "size_px"}
    missing = sorted(required.difference(frame.columns))
    if missing:
        raise RuntimeError(f"patch table is missing columns: {', '.join(missing)}")
    if frame.empty:
        raise RuntimeError("patch table is empty")

    batch_size = positive_int(settings, "batch_size", 32)
    requested_device = str(settings.get("device", "auto")).lower()
    cuda_available = torch.cuda.is_available()
    if requested_device == "auto":
        device = torch.device("cuda" if cuda_available else "cpu")
    else:
        device = torch.device(requested_device)
        if device.type == "cuda" and not cuda_available:
            raise RuntimeError("device `cuda` requested but no CUDA device is visible")
    amp = bool(settings.get("amp", False)) and device.type == "cuda"

    model, provenance = load_embedding_model(model_paths)
    model = model.to(device)

    img_size = provenance["img_size"]
    mean = torch.tensor(DEFAULT_IMAGE_NET_MEAN).view(3, 1, 1).to(device)
    std = torch.tensor(DEFAULT_IMAGE_NET_STD).view(3, 1, 1).to(device)

    def to_tensor(rgb: np.ndarray) -> torch.Tensor:
        image = Image.fromarray(rgb).resize((img_size, img_size), Image.BILINEAR)
        tensor = torch.from_numpy(np.asarray(image, dtype=np.float32) / 255.0)
        tensor = tensor.permute(2, 0, 1).unsqueeze(0)
        return (tensor - mean) / std

    embeddings: list[np.ndarray] = []
    batch: list[np.ndarray] = []
    with torch.inference_mode():
        for row in frame.to_dict(orient="records"):
            size = int(row["size_px"])
            location = (int(row["x"]), int(row["y"]))
            rgb = read_rgb(slide, location, int(row["level"]), (size, size))
            batch.append(to_tensor(rgb))
            if len(batch) == batch_size:
                embeddings.append(embed_batch(model, batch, amp).cpu().numpy())
                batch = []
        if batch:
            embeddings.append(embed_batch(model, batch, amp).cpu().numpy())

    stacked = np.concatenate(embeddings, axis=0).astype(np.float32)
    with h5py.File(output_dir(0), "w") as handle:
        dataset = handle.create_dataset("embeddings", data=stacked)
        dataset.attrs["arch"] = provenance["arch"]
        dataset.attrs["checkpoint_sha256"] = provenance["checkpoint_sha256"]
        handle.create_dataset(
            "patch_ids",
            data=np.asarray([str(value) for value in frame["patch_id"]], dtype=object),
            dtype=h5py.string_dtype(),
        )
        handle.create_dataset("xy", data=frame[["x", "y"]].to_numpy(dtype=np.int64))
    write_json(
        1,
        {
            "slide": summary,
            "n_patches": int(stacked.shape[0]),
            "embedding_dim": int(stacked.shape[1]),
            "device": str(device),
            "cuda_available": cuda_available,
            "amp": amp,
            "batch_size": batch_size,
            "model": provenance,
            "patch_table_hash": sha256(patch_table),
        },
    )


def embed_batch(model: Any, batch: list[Any], amp: bool) -> Any:
    import torch

    stacked = torch.cat(batch, dim=0)
    if amp:
        with torch.autocast(device_type="cuda", dtype=torch.float16):
            return model(stacked).float()
    return model(stacked)


def read_embeddings(path: Path) -> tuple[np.ndarray, np.ndarray]:
    with h5py.File(path, "r") as handle:
        embeddings = handle["embeddings"][()].astype(np.float64)
        patch_ids = np.asarray(
            [value.decode() if isinstance(value, bytes) else str(value) for value in handle["patch_ids"][()]]
        )
    return embeddings, patch_ids


def domain_check() -> None:
    from sklearn.metrics import silhouette_score

    settings = settings_env("PATHOLOGY_DOMAIN_SETTINGS")
    reference_path = input_paths(0)[0]
    comparison_path = input_paths(1)[0]
    reference, _ = read_embeddings(reference_path)
    comparison, _ = read_embeddings(comparison_path)
    if reference.shape[1] != comparison.shape[1]:
        raise RuntimeError(
            f"embedding dimensions differ: {reference.shape[1]} vs {comparison.shape[1]}"
        )

    normalize = bool(settings.get("l2_normalize", True))
    shrinkage = float(settings.get("shrinkage", 0.1))
    if not (0.0 <= shrinkage < 1.0):
        raise RuntimeError("shrinkage must lie in [0, 1)")
    if normalize:
        reference = reference / np.linalg.norm(reference, axis=1, keepdims=True)
        comparison = comparison / np.linalg.norm(comparison, axis=1, keepdims=True)

    mean_reference = reference.mean(axis=0)
    mean_comparison = comparison.mean(axis=0)
    difference = mean_reference - mean_comparison
    cosine_distance = float(
        1.0 - (mean_reference @ mean_comparison)
        / (np.linalg.norm(mean_reference) * np.linalg.norm(mean_comparison))
    )

    pooled = np.concatenate([reference, comparison], axis=0)
    labels = np.concatenate([np.zeros(len(reference)), np.ones(len(comparison))])
    pooled_covariance = np.cov(pooled, rowvar=False)
    dimension = pooled.shape[1]
    pooled_covariance = (1.0 - shrinkage) * pooled_covariance + shrinkage * (
        np.trace(pooled_covariance) / dimension
    ) * np.eye(dimension)
    mahalanobis_squared = float(difference @ np.linalg.solve(pooled_covariance, difference))

    silhouette_cap = positive_int(settings, "silhouette_max_samples", 20000)
    if len(pooled) > silhouette_cap:
        rng = np.random.default_rng(int(settings.get("seed", 0)))
        subset = rng.choice(len(pooled), size=silhouette_cap, replace=False)
        subset = np.sort(subset)
        silhouette = float(silhouette_score(pooled[subset], labels[subset], metric="cosine"))
    else:
        silhouette = float(silhouette_score(pooled, labels, metric="cosine"))

    standardized_shift = np.abs(difference) / np.sqrt(
        reference.var(axis=0) + comparison.var(axis=0) + 1e-12
    )
    top_dimensions = np.argsort(-standardized_shift)[:10]

    row = {
        "status": "valid",
        "n_reference": len(reference),
        "n_comparison": len(comparison),
        "embedding_dim": dimension,
        "centroid_cosine_distance": cosine_distance,
        "centroid_mahalanobis_squared": mahalanobis_squared,
        "batch_silhouette": silhouette,
        "max_standardized_dim_shift": float(standardized_shift.max()),
        "l2_normalize": normalize,
        "shrinkage": shrinkage,
    }
    pd.DataFrame([row]).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            **row,
            "reference_hash": sha256(reference_path),
            "comparison_hash": sha256(comparison_path),
            "top_shifted_dimensions": [
                {
                    "dimension": int(index),
                    "standardized_shift": float(standardized_shift[index]),
                }
                for index in top_dimensions
            ],
            "interpretation": (
                "silhouette near 0 and small centroid distances indicate exchangeable "
                "batches; large silhouette or Mahalanobis distances indicate a domain "
                "shift that must be addressed before pooling training cohorts"
            ),
        },
    )


def ihc_quant() -> None:
    settings = settings_env("PATHOLOGY_IHC_SETTINGS")
    slide_path = input_paths(0)[0]
    mask_path = input_paths(1)[0]
    if mask_path.suffix.lower() not in IMAGE_SUFFIXES:
        raise RuntimeError("IHC ROI mask must be a raster image (PNG/TIFF)")
    slide = open_slide(slide_path)
    summary = slide_summary(slide, slide_path)

    mask = np.asarray(Image.open(mask_path))
    if mask.ndim == 3:
        mask = mask[..., 0]
    mask_labels = sorted(int(value) for value in np.unique(mask) if value != 0)
    roi_label = int(settings.get("roi_label", mask_labels[0] if mask_labels else 1))
    roi = mask == roi_label
    if not roi.any():
        raise RuntimeError(f"ROI label {roi_label} is empty in the mask")

    mask_downsample = float(settings.get("mask_downsample", 16.0))
    if not (mask_downsample >= 1.0):
        raise RuntimeError("mask_downsample must be >= 1")
    dab_weak = float(settings.get("dab_weak_threshold", 0.15))
    dab_strong = float(settings.get("dab_strong_threshold", 0.35))
    if not (0.0 < dab_weak < dab_strong):
        raise RuntimeError("thresholds must satisfy 0 < dab_weak < dab_strong")
    tile_size = positive_int(settings, "tile_size", 512, minimum=64)
    max_tiles = positive_int(settings, "max_tiles", 20000)

    mask_y, mask_x = np.nonzero(roi)
    x0, x1 = int(mask_x.min()), int(mask_x.max())
    y0, y1 = int(mask_y.min()), int(mask_y.max())
    level0_x0, level0_y0 = int(x0 * mask_downsample), int(y0 * mask_downsample)
    level0_x1, level0_y1 = int((x1 + 1) * mask_downsample), int((y1 + 1) * mask_downsample)
    analysis_level = pick_level(slide, float(settings.get("max_downsample", 4.0)))
    downsample = slide.level_downsamples[analysis_level]

    rows: list[dict[str, Any]] = []
    dab_all: list[np.ndarray] = []
    for ty in range(level0_y0, level0_y1, int(tile_size * downsample)):
        for tx in range(level0_x0, level0_x1, int(tile_size * downsample)):
            if len(rows) >= max_tiles:
                break
            mask_tx0 = int(tx / mask_downsample)
            mask_ty0 = int(ty / mask_downsample)
            mask_tx1 = int((tx + tile_size * downsample) / mask_downsample)
            mask_ty1 = int((ty + tile_size * downsample) / mask_downsample)
            mask_window = roi[
                max(0, mask_ty0) : min(roi.shape[0], mask_ty1),
                max(0, mask_tx0) : min(roi.shape[1], mask_tx1),
            ]
            if mask_window.mean() < 0.1:
                continue
            level_tx = int(tx / downsample)
            level_ty = int(ty / downsample)
            level_width, level_height = slide.level_dimensions[analysis_level]
            if level_tx + tile_size > level_width or level_ty + tile_size > level_height:
                read_w = min(tile_size, level_width - level_tx)
                read_h = min(tile_size, level_height - level_ty)
                if read_w < 32 or read_h < 32:
                    continue
            else:
                read_w = read_h = tile_size
            rgb = read_rgb(slide, (tx, ty), analysis_level, (read_w, read_h))
            mask_resized = np.asarray(
                Image.fromarray((mask_window * 255).astype(np.uint8)).resize(
                    (read_w, read_h), Image.NEAREST
                )
            ) > 127
            if not mask_resized.any():
                continue
            hed = rgb2hed(rgb.astype(np.float32) / 255.0)
            dab = hed[..., 2][mask_resized]
            dab_all.append(dab)
            weak = float(((dab > dab_weak) & (dab <= dab_strong)).mean())
            strong = float((dab > dab_strong).mean())
            rows.append(
                {
                    "tile_x_level0": int(tx),
                    "tile_y_level0": int(ty),
                    "level": analysis_level,
                    "n_pixels": int(mask_resized.sum()),
                    "positive_fraction": float((dab > dab_weak).mean()),
                    "weak_fraction": weak,
                    "strong_fraction": strong,
                    "mean_dab": float(dab.mean()),
                    "mean_hematoxylin": float(hed[..., 0][mask_resized].mean()),
                }
            )

    if not rows:
        raise RuntimeError("IHC quantification sampled no tiles inside the ROI")
    stacked = np.concatenate(dab_all)
    weak_pct = float(((stacked > dab_weak) & (stacked <= dab_strong)).mean()) * 100.0
    strong_pct = float((stacked > dab_strong).mean()) * 100.0
    moderate_pct = float(
        (
            (stacked > dab_weak + 0.5 * (dab_strong - dab_weak))
            & (stacked <= dab_strong)
        ).mean()
    ) * 100.0
    weak_only_pct = max(0.0, weak_pct - moderate_pct)
    h_score = 100.0 * (3.0 * strong_pct / 100.0 + 2.0 * moderate_pct / 100.0 + weak_only_pct / 100.0)
    pd.DataFrame(rows).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "slide": summary,
            "roi_label": roi_label,
            "mask_labels": mask_labels,
            "mask_downsample": mask_downsample,
            "mask_hash": sha256(mask_path),
            "analysis_level": analysis_level,
            "dab_weak_threshold": dab_weak,
            "dab_strong_threshold": dab_strong,
            "n_tiles": len(rows),
            "n_pixels": int(stacked.size),
            "positive_fraction": float((stacked > dab_weak).mean()),
            "weak_fraction": weak_only_pct / 100.0,
            "moderate_fraction": moderate_pct / 100.0,
            "strong_fraction": strong_pct / 100.0,
            "h_score": h_score,
            "mean_dab": float(stacked.mean()),
            "settings": settings,
        },
    )


def qupath_import() -> None:
    settings = settings_env("PATHOLOGY_QUPATH_SETTINGS")
    mask_path = input_paths(0)[0]
    slide_path = input_paths(1)[0]
    slide = open_slide(slide_path)
    summary = slide_summary(slide, slide_path)

    mask = np.asarray(Image.open(mask_path))
    if mask.ndim == 3:
        mask = mask[..., 0]
    labels = sorted(int(value) for value in np.unique(mask) if value != 0)
    if not labels:
        raise RuntimeError("annotation mask has no positive labels")

    mask_downsample = float(settings.get("mask_downsample", 16.0))
    if not (mask_downsample >= 1.0):
        raise RuntimeError("mask_downsample must be >= 1")
    label_names_raw = settings.get("label_names", {})
    label_names = {int(key): str(value) for key, value in dict(label_names_raw).items()}
    simplify_tolerance = float(settings.get("simplify_tolerance_px", 2.0))
    if not (simplify_tolerance >= 0.0):
        raise RuntimeError("simplify_tolerance_px must be nonnegative")
    min_region_px = positive_int(settings, "min_region_px", 200, minimum=0)

    features: list[dict[str, Any]] = []
    region_counts: dict[str, int] = {}
    for label in labels:
        binary = mask == label
        if min_region_px > 0:
            binary = remove_small_objects(binary, min_size=min_region_px)
        if not binary.any():
            region_counts[str(label)] = 0
            continue
        contours = find_contours(binary.astype(float), 0.5)
        name = label_names.get(label, f"label_{label}")
        count = 0
        for contour in contours:
            if simplify_tolerance > 0:
                contour = approximate_polygon(contour, tolerance=simplify_tolerance)
            if contour is None or len(contour) < 4:
                continue
            polygon = [
                [float(x * mask_downsample), float(y * mask_downsample)]
                for y, x in contour
            ]
            if polygon[0] != polygon[-1]:
                polygon.append(polygon[0])
            features.append(
                {
                    "type": "Feature",
                    "properties": {
                        "classification": {"name": name},
                        "isLocked": False,
                    },
                    "geometry": {"type": "Polygon", "coordinates": [polygon]},
                }
            )
            count += 1
        region_counts[name] = count

    if not features:
        raise RuntimeError("annotation mask produced no polygons; check min_region_px")
    collection = {
        "type": "FeatureCollection",
        "features": features,
        # QuPath legacy GeoJSON: coordinates are level-0 image pixels (microns
        # per pixel in the slide metadata), y grows downward.
        "metadata": {
            "coordinate_system": "level0_image_pixels",
            "source_slide_dimensions": summary["dimensions"],
        },
    }
    output_dir(0).write_text(json.dumps(collection) + "\n")
    write_json(
        1,
        {
            "slide": summary,
            "mask_hash": sha256(mask_path),
            "mask_downsample": mask_downsample,
            "labels": labels,
            "label_names": {str(key): value for key, value in label_names.items()},
            "regions_per_label": region_counts,
            "n_features": len(features),
            "simplify_tolerance_px": simplify_tolerance,
            "holes_not_modeled": True,
            "import_hint": (
                "QuPath: drag the .geojson onto the slide image or use "
                "Objects > Import objects; coordinates are level-0 pixels"
            ),
        },
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "command",
        choices=[
            "wsi-ingest",
            "wsi-qc",
            "patch-sample",
            "wsi-embed",
            "domain-check",
            "ihc-quant",
            "qupath-import",
        ],
    )
    args = parser.parse_args()
    commands = {
        "wsi-ingest": wsi_ingest,
        "wsi-qc": wsi_qc,
        "patch-sample": patch_sample,
        "wsi-embed": wsi_embed,
        "domain-check": domain_check,
        "ihc-quant": ihc_quant,
        "qupath-import": qupath_import,
    }
    commands[args.command]()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"pathology_runner: {error}", file=sys.stderr)
        raise
