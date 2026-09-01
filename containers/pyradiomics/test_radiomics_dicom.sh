#!/usr/bin/env sh
set -eu

IMAGE=${AUTONOMICS_PYRADIOMICS_IMAGE:-localhost/atc/pyradiomics:3.1.0-r2}
IBSI_DIR=${AUTONOMICS_IBSI_DIR:-/mnt/data/ibsi_dataset}
VALIDATION_DIR="$IBSI_DIR/ibsi_validation"
SUBJECT=STS_001

if command -v podman >/dev/null 2>&1; then
  CONTAINER_CLI=podman
else
  CONTAINER_CLI=docker
fi

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/dicom/$SUBJECT/CT/image/000000.dcm:/input.dcm:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input.dcm \
  -e AUTONOMICS_OUTPUT0=/work/single.mha \
  -e AUTONOMICS_OUTPUT1=/work/single-meta.json \
  "$IMAGE" ingest-image

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/dicom/$SUBJECT/CT/mask/RS.dcm:/rtstruct.dcm:ro" \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/nifti:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/nifti/CT_image.nii.gz \
  -e AUTONOMICS_INPUT1=/rtstruct.dcm \
  -e AUTONOMICS_OUTPUT0=/work/mask.mha \
  -e AUTONOMICS_OUTPUT1=/work/roi-meta.json \
  -e 'RADIOMICS_MASK_SETTINGS={"roi_name":"GTV_Mass_CT"}' \
  "$IMAGE" ingest-mask

printf '%s\n' \
  "import json" \
  "import SimpleITK as sitk" \
  "import numpy as np" \
  "metadata = json.load(open('/work/single-meta.json'))" \
  "assert len(metadata['size']) in (2, 3)" \
  "actual = sitk.GetArrayFromImage(sitk.ReadImage('/work/mask.mha'))" \
  "expected = sitk.GetArrayFromImage(sitk.ReadImage('/nifti/CT_mask.nii.gz'))" \
  "dice = 2 * np.logical_and(actual, expected).sum() / (actual.sum() + expected.sum())" \
  "assert actual.sum() == expected.sum() and dice == 1.0" \
  "print('DICOM smoke test passed')" \
| "$CONTAINER_CLI" run --rm -i --entrypoint python \
    -v "$WORK_DIR:/work:ro" \
    -v "$VALIDATION_DIR/nifti/$SUBJECT:/nifti:ro" \
    "$IMAGE"
