#!/usr/bin/env sh
set -eu

IMAGE=${AUTONOMICS_PYRADIOMICS_IMAGE:-localhost/atc/pyradiomics:3.1.0-r1}
IBSI_DIR=${AUTONOMICS_IBSI_DIR:-/mnt/data/ibsi_dataset}
CASE_DIR="$IBSI_DIR/ibsi_validation/nifti/STS_001"

if command -v podman >/dev/null 2>&1; then
  CONTAINER_CLI=podman
else
  CONTAINER_CLI=docker
fi

WORK_DIR=$(mktemp -d)
STAGE_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR" "$STAGE_DIR"' EXIT

# Reproduce ContainerCommand staging's minimal extension without depending on
# that older behavior: a generic `.gz` filename must still decode as NIfTI.
cp "$CASE_DIR/CT_image.nii.gz" "$STAGE_DIR/input-0.gz"
cp "$CASE_DIR/CT_mask.nii.gz" "$STAGE_DIR/input-1.gz"

"$CONTAINER_CLI" run --rm \
  -v "$STAGE_DIR:/stage:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/stage/input-0.gz \
  -e AUTONOMICS_INPUT1=/stage/input-1.gz \
  -e AUTONOMICS_OUTPUT0=/work/pair_validation.parquet \
  -e AUTONOMICS_OUTPUT1=/work/geometry.json \
  -e 'RADIOMICS_VALIDATE_SETTINGS={"extraction_id":"sts001_ct","mask_label":1,"minimum_mask_voxels":1,"geometry_tolerance_mm":0.01}' \
  "$IMAGE" validate-pair

"$CONTAINER_CLI" run --rm \
  -v "$STAGE_DIR:/stage:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/stage/input-0.gz \
  -e AUTONOMICS_INPUT1=/stage/input-1.gz \
  -e AUTONOMICS_OUTPUT0=/work/features_wide.parquet \
  -e AUTONOMICS_OUTPUT1=/work/features_long.parquet \
  -e AUTONOMICS_OUTPUT2=/work/feature_metadata.parquet \
  -e AUTONOMICS_OUTPUT3=/work/diagnostics.parquet \
  -e AUTONOMICS_OUTPUT4=/work/provenance.json \
  -e 'RADIOMICS_EXTRACT_SETTINGS={"mask_label":1,"bin_width":25,"image_types":["Original"],"feature_classes":["shape","firstorder"]}' \
  -e 'RADIOMICS_EXTRACTION={"extraction_id":"sts001_ct","patient_id":"STS_001","image_id":"CT","roi_id":"GTV_Mass","roi_name":"GTV_Mass","modality":"CT","preset_id":"pyradiomics_original_v1"}' \
  "$IMAGE" extract-single

printf '%s\n' \
  "import json, pandas as pd" \
  "assert json.load(open('/work/geometry.json'))['valid']" \
  "w = pd.read_parquet('/work/features_wide.parquet')" \
  "assert w.shape[0] == 1 and w.loc[0, 'status'] == 'valid'" \
  "assert len(pd.read_parquet('/work/features_long.parquet')) >= 30" \
  "print('radiomics smoke test:', w.shape)" \
| "$CONTAINER_CLI" run --rm -i --entrypoint python -v "$WORK_DIR:/work:ro" "$IMAGE"
