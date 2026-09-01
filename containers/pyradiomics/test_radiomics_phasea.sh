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
  -e AUTONOMICS_OUTPUT0=/work/dicom-metadata.parquet \
  -e AUTONOMICS_OUTPUT1=/work/dicom-metadata.json \
  -e 'RADIOMICS_DICOM_METADATA_SETTINGS={"extra_tags":["PatientAge"]}' \
  "$IMAGE" dicom-metadata

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/dicom/$SUBJECT/CT/image/000000.dcm:/input.dcm:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input.dcm \
  -e AUTONOMICS_OUTPUT0=/work/scrubbed.zip \
  -e AUTONOMICS_OUTPUT1=/work/phi-report.parquet \
  -e AUTONOMICS_OUTPUT2=/work/phi-report.json \
  -e 'RADIOMICS_PHI_SCRUB_SETTINGS={"keep_patient_id":true,"pseudonym":"IBSI-TEST"}' \
  "$IMAGE" phi-scrub

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input/CT_mask.nii.gz \
  -e AUTONOMICS_INPUT1=/input/CT_mask.nii.gz \
  -e AUTONOMICS_OUTPUT0=/work/voi.parquet \
  -e AUTONOMICS_OUTPUT1=/work/voi.json \
  -e 'RADIOMICS_VOI_SETTINGS={"comparison_id":"same","mask_label":1,"surface_tolerance_mm":1}' \
  "$IMAGE" voi-similarity

printf '%s\n' \
  "import SimpleITK as sitk" \
  "mask = sitk.ReadImage('/input/CT_mask.nii.gz')" \
  "array = sitk.GetArrayFromImage(mask)" \
  "background = sitk.GetImageFromArray((array == 0).astype('uint8'))" \
  "background.CopyInformation(mask)" \
  "sitk.WriteImage(background, '/work/background.mha', True)" \
| "$CONTAINER_CLI" run --rm -i --entrypoint python \
    -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
    -v "$WORK_DIR:/work" \
    "$IMAGE"

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input/CT_image.nii.gz \
  -e AUTONOMICS_INPUT1=/input/CT_mask.nii.gz \
  -e AUTONOMICS_INPUT2=/work/background.mha \
  -e AUTONOMICS_OUTPUT0=/work/image-qc.parquet \
  -e AUTONOMICS_OUTPUT1=/work/image-qc.json \
  -e 'RADIOMICS_IMAGE_QC_SETTINGS={"extraction_id":"sts001_ct","mask_label":1}' \
  "$IMAGE" image-qc

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/dicom/$SUBJECT/CT/mask/RS.dcm:/rt.dcm:ro" \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/rt.dcm \
  -e AUTONOMICS_INPUT1=/input/CT_image.nii.gz \
  -e AUTONOMICS_OUTPUT0=/work/rt-geometry.parquet \
  -e AUTONOMICS_OUTPUT1=/work/rt-geometry.json \
  -e 'RADIOMICS_RTSTRUCT_GEOMETRY_SETTINGS={"roi_name":"GTV_Mass_CT"}' \
  "$IMAGE" rtstruct-geometry

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input/CT_image.nii.gz \
  -e AUTONOMICS_INPUT1=/input/CT_mask.nii.gz \
  -e AUTONOMICS_OUTPUT0=/work/ivh.parquet \
  -e AUTONOMICS_OUTPUT1=/work/ivh.json \
  -e 'RADIOMICS_IVH_SETTINGS={"extraction_id":"sts001_ct","mask_label":1,"volume_fractions":[0.1,0.5,0.9]}' \
  "$IMAGE" ivh-extract

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input/CT_mask.nii.gz \
  -e AUTONOMICS_OUTPUT0=/work/topology.parquet \
  -e AUTONOMICS_OUTPUT1=/work/slice-profile.parquet \
  -e AUTONOMICS_OUTPUT2=/work/topology.json \
  -e 'RADIOMICS_SHAPE_TOPOLOGY_SETTINGS={"extraction_id":"sts001_ct","mask_label":1}' \
  "$IMAGE" shape-topology

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input/CT_image.nii.gz \
  -e AUTONOMICS_INPUT1=/input/CT_image.nii.gz \
  -e AUTONOMICS_OUTPUT0=/work/registered.mha \
  -e AUTONOMICS_OUTPUT1=/work/transform.tfm \
  -e AUTONOMICS_OUTPUT2=/work/registration.json \
  -e 'RADIOMICS_REGISTER_SETTINGS={"transform_type":"rigid","iterations":1,"learning_rate":1,"sampling_percent":0.1}' \
  "$IMAGE" register

cat > "$WORK_DIR/delta.csv" <<'CSV'
patient_id,timepoint,feature_a
p1,baseline,10
p1,followup,15
CSV
"$CONTAINER_CLI" run --rm \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/work/delta.csv \
  -e AUTONOMICS_OUTPUT0=/work/delta.parquet \
  -e AUTONOMICS_OUTPUT1=/work/delta.json \
  -e 'RADIOMICS_DELTA_SETTINGS={"id_column":"patient_id","timepoint_column":"timepoint","baseline":"baseline","followup":"followup"}' \
  "$IMAGE" delta-features

"$CONTAINER_CLI" run --rm \
  -v "$VALIDATION_DIR/nifti/$SUBJECT:/input:ro" \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/input/PET_image.nii.gz \
  -e AUTONOMICS_INPUT1=/input/PET_mask.nii.gz \
  -e AUTONOMICS_OUTPUT0=/work/features-wide.parquet \
  -e AUTONOMICS_OUTPUT1=/work/features-long.parquet \
  -e AUTONOMICS_OUTPUT2=/work/feature-metadata.parquet \
  -e AUTONOMICS_OUTPUT3=/work/diagnostics.parquet \
  -e AUTONOMICS_OUTPUT4=/work/provenance.json \
  -e 'RADIOMICS_EXTRACT_SETTINGS={"mask_label":1,"bin_width":25,"image_types":["Original","LoG","Wavelet","Square","SquareRoot","Logarithm","Exponential","Gradient","LBP2D","LBP3D"],"feature_classes":["firstorder"],"log_sigmas":[1]}' \
  -e 'RADIOMICS_EXTRACTION={"extraction_id":"sts001_pet_all_types","patient_id":"STS_001","image_id":"PET","roi_id":"GTV_Mass","roi_name":"GTV_Mass","modality":"PET","preset_id":"pyradiomics_all_v1"}' \
  "$IMAGE" extract-single

printf '%s\n' \
  "import json" \
  "import pandas as pd" \
  "assert pd.read_parquet('/work/dicom-metadata.parquet').shape[0] == 1" \
  "assert json.load(open('/work/phi-report.json'))['n_files'] == 1" \
  "assert pd.read_parquet('/work/voi.parquet').loc[0, 'dice'] == 1" \
  "assert pd.read_parquet('/work/image-qc.parquet').loc[0, 'status'] == 'valid'" \
  "assert pd.read_parquet('/work/rt-geometry.parquet').loc[0, 'status'] == 'valid'" \
  "assert len(pd.read_parquet('/work/ivh.parquet')) == 3" \
  "assert pd.read_parquet('/work/topology.parquet').loc[0, 'connected_component_count'] == 1" \
  "assert 'metric_value' in json.load(open('/work/registration.json'))" \
  "assert pd.read_parquet('/work/delta.parquet').loc[0, 'absolute_change'] == 5" \
  "features = pd.read_parquet('/work/features-wide.parquet')" \
  "metadata = pd.read_parquet('/work/feature-metadata.parquet')" \
  "assert features.loc[0, 'status'] == 'valid'" \
  "assert metadata.image_type.nunique() == 10" \
  "print('Phase-A full node smoke test passed')" \
| "$CONTAINER_CLI" run --rm -i --entrypoint python -v "$WORK_DIR:/work:ro" "$IMAGE"
