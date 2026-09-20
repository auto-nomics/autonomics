#!/usr/bin/env python3
"""Radiomics Stage-A container entrypoint.

The container reads paths from AUTONOMICS_INPUT*, writes only declared files
under /work, and never fetches remote data. Command names intentionally match
the Rust node kinds one-to-one.
"""

from __future__ import annotations

import argparse
import hashlib
import gzip
import json
import math
import os
import shutil
from pathlib import Path
import platform
import re
import sys
import tempfile
import zipfile
from dataclasses import dataclass
from typing import Any, Sequence

import numpy as np
import pandas as pd
import pydicom
import SimpleITK as sitk
from scipy import ndimage
from scipy.spatial import ConvexHull


SUPPORTED_IMAGE_SUFFIXES = {".nii", ".nii.gz", ".mha", ".mhd", ".nrrd"}
RTSTRUCT_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.481.3"
DICOM_SORT_MODES = {"lexical", "instance_number", "position"}
DICOM_SORT_DIRECTIONS = {"ascending", "descending"}


@dataclass(frozen=True)
class DicomSeriesOrder:
    files: tuple[Path, ...]
    sop_instance_uid_to_index: dict[str, int]

    def metadata(self) -> dict[str, Any]:
        return {
            "z_sort_files": [str(path) for path in self.files],
            "sop_instance_uids": list(self.sop_instance_uid_to_index),
        }


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


def is_supported_image(path: Path) -> bool:
    lower = str(path).lower()
    return any(lower.endswith(suffix) for suffix in SUPPORTED_IMAGE_SUFFIXES)


def is_dicom_file(path: Path) -> bool:
    if path.suffix.lower() == ".dcm":
        return True
    try:
        with path.open("rb") as stream:
            stream.seek(128)
            return stream.read(4) == b"DICM"
    except OSError:
        return False


def dicom_order_settings(settings: dict[str, Any] | None = None) -> tuple[str, str]:
    settings = settings or {}
    z_sort = str(settings.get("z_sort", "position")).lower()
    z_direction = str(settings.get("z_direction", "ascending")).lower()
    if z_sort not in DICOM_SORT_MODES:
        raise RuntimeError(
            f"unsupported DICOM z_sort `{z_sort}`; expected one of {sorted(DICOM_SORT_MODES)}"
        )
    if z_direction not in DICOM_SORT_DIRECTIONS:
        raise RuntimeError(
            f"unsupported DICOM z_direction `{z_direction}`; expected one of "
            f"{sorted(DICOM_SORT_DIRECTIONS)}"
        )
    return z_sort, z_direction


def _finite_floats(value: Any, name: str, size: int | None = None) -> np.ndarray:
    try:
        values = np.asarray(value, dtype=float)
    except (TypeError, ValueError) as error:
        raise RuntimeError(f"DICOM {name} is not numeric") from error
    if size is not None and values.size != size:
        raise RuntimeError(f"DICOM {name} has {values.size} values; expected {size}")
    if not np.isfinite(values).all():
        raise RuntimeError(f"DICOM {name} contains a non-finite value")
    return values.reshape(-1)


def prepare_dicom_series(
    paths: Sequence[Path], settings: dict[str, Any] | None = None
) -> DicomSeriesOrder:
    """Order classic single-frame slices without relying on file names."""
    z_sort, z_direction = dicom_order_settings(settings)
    reverse = z_direction == "descending"
    rows: list[tuple[Path, pydicom.Dataset]] = [
        (path, pydicom.dcmread(str(path), stop_before_pixels=True)) for path in paths
    ]

    if z_sort == "lexical":
        rows.sort(key=lambda row: str(row[0]), reverse=reverse)
    elif z_sort == "instance_number":
        keyed_rows: list[tuple[float, str, Path, pydicom.Dataset]] = []
        for path, dataset in rows:
            if "InstanceNumber" not in dataset:
                raise RuntimeError(
                    f"DICOM instance-number sorting requires InstanceNumber: `{path}`"
                )
            keyed_rows.append((float(dataset.InstanceNumber), str(path), path, dataset))
        keyed_rows.sort(key=lambda row: row[:2], reverse=reverse)
        rows = [(path, dataset) for _, _, path, dataset in keyed_rows]
    else:
        normal: np.ndarray | None = None
        keyed_rows: list[tuple[float, str, Path, pydicom.Dataset]] = []
        for path, dataset in rows:
            if "ImagePositionPatient" not in dataset or "ImageOrientationPatient" not in dataset:
                raise RuntimeError(
                    f"DICOM position sorting requires ImagePositionPatient and "
                    f"ImageOrientationPatient: `{path}`"
                )
            position = _finite_floats(dataset.ImagePositionPatient, "ImagePositionPatient", 3)
            orientation = _finite_floats(
                dataset.ImageOrientationPatient, "ImageOrientationPatient", 6
            )
            row_axis, column_axis = orientation[:3], orientation[3:]
            slice_normal = np.cross(row_axis, column_axis)
            normal_length = float(np.linalg.norm(slice_normal))
            if normal_length < 1e-6:
                raise RuntimeError(f"DICOM ImageOrientationPatient is degenerate: `{path}`")
            slice_normal /= normal_length
            if normal is None:
                normal = slice_normal
            elif float(np.dot(slice_normal, normal)) < 1.0 - 1e-6:
                raise RuntimeError("DICOM series contains inconsistent slice orientations")
            keyed_rows.append((float(position @ normal), str(path), path, dataset))
        keyed_rows.sort(key=lambda row: row[:2], reverse=reverse)
        for index in range(1, len(keyed_rows)):
            if abs(keyed_rows[index][0] - keyed_rows[index - 1][0]) < 1e-5:
                raise RuntimeError("DICOM series contains duplicate slice positions")
        rows = [(path, dataset) for _, _, path, dataset in keyed_rows]

    uid_to_index: dict[str, int] = {}
    series_uids = {
        str(dataset.SeriesInstanceUID)
        for _, dataset in rows
        if "SeriesInstanceUID" in dataset
    }
    if len(series_uids) > 1:
        raise RuntimeError("multi-file image input contains more than one SeriesInstanceUID")
    for index, (_, dataset) in enumerate(rows):
        if "SOPInstanceUID" not in dataset:
            raise RuntimeError("every DICOM slice must provide SOPInstanceUID")
        uid = str(dataset.SOPInstanceUID)
        if uid in uid_to_index:
            raise RuntimeError(f"duplicate SOPInstanceUID `{uid}` in DICOM series")
        uid_to_index[uid] = index
    return DicomSeriesOrder(tuple(path for path, _ in rows), uid_to_index)


def read_single_image(path: Path) -> sitk.Image:
    if is_dicom_file(path):
        reader = sitk.ImageFileReader()
        reader.SetFileName(str(path))
        return reader.Execute()

    if is_supported_image(path):
        reader = sitk.ImageFileReader()
        reader.SetFileName(str(path))
        return reader.Execute()

    # Older staging names could reduce `.nii.gz` to `.gz`; SimpleITK's factory
    # cannot infer NIfTI from that filename, so expand it to a recoverable
    # temporary file with the complete extension.
    if path.suffix.lower() == ".gz":
        descriptor, temporary_name = tempfile.mkstemp(suffix=".nii.gz")
        os.close(descriptor)
        temporary = Path(temporary_name)
        try:
            with gzip.open(path, "rb") as source, temporary.open("wb") as target:
                shutil.copyfileobj(source, target)
            return read_single_image(temporary)
        finally:
            temporary.unlink(missing_ok=True)

    raise RuntimeError(
        f"unsupported image file `{path}`; expected NIfTI, MHA/NRRD, or a DICOM file"
    )


def read_dicom_series(files: Sequence[Path]) -> sitk.Image:
    reader = sitk.ImageSeriesReader()
    reader.SetFileNames([str(path) for path in files])
    return reader.Execute()


def read_image(
    paths: Sequence[Path], settings: dict[str, Any] | None = None
) -> sitk.Image:
    # Container staging may shorten `.nii.gz` to a generic `.gz` suffix. A
    # single file is always decoded through the format-sniffing ImageFileReader
    # rather than the DICOM-only series reader.
    if len(paths) == 1:
        return read_single_image(paths[0])

    dicom_files = [path for path in paths if path.is_file() and is_dicom_file(path)]
    if len(dicom_files) != len(paths):
        non_dicom = [str(path) for path in paths if path not in dicom_files]
        raise RuntimeError(
            "multi-file image input must be a homogeneous DICOM series; got: "
            + ", ".join(non_dicom)
        )
    return read_dicom_series(prepare_dicom_series(dicom_files, settings).files)


def image_metadata(image: sitk.Image, source: Sequence[Path]) -> dict[str, Any]:
    return {
        "size": list(image.GetSize()),
        "spacing": list(image.GetSpacing()),
        "origin": list(image.GetOrigin()),
        "direction": list(image.GetDirection()),
        "pixel_type": image.GetPixelIDTypeAsString(),
        "source_hashes": [sha256(path) for path in source],
        "simpleitk_version": sitk.Version_VersionString(),
    }


def write_json(index: int, payload: dict[str, Any]) -> None:
    output_dir(index).write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def ingest_image() -> None:
    paths = input_paths(0)
    settings = json.loads(os.environ.get("RADIOMICS_IMAGE_SETTINGS", "{}"))
    dicom_series = (
        prepare_dicom_series(paths, settings)
        if len(paths) > 1 and all(path.is_file() and is_dicom_file(path) for path in paths)
        else None
    )
    image = read_dicom_series(dicom_series.files) if dicom_series else read_image(paths, settings)
    metadata = image_metadata(image, paths)
    if dicom_series:
        metadata["dicom_series"] = dicom_series.metadata()
    sitk.WriteImage(image, str(output_dir(0)), True)
    write_json(1, metadata)


def select_roi(dataset: pydicom.Dataset, roi_name: str | None) -> tuple[str, str]:
    rows = list(getattr(dataset, "StructureSetROISequence", []))
    if not rows:
        raise RuntimeError("RTSTRUCT has no StructureSetROISequence")
    for row in rows:
        name = str(getattr(row, "ROIName", "")).strip()
        if roi_name and name == roi_name:
            return str(row.ROINumber), name
    if roi_name:
        raise RuntimeError(f"ROI `{roi_name}` was not found in RTSTRUCT")
    row = rows[0]
    return str(row.ROINumber), str(getattr(row, "ROIName", "ROI")).strip()


def contour_reference_uids(contour: pydicom.Dataset) -> list[str]:
    return [
        str(item.ReferencedSOPInstanceUID)
        for item in getattr(contour, "ContourImageSequence", [])
        if "ReferencedSOPInstanceUID" in item
    ]


def rtstruct_to_mask(
    rtstruct_path: Path,
    reference: sitk.Image,
    reference_series: DicomSeriesOrder | None,
    roi_name: str | None,
) -> tuple[sitk.Image, dict[str, Any]]:
    dataset = pydicom.dcmread(str(rtstruct_path), stop_before_pixels=True)
    roi_number, selected_roi_name = select_roi(dataset, roi_name)
    contours: list[pydicom.Dataset] = []
    for roi_contour in getattr(dataset, "ROIContourSequence", []):
        if str(getattr(roi_contour, "ReferencedROINumber", "")) != roi_number:
            continue
        contours.extend(getattr(roi_contour, "ContourSequence", []))
    if not contours:
        raise RuntimeError(f"ROI `{selected_roi_name}` has no contours")

    from skimage.draw import polygon

    array = np.zeros(reference.GetSize()[::-1], dtype=np.uint8)
    mapped_by_uid = 0
    mapped_geometrically = 0
    uid_mapping: dict[int, str] = {}
    for contour in contours:
        points = np.asarray(contour.ContourData, dtype=float).reshape(-1, 3)
        if not np.isfinite(points).all():
            raise RuntimeError(f"ROI `{selected_roi_name}` contains non-finite contour points")
        continuous = np.asarray(
            [
                reference.TransformPhysicalPointToContinuousIndex(tuple(point))
                for point in points
            ]
        )
        reference_uids = contour_reference_uids(contour)
        if reference_uids:
            indexes = {
                reference_series.sop_instance_uid_to_index[uid]
                for uid in reference_uids
                if reference_series is not None
                and uid in reference_series.sop_instance_uid_to_index
            }
            unknown_uids = [
                uid
                for uid in reference_uids
                if reference_series is None
                or uid not in reference_series.sop_instance_uid_to_index
            ]
            if unknown_uids:
                raise RuntimeError(
                    f"ROI `{selected_roi_name}` references SOPInstanceUID(s) absent from the "
                    f"reference series: {', '.join(unknown_uids)}"
                )
            if len(indexes) != 1:
                raise RuntimeError(
                    f"ROI `{selected_roi_name}` contour maps to multiple reference slices"
                )
            slice_index = next(iter(indexes))
            if np.max(np.abs(continuous[:, 2] - slice_index)) > 0.51:
                raise RuntimeError(
                    f"ROI `{selected_roi_name}` contour does not lie on referenced slice "
                    f"{slice_index}"
                )
            mapped_by_uid += 1
            uid_mapping[slice_index] = reference_uids[0]
        else:
            slice_values = continuous[:, 2]
            slice_index = int(round(float(np.median(slice_values))))
            if slice_index < 0 or slice_index >= array.shape[0]:
                raise RuntimeError(
                    f"ROI `{selected_roi_name}` contour is outside reference slice bounds"
                )
            if np.max(np.abs(slice_values - slice_index)) > 0.51:
                raise RuntimeError(
                    f"ROI `{selected_roi_name}` contour is oblique to the reference z axis"
                )
            mapped_geometrically += 1
        if np.any(continuous[:, 0] < -0.5) or np.any(continuous[:, 0] > array.shape[2] - 0.5):
            raise RuntimeError(
                f"ROI `{selected_roi_name}` contour is outside reference column bounds"
            )
        if np.any(continuous[:, 1] < -0.5) or np.any(continuous[:, 1] > array.shape[1] - 0.5):
            raise RuntimeError(
                f"ROI `{selected_roi_name}` contour is outside reference row bounds"
            )
        rows, columns = polygon(
            continuous[:, 1],
            continuous[:, 0],
            shape=array.shape[1:],
        )
        array[slice_index, rows, columns] = 1
    mask = sitk.GetImageFromArray(array)
    mask.CopyInformation(reference)
    voxel_count = int(np.count_nonzero(array))
    if voxel_count == 0:
        raise RuntimeError(
            f"ROI `{selected_roi_name}` rasterized to an empty mask; "
            f"contours={len(contours)}, mapped_by_uid={mapped_by_uid}, "
            f"mapped_geometrically={mapped_geometrically}"
        )
    return mask, {
        "roi_number": roi_number,
        "roi_name": selected_roi_name,
        "voxel_count": voxel_count,
        "contour_count": len(contours),
        "contours_mapped_by_sop_instance_uid": mapped_by_uid,
        "contours_mapped_geometrically": mapped_geometrically,
        "slice_to_sop_instance_uid": dict(sorted(uid_mapping.items())),
    }


def ingest_mask() -> None:
    image_sources = input_paths(0)
    mask_sources = input_paths(1)
    settings = json.loads(os.environ.get("RADIOMICS_MASK_SETTINGS", "{}"))
    reference_series = None
    reference_is_dicom_series = len(image_sources) > 1 and all(
        path.is_file() and is_dicom_file(path) for path in image_sources
    )
    if reference_is_dicom_series:
        reference_series = prepare_dicom_series(image_sources, settings)
        reference = read_dicom_series(reference_series.files)
    else:
        reference = read_image(image_sources, settings)
    roi_metadata: dict[str, Any] = {}
    if len(mask_sources) == 1 and not is_dicom_file(mask_sources[0]):
        mask = read_single_image(mask_sources[0])
    else:
        candidates = [path for path in mask_sources if path.suffix.lower() == ".dcm"]
        rtstruct = None
        for candidate in candidates:
            dataset = pydicom.dcmread(str(candidate), stop_before_pixels=True)
            if str(getattr(dataset, "SOPClassUID", "")) == RTSTRUCT_SOP_CLASS_UID:
                rtstruct = candidate
                break
        if rtstruct is None:
            raise RuntimeError("mask ingestion requires NIfTI/MHA or DICOM RTSTRUCT")
        mask, roi_metadata = rtstruct_to_mask(
            rtstruct, reference, reference_series, settings.get("roi_name")
        )
    voxel_count = int(np.count_nonzero(sitk.GetArrayFromImage(mask)))
    if voxel_count == 0:
        raise RuntimeError("mask ingestion produced an empty mask")
    roi_metadata["voxel_count"] = voxel_count
    sitk.WriteImage(mask, str(output_dir(0)), True)
    write_json(
        1,
        {
            "mask": image_metadata(mask, mask_sources),
            "reference_image": image_metadata(reference, image_sources),
            "reference_dicom_series": (
                reference_series.metadata() if reference_series is not None else None
            ),
            **roi_metadata,
        },
    )


def close(left: Sequence[float], right: Sequence[float], tolerance: float) -> bool:
    return len(left) == len(right) and all(
        math.isfinite(float(a)) and math.isfinite(float(b)) and abs(float(a) - float(b)) <= tolerance
        for a, b in zip(left, right)
    )


def validate_pair() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_VALIDATE_SETTINGS", "{}"))
    tolerance = float(settings.get("geometry_tolerance_mm", 0.01))
    label = int(settings.get("mask_label", 1))
    minimum_voxels = int(settings.get("minimum_mask_voxels", 1))
    image = read_single_image(input_paths(0)[0])
    mask = read_single_image(input_paths(1)[0])

    checks = {
        "dimensions_match": image.GetDimension() == mask.GetDimension(),
        "size_match": list(image.GetSize()) == list(mask.GetSize()),
        "spacing_match": close(image.GetSpacing(), mask.GetSpacing(), tolerance),
        "origin_match": close(image.GetOrigin(), mask.GetOrigin(), tolerance),
        "direction_match": close(image.GetDirection(), mask.GetDirection(), tolerance),
    }
    labels: list[int] = []
    voxel_count = 0
    if all(checks.values()):
        array = sitk.GetArrayFromImage(mask)
        labels = sorted(int(value) for value in np.unique(array) if value != 0)
        voxel_count = int(np.count_nonzero(array == label))
    checks["mask_label_present"] = label in labels if labels else False
    checks["minimum_mask_voxels"] = voxel_count >= minimum_voxels
    valid = all(checks.values())
    row = {
        "extraction_id": settings.get("extraction_id", "unknown"),
        "status": "valid" if valid else "invalid",
        "image_size": json.dumps(list(image.GetSize())),
        "mask_size": json.dumps(list(mask.GetSize())),
        "image_spacing": json.dumps(list(image.GetSpacing())),
        "mask_spacing": json.dumps(list(mask.GetSpacing())),
        "mask_labels": json.dumps(labels),
        "mask_label": label,
        "mask_voxel_count": voxel_count,
        **{key: bool(value) for key, value in checks.items()},
        "error_code": "" if valid else ";".join(key for key, value in checks.items() if not value),
    }
    pd.DataFrame([row]).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "valid": valid,
            "checks": checks,
            "geometry_tolerance_mm": tolerance,
            "image_hash": sha256(input_paths(0)[0]),
            "mask_hash": sha256(input_paths(1)[0]),
        },
    )


def make_resampling_reference(image: sitk.Image, spacing: Sequence[float]) -> sitk.Image:
    old_spacing = image.GetSpacing()
    original_size = image.GetSize()
    size = [max(1, int(round(original_size[i] * old_spacing[i] / spacing[i]))) for i in range(3)]
    reference = sitk.Image(size, sitk.sitkFloat32)
    reference.SetSpacing([float(value) for value in spacing])
    reference.SetOrigin(image.GetOrigin())
    reference.SetDirection(image.GetDirection())
    return reference


def preprocess() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_PREPROCESS_SETTINGS", "{}"))
    image = read_single_image(input_paths(0)[0])
    mask = read_single_image(input_paths(1)[0])
    if list(image.GetSize()) != list(mask.GetSize()) or not close(
        image.GetSpacing(), mask.GetSpacing(), 0.01
    ):
        raise RuntimeError("preprocessing requires an already validated image/mask pair")

    resegment = settings.get("resegment_range")
    if resegment:
        low, high = map(float, resegment)
        mask_array = sitk.GetArrayFromImage(mask)
        image_array = sitk.GetArrayFromImage(image)
        mask_array[(image_array < low) | (image_array > high)] = 0
        mask = sitk.GetImageFromArray(mask_array)
        mask.CopyInformation(image)

    spacing = settings.get("resampled_spacing")
    if spacing:
        reference = make_resampling_reference(image, spacing)
        interpolator = settings.get("interpolator", "sitkBSpline")
        image = sitk.Resample(
            image,
            reference,
            sitk.Transform(),
            getattr(sitk, interpolator),
            float(image.GetPixelIDValue()) * 0.0,
            image.GetPixelID(),
        )
        mask = sitk.Resample(
            mask,
            reference,
            sitk.Transform(),
            sitk.sitkNearestNeighbor,
            0,
            sitk.sitkUInt8,
        )

    if settings.get("normalize", False):
        image_array = sitk.GetArrayFromImage(image).astype(np.float32)
        mask_array = sitk.GetArrayFromImage(mask)
        values = image_array[mask_array > 0]
        if values.size == 0:
            raise RuntimeError("cannot normalize an empty ROI")
        mean, std = float(values.mean()), float(values.std())
        if std == 0:
            raise RuntimeError("cannot normalize an ROI with constant intensity")
        image = sitk.GetImageFromArray(((image_array - mean) / std).astype(np.float32))
        image.CopyInformation(mask)

    sitk.WriteImage(image, str(output_dir(0)), True)
    sitk.WriteImage(mask, str(output_dir(1)), True)
    write_json(
        2,
        {
            "settings": settings,
            "image": image_metadata(image, input_paths(0)),
            "mask": image_metadata(mask, input_paths(1)),
        },
    )


def json_safe(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, (bytes, bytearray)):
        return f"<{len(value)} bytes>"
    if isinstance(value, pydicom.multival.MultiValue):
        return [json_safe(item) for item in value]
    if isinstance(value, pydicom.valuerep.PersonName):
        return str(value)
    if isinstance(value, pydicom.Dataset):
        return {str(element.keyword or element.tag): json_safe(element.value) for element in value}
    if hasattr(value, "original_string"):
        try:
            return float(value)
        except (TypeError, ValueError):
            return str(value)
    if isinstance(value, Sequence):
        return [json_safe(item) for item in value]
    return str(value)


DICOM_METADATA_FIELDS = [
    "SOPClassUID",
    "SOPInstanceUID",
    "PatientID",
    "StudyInstanceUID",
    "SeriesInstanceUID",
    "FrameOfReferenceUID",
    "Modality",
    "StudyDate",
    "StudyTime",
    "AcquisitionDate",
    "AcquisitionTime",
    "Manufacturer",
    "ManufacturerModelName",
    "StationName",
    "SoftwareVersions",
    "InstitutionName",
    "BodyPartExamined",
    "PatientPosition",
    "KVP",
    "XRayTubeCurrent",
    "Exposure",
    "ExposureTime",
    "ConvolutionKernel",
    "ReconstructionDiameter",
    "SliceThickness",
    "SpacingBetweenSlices",
    "PixelSpacing",
    "ImagePositionPatient",
    "ImageOrientationPatient",
    "RescaleSlope",
    "RescaleIntercept",
    "CTDIvol",
    "DLP",
    "EffectiveDose",
]


def dicom_metadata() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_DICOM_METADATA_SETTINGS", "{}"))
    rows: list[dict[str, Any]] = []
    for path in input_paths(0):
        dataset = pydicom.dcmread(str(path), stop_before_pixels=True, force=False)
        row: dict[str, Any] = {
            "source_path": str(path),
            "source_hash": sha256(path),
        }
        for keyword in DICOM_METADATA_FIELDS + list(settings.get("extra_tags", [])):
            if keyword in row:
                continue
            value = dataset.get(keyword)
            if value is None and re.fullmatch(r"([0-9A-Fa-f]{4},[0-9A-Fa-f]{4})", keyword):
                group, element = keyword.split(",")
                value = dataset.get(pydicom.tag.Tag(int(group, 16), int(element, 16)))
            if isinstance(value, pydicom.DataElement):
                value = value.value
            row[keyword] = json_safe(value)
        rows.append(row)
    if not rows:
        raise RuntimeError("DICOM metadata input is empty")
    pd.DataFrame(rows).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "n_files": len(rows),
            "fields": list(pd.DataFrame(rows).columns),
            "source_hashes": [row["source_hash"] for row in rows],
        },
    )


PHI_KEYWORDS = [
    "PatientName",
    "PatientID",
    "PatientBirthDate",
    "PatientBirthTime",
    "PatientSex",
    "PatientAge",
    "PatientWeight",
    "PatientSize",
    "PatientAddress",
    "PatientTelephoneNumbers",
    "PatientMotherBirthName",
    "PatientInsurancePlanCode",
    "OtherPatientIDs",
    "OtherPatientNames",
    "OtherPatientIDsSequence",
    "EthnicGroup",
    "CountryOfResidence",
    "RegionOfResidence",
    "InstitutionName",
    "InstitutionAddress",
    "InstitutionalDepartmentName",
    "StationName",
    "OperatorsName",
    "ReferringPhysicianName",
    "PerformingPhysicianName",
    "NameOfPhysiciansReadingStudy",
    "RequestingPhysician",
    "PersonName",
]


def delete_phi(dataset: pydicom.Dataset, keep_patient_id: bool, pseudonym: str | None) -> list[str]:
    removed: list[str] = []
    for element in list(dataset):
        keyword = str(element.keyword or element.tag)
        if keyword in PHI_KEYWORDS or element.VR == "PN":
            if keep_patient_id and keyword == "PatientID":
                if pseudonym:
                    element.value = pseudonym
                continue
            if pseudonym and keyword in {"PatientID", "PatientName"}:
                element.value = pseudonym
                continue
            del dataset[keyword]
            removed.append(keyword)
        elif isinstance(element.value, pydicom.Dataset):
            removed.extend(delete_phi(element.value, keep_patient_id, pseudonym))
        elif element.VR == "SQ":
            for item in element.value:
                removed.extend(delete_phi(item, keep_patient_id, pseudonym))
    return sorted(set(removed))


def phi_scrub() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_PHI_SCRUB_SETTINGS", "{}"))
    keep_patient_id = bool(settings.get("keep_patient_id", False))
    pseudonym = settings.get("pseudonym")
    report: list[dict[str, Any]] = []
    archive_path = output_path(0)
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in input_paths(0):
            dataset = pydicom.dcmread(str(path), stop_before_pixels=False, force=False)
            removed = delete_phi(dataset, keep_patient_id, pseudonym)
            member = f"{len(report):06d}-{path.name}"
            with tempfile.NamedTemporaryFile(suffix=".dcm", delete=False) as temporary:
                temporary_name = temporary.name
            temporary_path = Path(temporary_name)
            try:
                dataset.save_as(str(temporary_path))
                archive.write(temporary_path, member)
            finally:
                temporary_path.unlink(missing_ok=True)
            report.append(
                {
                    "source_path": str(path),
                    "source_hash": sha256(path),
                    "scrubbed_member": member,
                    "removed_tags": json.dumps(removed),
                    "n_removed_tags": len(removed),
                }
            )
    pd.DataFrame(report).to_parquet(output_dir(1), index=False)
    write_json(
        2,
        {
            "n_files": len(report),
            "keep_patient_id": keep_patient_id,
            "pseudonym_applied": bool(pseudonym),
            "n_removed_tags_total": sum(row["n_removed_tags"] for row in report),
        },
    )


def binary_mask(mask: sitk.Image, label: int) -> sitk.Image:
    return sitk.Cast(mask == label, sitk.sitkUInt8)


def require_same_geometry(left: sitk.Image, right: sitk.Image, names: tuple[str, str]) -> None:
    checks = (
        left.GetDimension() == right.GetDimension()
        and left.GetSize() == right.GetSize()
        and close(left.GetSpacing(), right.GetSpacing(), 0.01)
        and close(left.GetOrigin(), right.GetOrigin(), 0.01)
        and close(left.GetDirection(), right.GetDirection(), 0.01)
    )
    if not checks:
        raise RuntimeError(
            f"{names[0]} and {names[1]} must share geometry before comparison"
        )


def voi_similarity() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_VOI_SETTINGS", "{}"))
    label = int(settings.get("mask_label", 1))
    surface_tolerance = float(settings.get("surface_tolerance_mm", 1.0))
    left_mask = binary_mask(read_single_image(input_paths(0)[0]), label)
    right_mask = binary_mask(read_single_image(input_paths(1)[0]), label)
    require_same_geometry(left_mask, right_mask, ("reference mask", "comparison mask"))
    left_array = sitk.GetArrayFromImage(left_mask) > 0
    right_array = sitk.GetArrayFromImage(right_mask) > 0
    left_count, right_count = int(left_array.sum()), int(right_array.sum())
    intersection, union = int((left_array & right_array).sum()), int((left_array | right_array).sum())
    dice = 2 * intersection / (left_count + right_count) if left_count + right_count else 1.0
    jaccard = intersection / union if union else 1.0
    volume_similarity = (
        1.0 - abs(left_count - right_count) / (left_count + right_count)
        if left_count + right_count
        else 1.0
    )
    hausdorff = sitk.HausdorffDistanceImageFilter()
    hausdorff.Execute(left_mask, right_mask)
    directed_left = hausdorff.GetHausdorffDistance()
    hausdorff_reverse = sitk.HausdorffDistanceImageFilter()
    hausdorff_reverse.Execute(right_mask, left_mask)
    directed_right = hausdorff_reverse.GetHausdorffDistance()

    def surface_coverage(surface: sitk.Image, mask: sitk.Image) -> float:
        if int((sitk.GetArrayFromImage(surface) > 0).sum()) == 0:
            return 1.0
        distance = sitk.SignedMaurerDistanceMap(
            mask, insideIsPositive=False, useImageSpacing=True
        )
        surface_array = sitk.GetArrayFromImage(surface)
        surface_values = sitk.GetArrayFromImage(distance)[surface_array > 0]
        return float((np.abs(surface_values) <= surface_tolerance).mean())

    left_surface = sitk.BinaryContourImageFilter().Execute(left_mask)
    right_surface = sitk.BinaryContourImageFilter().Execute(right_mask)
    left_coverage = surface_coverage(left_surface, right_mask)
    right_coverage = surface_coverage(right_surface, left_mask)
    surface_dsc = (
        2 * left_coverage * right_coverage / (left_coverage + right_coverage)
        if left_coverage + right_coverage
        else 1.0
    )
    voxel_volume = float(np.prod(left_mask.GetSpacing()))
    row = {
        "comparison_id": settings.get("comparison_id", "comparison"),
        "status": "valid",
        "mask_label": label,
        "reference_voxels": left_count,
        "comparison_voxels": right_count,
        "intersection_voxels": intersection,
        "union_voxels": union,
        "reference_volume_mm3": left_count * voxel_volume,
        "comparison_volume_mm3": right_count * voxel_volume,
        "dice": dice,
        "jaccard": jaccard,
        "volume_similarity": volume_similarity,
        "hausdorff_mm": hausdorff.GetHausdorffDistance(),
        "hausdorff_reference_to_comparison_mm": directed_left,
        "hausdorff_comparison_to_reference_mm": directed_right,
        "surface_tolerance_mm": surface_tolerance,
        "surface_dice": surface_dsc,
    }
    pd.DataFrame([row]).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            **row,
            "reference_hash": sha256(input_paths(0)[0]),
            "comparison_hash": sha256(input_paths(1)[0]),
        },
    )


def image_qc() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_IMAGE_QC_SETTINGS", "{}"))
    label = int(settings.get("mask_label", 1))
    image = read_single_image(input_paths(0)[0])
    foreground = binary_mask(read_single_image(input_paths(1)[0]), label)
    background = binary_mask(read_single_image(input_paths(2)[0]), label)
    require_same_geometry(image, foreground, ("image", "foreground mask"))
    require_same_geometry(image, background, ("image", "background mask"))
    image_array = sitk.GetArrayFromImage(image).astype(float)
    foreground_array = sitk.GetArrayFromImage(foreground) > 0
    background_array = sitk.GetArrayFromImage(background) > 0
    if not foreground_array.any() or not background_array.any():
        raise RuntimeError("image_qc requires nonempty foreground and background masks")
    foreground_values = image_array[foreground_array]
    background_values = image_array[background_array]
    fg_mean, fg_std = float(foreground_values.mean()), float(foreground_values.std())
    bg_mean, bg_std = float(background_values.mean()), float(background_values.std())
    snr = fg_mean / bg_std if bg_std > 0 else math.inf
    cnr = abs(fg_mean - bg_mean) / math.sqrt(fg_std**2 + bg_std**2) if fg_std + bg_std > 0 else math.inf
    per_slice_counts = foreground_array.reshape((foreground_array.shape[0], -1)).sum(axis=1)
    active_slices = np.flatnonzero(per_slice_counts > 0)
    missing_interior = (
        list(range(int(active_slices[0]) + 1, int(active_slices[-1])))
        if len(active_slices) > 1
        else []
    )
    gap_counts = [
        int(active_slices[index + 1] - active_slices[index] - 1)
        for index in range(len(active_slices) - 1)
    ]
    zero_variance_slices = int(
        sum(
            image_array[slice_index].std() == 0
            for slice_index in range(image_array.shape[0])
        )
    )
    row = {
        "extraction_id": settings.get("extraction_id", "unknown"),
        "status": "valid",
        "image_size": json.dumps(list(image.GetSize())),
        "image_spacing": json.dumps(list(image.GetSpacing())),
        "foreground_voxels": int(foreground_array.sum()),
        "background_voxels": int(background_array.sum()),
        "foreground_mean": fg_mean,
        "foreground_std": fg_std,
        "background_mean": bg_mean,
        "background_std": bg_std,
        "snr": snr,
        "cnr": cnr,
        "foreground_coefficient_of_variation": fg_std / abs(fg_mean) if fg_mean != 0 else math.inf,
        "foreground_uniformity": 1.0 - fg_std / abs(fg_mean) if fg_mean != 0 else math.nan,
        "n_slices": int(image_array.shape[0]),
        "n_active_foreground_slices": int(len(active_slices)),
        "n_zero_variance_slices": zero_variance_slices,
        "n_missing_interior_foreground_slices": len(missing_interior),
        "max_foreground_slice_gap": max(gap_counts, default=0),
    }
    pd.DataFrame([row]).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            **row,
            "image_hash": sha256(input_paths(0)[0]),
            "foreground_hash": sha256(input_paths(1)[0]),
            "background_hash": sha256(input_paths(2)[0]),
        },
    )


def rtstruct_geometry() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_RTSTRUCT_GEOMETRY_SETTINGS", "{}"))
    selected_roi_name = settings.get("roi_name")
    rtstruct_path = input_paths(0)[0]
    reference = read_image(input_paths(1))
    dataset = pydicom.dcmread(str(rtstruct_path), stop_before_pixels=True)
    roi_names = {
        str(item.ROINumber): str(getattr(item, "ROIName", "")).strip()
        for item in getattr(dataset, "StructureSetROISequence", [])
    }
    rows: list[dict[str, Any]] = []
    for roi_contour in getattr(dataset, "ROIContourSequence", []):
        roi_number = str(getattr(roi_contour, "ReferencedROINumber", ""))
        roi_name = roi_names.get(roi_number, "")
        if selected_roi_name and roi_name != selected_roi_name:
            continue
        contours = list(getattr(roi_contour, "ContourSequence", []))
        point_counts = [int(getattr(contour, "NumberOfContourPoints", 0)) for contour in contours]
        geometric_types = [str(getattr(contour, "ContourGeometricType", "")) for contour in contours]
        z_values: list[float] = []
        inside_bounds = True
        finite_points = True
        for contour in contours:
            points = np.asarray(contour.ContourData, dtype=float).reshape(-1, 3)
            finite_points = finite_points and bool(np.isfinite(points).all())
            z_values.extend(points[:, 2].tolist())
            for point in points:
                index = reference.TransformPhysicalPointToContinuousIndex(tuple(point))
                inside_bounds = inside_bounds and all(
                    -0.5 <= index[dim] < size + 0.5
                    for dim, size in enumerate(reference.GetSize())
                )
        unique_z = sorted(set(round(value, 4) for value in z_values))
        z_gaps = [unique_z[index + 1] - unique_z[index] for index in range(len(unique_z) - 1)]
        rows.append(
            {
                "roi_number": roi_number,
                "roi_name": roi_name,
                "status": "valid" if contours and finite_points and inside_bounds else "invalid",
                "contour_count": len(contours),
                "closed_planar_count": sum(value == "CLOSED_PLANAR" for value in geometric_types),
                "point_count": sum(point_counts),
                "min_points_per_contour": min(point_counts, default=0),
                "max_points_per_contour": max(point_counts, default=0),
                "unique_slice_count": len(unique_z),
                "median_slice_spacing_mm": float(np.median(z_gaps)) if z_gaps else None,
                "min_slice_spacing_mm": float(min(z_gaps)) if z_gaps else None,
                "max_slice_spacing_mm": float(max(z_gaps)) if z_gaps else None,
                "all_points_finite": finite_points,
                "all_points_inside_reference_bounds": inside_bounds,
                "reference_size": json.dumps(list(reference.GetSize())),
                "reference_spacing": json.dumps(list(reference.GetSpacing())),
            }
        )
    if selected_roi_name and not rows:
        raise RuntimeError(f"ROI `{selected_roi_name}` was not found in RTSTRUCT")
    pd.DataFrame(rows).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "rtstruct_hash": sha256(rtstruct_path),
            "reference_hash": sha256(input_paths(1)[0]),
            "roi_count": len(rows),
            "invalid_roi_count": sum(row["status"] != "valid" for row in rows),
        },
    )


def ivh_extract() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_IVH_SETTINGS", "{}"))
    label = int(settings.get("mask_label", 1))
    fractions = settings.get("volume_fractions", list(np.arange(0.05, 1.0, 0.05)))
    image = read_single_image(input_paths(0)[0])
    mask = binary_mask(read_single_image(input_paths(1)[0]), label)
    require_same_geometry(image, mask, ("image", "mask"))
    values = sitk.GetArrayFromImage(image)[sitk.GetArrayFromImage(mask) > 0].astype(float)
    if values.size == 0:
        raise RuntimeError("IVH extraction requires a nonempty mask")
    ordered = np.sort(values)
    volume_fraction = np.arange(1, ordered.size + 1, dtype=float) / ordered.size
    rows = []
    for fraction in fractions:
        target = float(fraction)
        index = min(int(np.searchsorted(volume_fraction, target, side="left")), ordered.size - 1)
        rows.append(
            {
                "extraction_id": settings.get("extraction_id", "unknown"),
                "mask_label": label,
                "volume_fraction": target,
                "intensity": float(ordered[index]),
            }
        )
    pd.DataFrame(rows).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "extraction_id": settings.get("extraction_id", "unknown"),
            "voxel_count": int(values.size),
            "minimum": float(ordered[0]),
            "maximum": float(ordered[-1]),
            "image_hash": sha256(input_paths(0)[0]),
            "mask_hash": sha256(input_paths(1)[0]),
            "volume_fractions": [float(value) for value in fractions],
        },
    )


def shape_topology() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_SHAPE_TOPOLOGY_SETTINGS", "{}"))
    label = int(settings.get("mask_label", 1))
    mask = binary_mask(read_single_image(input_paths(0)[0]), label)
    array = sitk.GetArrayFromImage(mask) > 0
    if not array.any():
        raise RuntimeError("shape topology requires a nonempty mask")
    structure = np.ones((3, 3, 3), dtype=np.uint8)
    labels, component_count = ndimage.label(array, structure=structure)
    surface_array = array & ~ndimage.binary_erosion(
        array, structure=structure, border_value=0
    )
    from skimage.measure import euler_number, perimeter

    euler = int(euler_number(array, connectivity=3))
    coordinates = np.argwhere(array)
    spacing = np.asarray(mask.GetSpacing(), dtype=float)
    physical_coordinates = []
    for z, y, x in coordinates:
        point = mask.TransformContinuousIndexToPhysicalPoint((int(x), int(y), int(z)))
        physical_coordinates.append(point)
    points = np.asarray(physical_coordinates, dtype=float)
    centroid = points.mean(axis=0)
    centered = points - centroid
    covariance = centered.T @ centered / max(1, len(centered))
    eigenvalues, eigenvectors = np.linalg.eigh(covariance)
    surface_points = points[
        surface_array[coordinates[:, 0], coordinates[:, 1], coordinates[:, 2]]
    ]
    hull = ConvexHull(surface_points)
    voxel_volume = float(np.prod(mask.GetSpacing()))
    mask_volume = int(array.sum()) * voxel_volume
    slice_profile: list[dict[str, Any]] = []
    for slice_index in range(array.shape[0]):
        binary_slice = array[slice_index]
        count = int(binary_slice.sum())
        if count == 0:
            continue
        ys, xs = np.nonzero(binary_slice)
        slice_points = points[coordinates[:, 0] == slice_index]
        hull_points = slice_points
        if len(slice_points) > 3:
            hull_points = slice_points[ConvexHull(slice_points[:, :2]).vertices]
        diameter = float(
            np.sqrt(
                ((hull_points[:, None, :] - hull_points[None, :, :]) ** 2).sum(axis=2)
            ).max()
        )
        slice_profile.append(
            {
                "extraction_id": settings.get("extraction_id", "unknown"),
                "slice_index": slice_index,
                "area_mm2": count * float(spacing[0] * spacing[1]),
                "perimeter_mm": float(
                    perimeter(binary_slice, neighborhood=4)
                    * math.sqrt(float(spacing[0] * spacing[1]))
                ),
                "max_diameter_mm": diameter,
                "centroid_x_mm": float(xs.mean() * spacing[0]),
                "centroid_y_mm": float(ys.mean() * spacing[1]),
            }
        )
    row = {
        "extraction_id": settings.get("extraction_id", "unknown"),
        "mask_label": label,
        "status": "valid",
        "voxel_count": int(array.sum()),
        "connected_component_count": int(component_count),
        "largest_component_fraction": float((labels == np.argmax(np.bincount(labels.ravel())[1:]) + 1).sum() / array.sum()),
        "euler_number_connectivity_26": euler,
        "hole_count_estimate_26": max(0, int(-euler)),
        "centroid_x_mm": float(centroid[0]),
        "centroid_y_mm": float(centroid[1]),
        "centroid_z_mm": float(centroid[2]),
        "principal_axis_variance_mm2_1": float(eigenvalues[-1]),
        "principal_axis_variance_mm2_2": float(eigenvalues[-2]),
        "principal_axis_variance_mm2_3": float(eigenvalues[-3]),
        "principal_axis_ratio_21": float(eigenvalues[-1] / eigenvalues[-2]) if eigenvalues[-2] > 0 else math.inf,
        "principal_axis_ratio_32": float(eigenvalues[-2] / eigenvalues[-3]) if eigenvalues[-3] > 0 else math.inf,
        "slice_count": len(slice_profile),
        "mask_volume_mm3": mask_volume,
        "convex_hull_volume_mm3": float(hull.volume),
        "convex_hull_surface_area_mm2": float(hull.area),
        "convex_hull_solidity": mask_volume / float(hull.volume) if hull.volume > 0 else math.nan,
    }
    pd.DataFrame([row]).to_parquet(output_dir(0), index=False)
    pd.DataFrame(slice_profile).to_parquet(output_dir(1), index=False)
    write_json(
        2,
        {**row, "mask_hash": sha256(input_paths(0)[0]), "principal_axes": eigenvectors.tolist()},
    )


def register_images() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_REGISTER_SETTINGS", "{}"))
    fixed = read_single_image(input_paths(0)[0])
    moving = read_single_image(input_paths(1)[0])
    dimensions = fixed.GetDimension()
    method = sitk.ImageRegistrationMethod()
    method.SetMetricAsMattesMutualInformation(numberOfHistogramBins=50)
    method.SetMetricSamplingStrategy(method.RANDOM)
    method.SetMetricSamplingPercentage(float(settings.get("sampling_percent", 0.15)))
    method.SetInterpolator(sitk.sitkLinear)
    method.SetOptimizerAsGradientDescent(
        learningRate=float(settings.get("learning_rate", 1.0)),
        numberOfIterations=int(settings.get("iterations", 100)),
        convergenceMinimumValue=float(settings.get("convergence_minimum_value", 1e-6)),
        convergenceWindowSize=10,
    )
    method.SetOptimizerScalesFromPhysicalShift()
    if settings.get("transform_type", "rigid").lower() == "affine":
        initial = sitk.AffineTransform(dimensions)
    else:
        initial = sitk.Euler3DTransform() if dimensions == 3 else sitk.Euler2DTransform()
    initializer = sitk.CenteredTransformInitializer(
        fixed,
        moving,
        initial,
        sitk.CenteredTransformInitializerFilter.MOMENTS,
    )
    method.SetInitialTransform(sitk.Transform(initializer), inPlace=False)
    transform = method.Execute(fixed, moving)
    metric = method.GetMetricValue()
    registered = sitk.Resample(
        moving,
        fixed,
        transform,
        sitk.sitkLinear,
        float(settings.get("default_pixel_value", 0.0)),
        moving.GetPixelID(),
    )
    sitk.WriteImage(registered, str(output_dir(0)), True)
    output_dir(1).write_text(str(transform) + "\n")
    write_json(
        2,
        {
            "transform_type": settings.get("transform_type", "rigid"),
            "metric_value": float(metric),
            "iterations": int(settings.get("iterations", 100)),
            "optimizer_stop_condition": method.GetOptimizerStopConditionDescription(),
            "fixed_hash": sha256(input_paths(0)[0]),
            "moving_hash": sha256(input_paths(1)[0]),
            "registered": image_metadata(registered, input_paths(1)),
        },
    )


def delta_features() -> None:
    settings = json.loads(os.environ.get("RADIOMICS_DELTA_SETTINGS", "{}"))
    source = input_paths(0)[0]
    frame = pd.read_parquet(source) if source.suffix.lower() == ".parquet" else pd.read_csv(source)
    id_column = settings.get("id_column", "patient_id")
    time_column = settings.get("timepoint_column", "timepoint")
    baseline = settings.get("baseline", "baseline")
    followup = settings.get("followup", "followup")
    required = {id_column, time_column}
    missing = sorted(required.difference(frame.columns))
    if missing:
        raise RuntimeError(f"delta feature table is missing columns: {', '.join(missing)}")
    reserved = {
        id_column,
        time_column,
        "extraction_id",
        "image_id",
        "roi_id",
        "modality",
        "preset_id",
        "status",
        "error_code",
        "feature_set_id",
        "feature_set_version",
    }
    feature_columns = [
        column
        for column in frame.columns
        if column not in reserved and pd.api.types.is_numeric_dtype(frame[column])
    ]
    if not feature_columns:
        raise RuntimeError("delta feature table has no numeric feature columns")
    indexed = frame.set_index([id_column, time_column])
    rows: list[dict[str, Any]] = []
    unresolved: list[dict[str, Any]] = []
    for patient_id, group in frame.groupby(id_column, sort=True):
        try:
            base = indexed.loc[(patient_id, baseline)]
            follow = indexed.loc[(patient_id, followup)]
        except KeyError as error:
            unresolved.append({"patient_id": patient_id, "reason": f"missing timepoint: {error}"})
            continue
        if isinstance(base, pd.DataFrame) or isinstance(follow, pd.DataFrame):
            unresolved.append({"patient_id": patient_id, "reason": "duplicate timepoint"})
            continue
        for feature in feature_columns:
            baseline_value, followup_value = float(base[feature]), float(follow[feature])
            difference = followup_value - baseline_value
            rows.append(
                {
                    id_column: patient_id,
                    "feature_id": feature,
                    "baseline_timepoint": baseline,
                    "followup_timepoint": followup,
                    "baseline_value": baseline_value,
                    "followup_value": followup_value,
                    "absolute_change": difference,
                    "relative_change": difference / baseline_value if baseline_value != 0 else math.nan,
                    "percent_change": 100.0 * difference / baseline_value if baseline_value != 0 else math.nan,
                }
            )
    pd.DataFrame(rows).to_parquet(output_dir(0), index=False)
    write_json(
        1,
        {
            "n_patients": int(frame[id_column].nunique()),
            "n_pairs": len({row[id_column] for row in rows}),
            "n_features": len(feature_columns),
            "unresolved": unresolved,
            "source_hash": sha256(source),
        },
    )


def normalize_feature_name(name: str) -> str:
    value = re.sub(r"[^a-zA-Z0-9]+", "_", name).strip("_").lower()
    value = re.sub(r"_+", "_", value)
    return value or "feature"


def image_type_from_pyradiomics_name(raw_name: str, normalized_name: str) -> str:
    lower = raw_name.lower()
    if lower.startswith("wavelet-"):
        return "wavelet"
    if lower.startswith("log-sigma-"):
        return "log"
    if lower.startswith("lbp-2d"):
        return "lbp2d"
    if lower.startswith("lbp-3d"):
        return "lbp3d"
    for prefix in [
        "original",
        "square",
        "squareroot",
        "logarithm",
        "exponential",
        "gradient",
    ]:
        if lower.startswith(f"{prefix}-") or normalized_name.startswith(f"{prefix}_"):
            return prefix
    return normalized_name.split("_", 1)[0]


def extraction_settings() -> dict[str, Any]:
    return json.loads(os.environ.get("RADIOMICS_EXTRACT_SETTINGS", "{}"))


def make_extractor() -> Any:
    from radiomics import featureextractor
    import scipy.special

    # PyRadiomics 3.1 imports the SciPy <=1.16 sph_harm symbol. Provide the
    # same argument order on SciPy 1.17+ instead of losing LBP3D silently.
    if not hasattr(scipy.special, "sph_harm"):
        scipy.special.sph_harm = lambda m, n, theta, phi: scipy.special.sph_harm_y(
            n, m, theta, phi
        )

    settings = extraction_settings()
    kwargs = {
        "label": int(settings.get("mask_label", 1)),
        "binWidth": float(settings.get("bin_width", 25.0)),
    }
    if settings.get("resampled_spacing"):
        kwargs["resampledPixelSpacing"] = [float(value) for value in settings["resampled_spacing"]]
    if settings.get("force2d", False):
        kwargs["force2D"] = True
        kwargs["force2Ddimension"] = int(settings.get("force2d_dimension", 0))
    extractor = featureextractor.RadiomicsFeatureExtractor(**kwargs)
    image_types = settings.get("image_types", ["Original"])
    feature_classes = settings.get(
        "feature_classes",
        ["shape", "firstorder", "glcm", "glrlm", "glszm", "gldm", "ngtdm"],
    )
    extractor.disableAllImageTypes()
    for image_type in image_types:
        if image_type == "LoG":
            extractor.enableImageTypeByName(
                "LoG",
                customArgs={
                    "sigma": [float(value) for value in settings.get("log_sigmas", [2, 3, 4, 5])]
                },
            )
        else:
            extractor.enableImageTypeByName(image_type)
    extractor.disableAllFeatures()
    for feature_class in feature_classes:
        extractor.enableFeatureClassByName(feature_class)
    return extractor


def result_rows(extraction: dict[str, Any], result: dict[str, Any]) -> tuple[dict[str, Any], list[dict[str, Any]], list[dict[str, Any]]]:
    wide: dict[str, Any] = dict(extraction)
    long_rows: list[dict[str, Any]] = []
    metadata_rows: list[dict[str, Any]] = []
    for raw_name, value in result.items():
        if raw_name.startswith("diagnostics_"):
            continue
        if isinstance(value, np.ndarray) and value.size == 1:
            value = value.item()
        if isinstance(value, complex):
            value = float(value.real)
        if not isinstance(value, (int, float, np.number)) or not np.isfinite(float(value)):
            continue
        feature_id = normalize_feature_name(raw_name)
        wide[feature_id] = float(value)
        parts = feature_id.split("_")
        family = next((part for part in parts if part in {"shape", "shape2d", "firstorder", "glcm", "glrlm", "glszm", "gldm", "ngtdm"}), "other")
        image_type = image_type_from_pyradiomics_name(raw_name, feature_id)
        long_rows.append(
            {
                "extraction_id": extraction["extraction_id"],
                "feature_id": feature_id,
                "pyradiomics_name": raw_name,
                "value": float(value),
            }
        )
        metadata_rows.append(
            {
                "feature_id": feature_id,
                "pyradiomics_name": raw_name,
                "feature_family": family,
                "image_type": image_type,
                "preset_id": extraction.get("preset_id", ""),
                "modality": extraction.get("modality", ""),
            }
        )
    return wide, long_rows, metadata_rows


def extract_rows(extractions: list[dict[str, Any]], images: list[Path], masks: list[Path]) -> None:
    if len(extractions) != len(images) or len(extractions) != len(masks):
        raise RuntimeError("manifest, image FileSet, and mask FileSet lengths differ")
    if not extractions:
        raise RuntimeError("no extraction units were selected")
    extractor = make_extractor()
    wide_rows: list[dict[str, Any]] = []
    long_rows: list[dict[str, Any]] = []
    metadata_rows: list[dict[str, Any]] = []
    diagnostic_rows: list[dict[str, Any]] = []
    provenance: list[dict[str, Any]] = []
    import radiomics
    import pywt
    default_label = int(extraction_settings().get("mask_label", 1))

    for index, extraction in enumerate(extractions):
        row_settings = dict(extraction_settings())
        try:
            row_label = int(extraction.get("mask_label", default_label))
            extractor.settings["label"] = row_label
            row_settings["mask_label"] = row_label
            image_hash = sha256(images[index])
            mask_hash = sha256(masks[index])
            image = read_single_image(images[index])
            mask = read_single_image(masks[index])
            mask_voxels = int(np.count_nonzero(sitk.GetArrayFromImage(mask) == row_label))
            if mask_voxels == 0:
                raise RuntimeError(
                    f"mask label {row_label} is empty for extraction "
                    f"`{extraction['extraction_id']}`"
                )
            result = extractor.execute(image, mask)
            wide, longs, metadata = result_rows(extraction, dict(result))
            if not longs:
                raise RuntimeError(
                    f"PyRadiomics returned no features for extraction "
                    f"`{extraction['extraction_id']}`"
                )
            wide["status"] = "valid"
            wide["error_code"] = ""
            wide_rows.append(wide)
            provenance.append(
                {
                    **extraction,
                    "pyradiomics_version": radiomics.__version__,
                    "simpleitk_version": sitk.Version_VersionString(),
                    "numpy_version": np.__version__,
                    "pywavelets_version": pywt.__version__,
                    "python_version": platform.python_version(),
                    "image_hash": image_hash,
                    "mask_hash": mask_hash,
                    "settings": row_settings,
                    "status": "valid",
                    "error_code": "",
                }
            )
            long_rows.extend(longs)
            metadata_rows.extend(metadata)
            for key, value in result.items():
                if key.startswith("diagnostics_"):
                    diagnostic_rows.append(
                        {
                            "extraction_id": extraction["extraction_id"],
                            "field": key,
                            "value": str(value),
                        }
                    )
        except Exception as error:  # preserve other extraction units
            fallback = dict(extraction)
            fallback.update({"status": "invalid", "error_code": str(error)})
            wide_rows.append(fallback)
            diagnostic_rows.append(
                {
                    "extraction_id": extraction["extraction_id"],
                    "field": "extraction_error",
                    "value": str(error),
                }
            )
            provenance.append(
                {
                    **extraction,
                    "pyradiomics_version": radiomics.__version__,
                    "simpleitk_version": sitk.Version_VersionString(),
                    "numpy_version": np.__version__,
                    "pywavelets_version": pywt.__version__,
                    "python_version": platform.python_version(),
                    "image_hash": None,
                    "mask_hash": None,
                    "settings": row_settings,
                    "status": "invalid",
                    "error_code": str(error),
                }
            )

    pd.DataFrame(wide_rows).to_parquet(output_dir(0), index=False)
    pd.DataFrame(long_rows).to_parquet(output_dir(1), index=False)
    pd.DataFrame(metadata_rows).drop_duplicates("feature_id").to_parquet(output_dir(2), index=False)
    pd.DataFrame(diagnostic_rows).to_parquet(output_dir(3), index=False)
    write_json(4, {"extractions": provenance})
    failures = [row for row in provenance if row["status"] != "valid"]
    if failures:
        details = "; ".join(
            f"`{row.get('extraction_id', 'unknown')}`: {row.get('error_code', '')}".strip(": ")
            for row in failures
        )
        raise RuntimeError(f"{len(failures)} extraction unit(s) failed: {details}")


def extract_single() -> None:
    extraction = json.loads(os.environ.get("RADIOMICS_EXTRACTION", "{}"))
    required = ["extraction_id", "patient_id", "image_id", "roi_id", "modality", "preset_id"]
    missing = [key for key in required if not extraction.get(key)]
    if missing:
        raise RuntimeError(f"missing extraction metadata: {', '.join(missing)}")
    extract_rows([extraction], input_paths(0), input_paths(1))


def extract_batch() -> None:
    manifest = pd.read_csv(input_paths(2)[0])
    required = {"extraction_id", "patient_id", "image_id", "roi_id", "modality", "preset_id"}
    missing = sorted(required.difference(manifest.columns))
    if missing:
        raise RuntimeError(f"manifest is missing columns: {', '.join(missing)}")
    extractions = manifest.to_dict(orient="records")
    valid_only = os.environ.get("RADIOMICS_VALID_ONLY", "true").lower() == "true"
    if valid_only and "status" in manifest:
        keep = manifest["status"].astype(str).eq("valid")
        extractions = manifest.loc[keep].to_dict(orient="records")
    extract_rows(extractions, input_paths(0), input_paths(1))


def require_same_geometry(reference: sitk.Image, other: sitk.Image, role: str) -> None:
    if list(reference.GetSize()) != list(other.GetSize()) or not close(
        reference.GetSpacing(), other.GetSpacing(), 0.01
    ):
        raise RuntimeError(f"{role} geometry does not match the reference image")


def bias_correct() -> None:
    """N4 inhomogeneity correction, mask-guided; the field is kept for QC."""
    settings = json.loads(os.environ.get("RADIOMICS_BIAS_SETTINGS", "{}"))
    image = read_single_image(input_paths(0)[0])
    mask = read_single_image(input_paths(1)[0])
    require_same_geometry(image, mask, "mask")

    label = int(settings.get("mask_label", 1))
    shrink = int(settings.get("shrink_factor", 4))
    iterations = [int(value) for value in settings.get("max_iterations", [50, 50, 50, 50])]
    if shrink < 1 or shrink > 8:
        raise RuntimeError("shrink_factor must lie in [1, 8]")
    if not iterations or len(iterations) > 8 or any(value < 1 for value in iterations):
        raise RuntimeError("max_iterations must contain 1-8 positive levels")

    binary_array = (sitk.GetArrayFromImage(mask) == label).astype(np.uint8)
    if np.count_nonzero(binary_array) == 0:
        raise RuntimeError(f"mask label {label} is empty")
    binary = sitk.GetImageFromArray(binary_array)
    binary.CopyInformation(mask)

    image = sitk.Cast(image, sitk.sitkFloat32)
    shrunk_image = sitk.Shrink(image, [shrink] * image.GetDimension())
    shrunk_mask = sitk.Shrink(binary, [shrink] * image.GetDimension())
    if np.count_nonzero(sitk.GetArrayFromImage(shrunk_mask)) == 0:
        raise RuntimeError(
            f"mask label {label} vanishes at shrink factor {shrink}; lower shrink_factor"
        )

    corrector = sitk.N4BiasFieldCorrectionImageFilter()
    corrector.SetMaximumNumberOfIterations(iterations)
    corrector.SetConvergenceThreshold(float(settings.get("convergence_threshold", 1e-6)))
    corrector.SetNumberOfHistogramBins(int(settings.get("histogram_bins", 200)))
    corrector.SetWienerFilterNoise(float(settings.get("wiener_noise", 0.01)))
    corrector.Execute(shrunk_image, shrunk_mask)
    # SimpleITK wraps ITK's GetLogBiasFieldAsImageHeadReference under a
    # shorter name; the argument is the full-resolution reference grid.
    log_bias = corrector.GetLogBiasFieldAsImage(image)
    field = sitk.Exp(log_bias)
    corrected = sitk.Divide(image, field)

    sitk.WriteImage(sitk.Cast(corrected, sitk.sitkFloat32), str(output_dir(0)), True)
    sitk.WriteImage(sitk.Cast(field, sitk.sitkFloat32), str(output_dir(1)), True)
    field_array = sitk.GetArrayFromImage(field)
    write_json(
        2,
        {
            "settings": settings,
            "corrected": image_metadata(corrected, input_paths(0)),
            "bias_field_min": float(field_array.min()),
            "bias_field_max": float(field_array.max()),
            "bias_field_ratio": float(field_array.max() / field_array.min()),
            "elapsed_iterations": corrector.GetElapsedIterations(),
            "convergence_measurement": corrector.GetCurrentConvergenceMeasurement(),
            "image_hash": sha256(input_paths(0)[0]),
            "mask_hash": sha256(input_paths(1)[0]),
        },
    )


def robust_normalize() -> None:
    """Percentile-truncated robust z-score over a tissue mask, whole image."""
    settings = json.loads(os.environ.get("RADIOMICS_NORMALIZE_SETTINGS", "{}"))
    image = read_single_image(input_paths(0)[0])
    mask = read_single_image(input_paths(1)[0])
    require_same_geometry(image, mask, "mask")

    label = int(settings.get("mask_label", 1))
    lower_pct = float(settings.get("lower_percentile", 1.0))
    upper_pct = float(settings.get("upper_percentile", 99.0))
    if not (0.0 <= lower_pct < upper_pct <= 100.0):
        raise RuntimeError("percentiles must satisfy 0 <= lower < upper <= 100")

    image_array = sitk.GetArrayFromImage(image).astype(np.float64)
    tissue = sitk.GetArrayFromImage(mask) == label
    if np.count_nonzero(tissue) == 0:
        raise RuntimeError(f"tissue mask label {label} is empty")
    values = image_array[tissue]
    low, high = (float(value) for value in np.percentile(values, [lower_pct, upper_pct]))
    if high <= low:
        raise RuntimeError("tissue intensity range is degenerate")
    winsorized = np.clip(values, low, high)
    mean = float(winsorized.mean())
    std = float(winsorized.std())
    if std == 0.0:
        raise RuntimeError("tissue intensity is constant; cannot z-score")

    normalized = sitk.GetImageFromArray(
        ((np.clip(image_array, low, high) - mean) / std).astype(np.float32)
    )
    normalized.CopyInformation(image)
    sitk.WriteImage(normalized, str(output_dir(0)), True)
    write_json(
        1,
        {
            "settings": settings,
            "normalized": image_metadata(normalized, input_paths(0)),
            "lower_bound": low,
            "upper_bound": high,
            "winsorized_mean": mean,
            "winsorized_std": std,
            "n_tissue_voxels": int(np.count_nonzero(tissue)),
            "clipped_tissue_fraction": float(np.mean((values < low) | (values > high))),
            "image_hash": sha256(input_paths(0)[0]),
        },
    )


def physical_ball_footprint(spacing_xyz: Sequence[float], radius_mm: float) -> np.ndarray:
    """Boolean ball in array space (z, y, x) covering a physical radius."""
    spacing_zyx = [float(value) for value in reversed(spacing_xyz)]
    radii = [max(1, int(math.ceil(radius_mm / value))) for value in spacing_zyx]
    grids = np.ogrid[tuple(slice(-radius, radius + 1) for radius in radii)]
    squared = sum((grid * value) ** 2 for grid, value in zip(grids, spacing_zyx))
    footprint = squared <= radius_mm**2 + 1e-9
    if not footprint.any():
        raise RuntimeError(f"ball footprint for {radius_mm} mm is empty")
    return footprint


def peritumoral_ring() -> None:
    """Ring band around the tumor at physical radii, minus an exclusion mask."""
    settings = json.loads(os.environ.get("RADIOMICS_RING_SETTINGS", "{}"))
    image = read_single_image(input_paths(0)[0])
    mask = read_single_image(input_paths(1)[0])
    exclusion = read_single_image(input_paths(2)[0])
    require_same_geometry(image, mask, "mask")
    require_same_geometry(image, exclusion, "exclusion mask")

    label = int(settings.get("mask_label", 1))
    inner_mm = float(settings.get("inner_mm", 0.0))
    outer_mm = float(settings.get("outer_mm", 5.0))
    if not (0.0 <= inner_mm < outer_mm) or outer_mm > 50.0:
        raise RuntimeError("radii must satisfy 0 <= inner_mm < outer_mm <= 50")

    spacing = image.GetSpacing()
    voxel_volume = float(np.prod(spacing))
    tumor = sitk.GetArrayFromImage(mask) == label
    if np.count_nonzero(tumor) == 0:
        raise RuntimeError(f"mask label {label} is empty")
    excluded = sitk.GetArrayFromImage(exclusion) != 0

    outer_shell = ndimage.binary_dilation(
        tumor, structure=physical_ball_footprint(spacing, outer_mm)
    )
    core = (
        ndimage.binary_dilation(tumor, structure=physical_ball_footprint(spacing, inner_mm))
        if inner_mm > 0.0
        else tumor
    )
    ring = outer_shell & ~core & ~tumor & ~excluded
    ring_voxels = int(np.count_nonzero(ring))
    if ring_voxels == 0:
        raise RuntimeError(
            "peritumoral ring is empty under the requested radii and exclusions"
        )

    def write_mask(array: np.ndarray, index: int) -> None:
        written = sitk.GetImageFromArray(array.astype(np.uint8))
        written.CopyInformation(image)
        sitk.WriteImage(written, str(output_dir(index)), True)

    write_mask(ring, 0)
    labeled = np.where(tumor, 1, np.where(ring, 2, 0))
    write_mask(labeled, 1)
    write_mask(tumor | ring, 2)
    write_json(
        3,
        {
            "settings": settings,
            "tumor_voxels": int(np.count_nonzero(tumor)),
            "ring_voxels": ring_voxels,
            "combined_voxels": int(np.count_nonzero(tumor | ring)),
            "tumor_volume_mm3": float(np.count_nonzero(tumor) * voxel_volume),
            "ring_volume_mm3": float(ring_voxels * voxel_volume),
            "ring_voxels_removed_by_exclusion": int(
                np.count_nonzero(outer_shell & ~core & ~tumor & excluded)
            ),
            "voxel_volume_mm3": voxel_volume,
            "image_hash": sha256(input_paths(0)[0]),
            "mask_hash": sha256(input_paths(1)[0]),
            "exclusion_hash": sha256(input_paths(2)[0]),
        },
    )


def kmeans_pp_init(data: np.ndarray, k: int, rng: np.random.Generator) -> np.ndarray:
    centers = np.empty((k, data.shape[1]), dtype=np.float64)
    centers[0] = data[rng.integers(data.shape[0])]
    closest = ((data - centers[0]) ** 2).sum(axis=1)
    for index in range(1, k):
        total = float(closest.sum())
        if total <= 0.0:
            centers[index:] = data[rng.integers(data.shape[0], size=k - index)]
            break
        centers[index] = data[rng.choice(data.shape[0], p=closest / total)]
        closest = np.minimum(closest, ((data - centers[index]) ** 2).sum(axis=1))
    return centers


def deterministic_kmeans(
    data: np.ndarray,
    k: int,
    seed: int,
    n_init: int,
    max_iter: int,
    tol: float,
) -> tuple[np.ndarray, np.ndarray, float]:
    """Plain Lloyd k-means with seeded k-means++ restarts; best inertia wins."""
    rng = np.random.default_rng(seed)
    best_centers = None
    best_labels = None
    best_inertia = math.inf
    for _ in range(n_init):
        centers = kmeans_pp_init(data, k, rng)
        labels = np.zeros(data.shape[0], dtype=np.int64)
        for _ in range(max_iter):
            distances = ((data[:, None, :] - centers[None, :, :]) ** 2).sum(axis=2)
            labels = distances.argmin(axis=1)
            new_centers = centers.copy()
            for cluster in range(k):
                members = labels == cluster
                if np.any(members):
                    new_centers[cluster] = data[members].mean(axis=0)
                else:
                    # Re-seed an empty cluster on the worst-fit point.
                    new_centers[cluster] = data[int(distances.min(axis=1).argmax())]
            shift = float(np.abs(new_centers - centers).max())
            centers = new_centers
            if shift <= tol:
                break
        distances = ((data[:, None, :] - centers[None, :, :]) ** 2).sum(axis=2)
        labels = distances.argmin(axis=1)
        inertia = float(distances.min(axis=1).sum())
        if inertia < best_inertia:
            best_centers, best_labels, best_inertia = centers, labels, inertia
    assert best_centers is not None and best_labels is not None
    return best_centers, best_labels, best_inertia


def read_channel_stack(
    mask: sitk.Image, channel_paths: Sequence[Path], role: str
) -> np.ndarray:
    arrays = []
    for path in channel_paths:
        channel = read_single_image(path)
        require_same_geometry(mask, channel, f"{role} channel `{path.name}`")
        arrays.append(sitk.GetArrayFromImage(channel).astype(np.float64))
    return np.stack(arrays, axis=-1)


def habitat_fit() -> None:
    """Fit common habitat centers on pooled, per-case standardized samples."""
    settings = json.loads(os.environ.get("RADIOMICS_HABITAT_FIT_SETTINGS", "{}"))
    manifest = pd.read_csv(input_paths(2)[0], dtype=str)
    required = {"case_id", "mask", "channels"}
    missing = sorted(required.difference(manifest.columns))
    if missing:
        raise RuntimeError(f"manifest is missing columns: {', '.join(missing)}")
    rows = manifest.to_dict(orient="records")
    if not rows:
        raise RuntimeError("habitat manifest has no rows")

    label = int(settings.get("mask_label", 1))
    n_habitats = int(settings.get("n_habitats", 3))
    sample_target = int(settings.get("sample_voxels_per_case", 10000))
    seed = int(settings.get("seed", 0))
    standardize = bool(settings.get("standardize", True))
    n_init = int(settings.get("n_init", 8))
    max_iter = int(settings.get("max_iter", 300))
    if not 2 <= n_habitats <= 6:
        raise RuntimeError("n_habitats must lie in [2, 6]")
    if sample_target < 100:
        raise RuntimeError("sample_voxels_per_case must be at least 100")
    if not 1 <= n_init <= 50 or max_iter < 1:
        raise RuntimeError("n_init must lie in [1, 50] and max_iter must be positive")

    mask_files = {path.name: path for path in input_paths(0)}
    channel_files = {path.name: path for path in input_paths(1)}
    channel_names = [name.strip() for name in str(rows[0]["channels"]).split(";") if name.strip()]
    if not channel_names:
        raise RuntimeError("channels column must list at least one channel file name")

    pooled: list[np.ndarray] = []
    per_case: list[dict[str, Any]] = []
    for case_index, row in enumerate(rows):
        case_id = str(row["case_id"])
        mask_name = str(row["mask"])
        if mask_name not in mask_files:
            raise RuntimeError(f"case `{case_id}` references unknown mask `{mask_name}`")
        names = [name.strip() for name in str(row["channels"]).split(";") if name.strip()]
        if names != channel_names:
            raise RuntimeError(
                f"case `{case_id}` channel list differs from the first case; "
                "all cases must share one ordered channel list"
            )
        missing_channels = [name for name in channel_names if name not in channel_files]
        if missing_channels:
            raise RuntimeError(
                f"case `{case_id}` is missing channel files: {', '.join(missing_channels)}"
            )
        mask = read_single_image(mask_files[mask_name])
        stack = read_channel_stack(
            mask, [channel_files[name] for name in channel_names], case_id
        )
        tumor = sitk.GetArrayFromImage(mask) == label
        n_tumor = int(np.count_nonzero(tumor))
        if n_tumor == 0:
            raise RuntimeError(f"case `{case_id}` mask label {label} is empty")
        values = stack[tumor]
        case_rng = np.random.default_rng([seed, case_index])
        if values.shape[0] > sample_target:
            chosen = np.sort(case_rng.choice(values.shape[0], size=sample_target, replace=False))
            values = values[chosen]
        means = np.zeros(values.shape[1])
        scales = np.ones(values.shape[1])
        if standardize:
            means = values.mean(axis=0)
            scales = values.std(axis=0)
            if np.any(scales == 0.0):
                raise RuntimeError(
                    f"case `{case_id}` has a constant channel; cannot standardize"
                )
            values = (values - means) / scales
        pooled.append(values)
        per_case.append(
            {
                "case_id": case_id,
                "n_tumor_voxels": n_tumor,
                "n_sampled": int(values.shape[0]),
                "channel_means": means.tolist(),
                "channel_stds": scales.tolist(),
            }
        )

    data = np.concatenate(pooled, axis=0)
    centers, _, inertia = deterministic_kmeans(
        data, n_habitats, seed, n_init, max_iter, 1e-8
    )
    # Freeze label order by descending first-channel center so habitat k means
    # the same thing across folds, cases, and the frozen model.
    order = np.argsort(-centers[:, 0], kind="stable")
    centers = centers[order]

    write_json(
        0,
        {
            "n_habitats": n_habitats,
            "channel_names": channel_names,
            "standardize": standardize,
            "centers": centers.tolist(),
            "relabel_rule": "descending_first_channel_center",
            "inertia": inertia,
            "seed": seed,
            "n_init": n_init,
            "max_iter": max_iter,
            "sample_voxels_per_case": sample_target,
            "n_cases": len(rows),
            "n_pooled_voxels": int(data.shape[0]),
            "per_case": per_case,
            "mask_hashes": [sha256(mask_files[str(row["mask"])]) for row in rows],
        },
    )
    sample_rows = []
    offset = 0
    for case in per_case:
        n_sampled = case["n_sampled"]
        counts = np.bincount(
            np.argmax(
                (
                    (data[offset : offset + n_sampled, None, :] - centers[None, :, :]) ** 2
                ).sum(axis=2),
                axis=1,
            ),
            minlength=n_habitats,
        )
        for habitat in range(n_habitats):
            sample_rows.append(
                {
                    "case_id": case["case_id"],
                    "habitat": habitat + 1,
                    "n_sampled_voxels": int(counts[habitat]),
                }
            )
        offset += n_sampled
    pd.DataFrame(sample_rows).to_parquet(output_dir(1), index=False)


def habitat_interface_fraction(labels: np.ndarray) -> np.ndarray:
    """Per-label fraction of voxels 6-adjacent to a different habitat label."""
    different = np.zeros(labels.shape, dtype=bool)
    for axis in range(labels.ndim):
        for step in (-1, 1):
            neighbor = np.zeros_like(labels)
            target = [slice(None)] * labels.ndim
            source = [slice(None)] * labels.ndim
            if step == 1:
                target[axis] = slice(0, -1)
                source[axis] = slice(1, None)
            else:
                target[axis] = slice(1, None)
                source[axis] = slice(0, -1)
            neighbor[tuple(target)] = labels[tuple(source)]
            different |= (neighbor != 0) & (neighbor != labels)
    fractions = []
    for habitat in range(1, int(labels.max()) + 1):
        members = labels == habitat
        n_members = int(np.count_nonzero(members))
        fractions.append(
            float(np.count_nonzero(members & different) / n_members) if n_members else 0.0
        )
    return np.asarray(fractions)


def habitat_assign() -> None:
    """Assign tumor voxels to frozen habitat centers; emit mask + features."""
    settings = json.loads(os.environ.get("RADIOMICS_HABITAT_ASSIGN_SETTINGS", "{}"))
    label = int(settings.get("mask_label", 1))
    mask = read_single_image(input_paths(0)[0])
    model = json.loads(input_paths(2)[0].read_text())
    channel_names = [str(name) for name in model["channel_names"]]
    centers = np.asarray(model["centers"], dtype=np.float64)
    n_habitats = centers.shape[0]

    channel_files = {path.name: path for path in input_paths(1)}
    missing = [name for name in channel_names if name not in channel_files]
    if missing:
        raise RuntimeError(f"channel files missing for assignment: {', '.join(missing)}")
    stack = read_channel_stack(
        mask, [channel_files[name] for name in channel_names], "assignment"
    )
    tumor = sitk.GetArrayFromImage(mask) == label
    n_tumor = int(np.count_nonzero(tumor))
    if n_tumor == 0:
        raise RuntimeError(f"mask label {label} is empty")

    values = stack[tumor]
    means = np.zeros(values.shape[1])
    scales = np.ones(values.shape[1])
    if bool(model.get("standardize", True)):
        means = values.mean(axis=0)
        scales = values.std(axis=0)
        if np.any(scales == 0.0):
            raise RuntimeError("assignment case has a constant channel; cannot standardize")
        values = (values - means) / scales
    assignments = ((values[:, None, :] - centers[None, :, :]) ** 2).sum(axis=2).argmin(axis=1)

    label_volume = np.zeros(stack.shape[:3], dtype=np.uint8)
    label_volume[tumor] = (assignments + 1).astype(np.uint8)
    habitat_mask = sitk.GetImageFromArray(label_volume)
    habitat_mask.CopyInformation(mask)
    sitk.WriteImage(habitat_mask, str(output_dir(0)), True)

    spacing_zyx = np.asarray(list(reversed(mask.GetSpacing())), dtype=np.float64)
    voxel_volume = float(np.prod(spacing_zyx))
    interface = habitat_interface_fraction(label_volume)
    rows = []
    coordinates = np.array(np.nonzero(tumor), dtype=np.float64).T * spacing_zyx
    for habitat in range(n_habitats):
        members = assignments == habitat
        n_members = int(np.count_nonzero(members))
        member_coordinates = coordinates[members]
        dispersion = (
            float(np.linalg.norm(member_coordinates - member_coordinates.mean(axis=0), axis=1).mean())
            if n_members
            else 0.0
        )
        raw = stack[tumor][members]
        row = {
            "habitat": habitat + 1,
            "n_voxels": n_members,
            "volume_mm3": float(n_members * voxel_volume),
            "volume_fraction": float(n_members / n_tumor),
            "dispersion_mm": dispersion,
            "interface_fraction": float(interface[habitat]),
        }
        for channel_index, name in enumerate(channel_names):
            suffix = name.rsplit(".", 1)[0]
            row[f"mean_{suffix}"] = float(raw[:, channel_index].mean()) if n_members else 0.0
            row[f"std_{suffix}"] = float(raw[:, channel_index].std()) if n_members else 0.0
        rows.append(row)
    pd.DataFrame(rows).to_parquet(output_dir(1), index=False)
    write_json(
        2,
        {
            "settings": settings,
            "n_habitats": int(n_habitats),
            "channel_names": channel_names,
            "channel_means": means.tolist(),
            "channel_stds": scales.tolist(),
            "n_tumor_voxels": n_tumor,
            "tumor_volume_mm3": float(n_tumor * voxel_volume),
            "mask_hash": sha256(input_paths(0)[0]),
            "model_hash": sha256(input_paths(2)[0]),
        },
    )


PERTURBATION_AXES = {"translate_x": 2, "translate_y": 1, "translate_z": 0}


def perturb_stability() -> None:
    """Re-extract features under controlled mask/image perturbations."""
    extraction = json.loads(os.environ.get("RADIOMICS_EXTRACTION", "{}"))
    settings = json.loads(os.environ.get("RADIOMICS_PERTURB_SETTINGS", "{}"))
    required = ["extraction_id", "patient_id", "image_id", "roi_id", "modality", "preset_id"]
    missing = [key for key in required if not extraction.get(key)]
    if missing:
        raise RuntimeError(f"missing extraction metadata: {', '.join(missing)}")

    allowed = ["dilate1", "erode1", *PERTURBATION_AXES, "noise"]
    perturbations = [
        str(value) for value in settings.get(
            "perturbations", ["dilate1", "erode1", "translate_x", "translate_y", "translate_z", "noise"]
        )
    ]
    unknown = [value for value in perturbations if value not in allowed]
    if unknown:
        raise RuntimeError(
            f"unsupported perturbations: {', '.join(unknown)}; expected one of {allowed}"
        )
    if len(set(perturbations)) != len(perturbations):
        raise RuntimeError("perturbations must be unique")
    sigma_pct = float(settings.get("noise_sigma_pct", 2.0))
    seed = int(settings.get("seed", 0))
    if not (sigma_pct > 0.0):
        raise RuntimeError("noise_sigma_pct must be positive")

    image = read_single_image(input_paths(0)[0])
    mask = read_single_image(input_paths(1)[0])
    require_same_geometry(image, mask, "mask")
    label = int(extraction_settings().get("mask_label", 1))
    mask_array = sitk.GetArrayFromImage(mask) == label
    if np.count_nonzero(mask_array) == 0:
        raise RuntimeError(f"mask label {label} is empty")
    image_array = sitk.GetArrayFromImage(image).astype(np.float64)
    roi_values = image_array[mask_array]
    roi_std = float(roi_values.std())
    if roi_std == 0.0:
        raise RuntimeError("ROI intensity is constant; noise perturbation is undefined")
    sigma = sigma_pct / 100.0 * roi_std
    rng = np.random.default_rng(seed)
    noise = rng.normal(0.0, sigma, image_array.shape)

    def as_image(array: np.ndarray, reference: sitk.Image) -> sitk.Image:
        written = sitk.GetImageFromArray(array)
        written.CopyInformation(reference)
        return written

    replicates: list[tuple[str, sitk.Image, sitk.Image]] = [
        ("original", image, as_image(mask_array.astype(np.uint8), mask))
    ]
    structure = ndimage.generate_binary_structure(3, 1)
    if "dilate1" in perturbations:
        replicates.append(
            ("dilate1", image, as_image(ndimage.binary_dilation(mask_array, structure).astype(np.uint8), mask))
        )
    if "erode1" in perturbations:
        eroded = ndimage.binary_erosion(mask_array, structure)
        if np.count_nonzero(eroded) == 0:
            raise RuntimeError("erode1 emptied the mask; ROI is too small to perturb")
        replicates.append(("erode1", image, as_image(eroded.astype(np.uint8), mask)))
    for name in ("translate_x", "translate_y", "translate_z"):
        if name not in perturbations:
            continue
        shift = [0.0, 0.0, 0.0]
        shift[PERTURBATION_AXES[name]] = 1.0
        shifted = ndimage.shift(mask_array.astype(np.uint8), shift, order=0, mode="constant", cval=0)
        if np.count_nonzero(shifted) == 0:
            raise RuntimeError(f"{name} emptied the mask")
        replicates.append((name, image, as_image(shifted, mask)))
    if "noise" in perturbations:
        noisy = as_image((image_array + noise).astype(np.float32), image)
        replicates.append(("noise", noisy, as_image(mask_array.astype(np.uint8), mask)))

    extractor = make_extractor()
    wide_rows: list[dict[str, Any]] = []
    long_rows: list[dict[str, Any]] = []
    metadata_rows: list[dict[str, Any]] = []
    replicate_meta: list[dict[str, Any]] = []
    original_voxels = int(np.count_nonzero(mask_array))
    for replicate_id, (name, replicate_image, replicate_mask) in enumerate(replicates):
        replicate_mask_array = sitk.GetArrayFromImage(replicate_mask) != 0
        n_voxels = int(np.count_nonzero(replicate_mask_array))
        row_extraction = {**extraction, "replicate_id": replicate_id, "perturbation": name}
        result = extractor.execute(replicate_image, replicate_mask)
        wide, longs, metadata = result_rows(row_extraction, dict(result))
        if not longs:
            raise RuntimeError(f"PyRadiomics returned no features for perturbation `{name}`")
        wide_rows.append(wide)
        long_rows.extend(longs)
        metadata_rows.extend(metadata)
        replicate_meta.append(
            {
                "replicate_id": replicate_id,
                "perturbation": name,
                "mask_voxels": n_voxels,
                "voxel_delta_vs_original": n_voxels - original_voxels,
            }
        )

    pd.DataFrame(wide_rows).to_parquet(output_dir(0), index=False)
    pd.DataFrame(long_rows).to_parquet(output_dir(1), index=False)
    write_json(
        2,
        {
            "settings": settings,
            "extraction": extraction,
            "replicates": replicate_meta,
            "roi_std": roi_std,
            "noise_sigma": sigma,
            "image_hash": sha256(input_paths(0)[0]),
            "mask_hash": sha256(input_paths(1)[0]),
        },
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=[
        "ingest-image",
        "ingest-mask",
        "validate-pair",
        "preprocess",
        "extract-single",
        "extract-batch",
        "dicom-metadata",
        "phi-scrub",
        "voi-similarity",
        "image-qc",
        "rtstruct-geometry",
        "ivh-extract",
        "shape-topology",
        "register",
        "delta-features",
        "bias-correct",
        "normalize",
        "peritumoral-ring",
        "habitat-fit",
        "habitat-assign",
        "perturb-stability",
    ])
    args = parser.parse_args()
    commands = {
        "ingest-image": ingest_image,
        "ingest-mask": ingest_mask,
        "validate-pair": validate_pair,
        "preprocess": preprocess,
        "extract-single": extract_single,
        "extract-batch": extract_batch,
        "dicom-metadata": dicom_metadata,
        "phi-scrub": phi_scrub,
        "voi-similarity": voi_similarity,
        "image-qc": image_qc,
        "rtstruct-geometry": rtstruct_geometry,
        "ivh-extract": ivh_extract,
        "shape-topology": shape_topology,
        "register": register_images,
        "delta-features": delta_features,
        "bias-correct": bias_correct,
        "normalize": robust_normalize,
        "peritumoral-ring": peritumoral_ring,
        "habitat-fit": habitat_fit,
        "habitat-assign": habitat_assign,
        "perturb-stability": perturb_stability,
    }
    commands[args.command]()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"radiomics_runner: {error}", file=sys.stderr)
        raise
