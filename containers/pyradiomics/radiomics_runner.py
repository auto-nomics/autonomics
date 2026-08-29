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
from typing import Any, Sequence

import numpy as np
import pandas as pd
import pydicom
import SimpleITK as sitk
from scipy import ndimage
from scipy.spatial import ConvexHull


SUPPORTED_IMAGE_SUFFIXES = {".nii", ".nii.gz", ".mha", ".mhd", ".nrrd"}
RTSTRUCT_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.481.3"


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


def read_image(paths: Sequence[Path]) -> sitk.Image:
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
    reader = sitk.ImageSeriesReader()
    reader.SetFileNames([str(path) for path in dicom_files])
    return reader.Execute()


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
    image = read_image(paths)
    sitk.WriteImage(image, str(output_dir(0)), True)
    write_json(1, image_metadata(image, paths))


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


def rtstruct_to_mask(rtstruct_path: Path, reference: sitk.Image, roi_name: str | None) -> sitk.Image:
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
    for contour in contours:
        points = np.asarray(contour.ContourData, dtype=float).reshape(-1, 3)
        continuous = np.asarray(
            [
                reference.TransformPhysicalPointToContinuousIndex(tuple(point))
                for point in points
            ]
        )
        slice_index = int(round(float(np.median(continuous[:, 2]))))
        if slice_index < 0 or slice_index >= array.shape[0]:
            continue
        rows, columns = polygon(
            continuous[:, 1],
            continuous[:, 0],
            shape=array.shape[1:],
        )
        array[slice_index, rows, columns] = 1
    mask = sitk.GetImageFromArray(array)
    mask.CopyInformation(reference)
    return mask


def ingest_mask() -> None:
    image_sources = input_paths(0)
    mask_sources = input_paths(1)
    reference = read_image(image_sources)
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
        settings = json.loads(os.environ.get("RADIOMICS_MASK_SETTINGS", "{}"))
        mask = rtstruct_to_mask(rtstruct, reference, settings.get("roi_name"))
    sitk.WriteImage(mask, str(output_dir(0)), True)
    write_json(
        1,
        {
            "mask": image_metadata(mask, mask_sources),
            "reference_image": image_metadata(reference, image_sources),
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
        try:
            row_label = int(extraction.get("mask_label", default_label))
            extractor.settings["label"] = row_label
            row_settings = dict(extraction_settings())
            row_settings["mask_label"] = row_label
            image_hash = sha256(images[index])
            mask_hash = sha256(masks[index])
            image = read_single_image(images[index])
            mask = read_single_image(masks[index])
            result = extractor.execute(image, mask)
            wide, longs, metadata = result_rows(extraction, dict(result))
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
                    "settings": extraction_settings(),
                    "status": "invalid",
                    "error_code": str(error),
                }
            )

    pd.DataFrame(wide_rows).to_parquet(output_dir(0), index=False)
    pd.DataFrame(long_rows).to_parquet(output_dir(1), index=False)
    pd.DataFrame(metadata_rows).drop_duplicates("feature_id").to_parquet(output_dir(2), index=False)
    pd.DataFrame(diagnostic_rows).to_parquet(output_dir(3), index=False)
    write_json(4, {"extractions": provenance})


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
    }
    commands[args.command]()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"radiomics_runner: {error}", file=sys.stderr)
        raise
