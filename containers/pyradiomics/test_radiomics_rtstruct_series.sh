#!/usr/bin/env sh
set -eu

IMAGE=${AUTONOMICS_PYRADIOMICS_IMAGE:-localhost/atc/pyradiomics:3.1.0-r2}
PHANTOM_DIR=${AUTONOMICS_IBSI1_CT_DIR:-/mnt/data/ibsi_dataset/ibsi_1_ct_radiomics_phantom}

if command -v podman >/dev/null 2>&1; then
  CONTAINER_CLI=podman
else
  CONTAINER_CLI=docker
fi

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT
DICOM_INPUTS=$(
  find "$PHANTOM_DIR/dicom/image" -maxdepth 1 -name '*.dcm' -type f \
    | sort | sed "s#^$PHANTOM_DIR#/data#" | paste -sd, -
)

"$CONTAINER_CLI" run --rm \
  -v "$PHANTOM_DIR:/data:ro" \
  -v "$WORK_DIR:/work" \
  -e "AUTONOMICS_INPUT0=$DICOM_INPUTS" \
  -e AUTONOMICS_OUTPUT0=/work/image.mha \
  -e AUTONOMICS_OUTPUT1=/work/image-meta.json \
  "$IMAGE" ingest-image

"$CONTAINER_CLI" run --rm \
  -v "$PHANTOM_DIR:/data:ro" \
  -v "$WORK_DIR:/work" \
  -e "AUTONOMICS_INPUT0=$DICOM_INPUTS" \
  -e AUTONOMICS_INPUT1=/data/dicom/mask/DCM_RS_00060.dcm \
  -e AUTONOMICS_OUTPUT0=/work/mask.mha \
  -e AUTONOMICS_OUTPUT1=/work/roi-meta.json \
  "$IMAGE" ingest-mask

"$CONTAINER_CLI" run --rm \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/work/image.mha \
  -e AUTONOMICS_INPUT1=/work/mask.mha \
  -e AUTONOMICS_OUTPUT0=/work/features-wide.parquet \
  -e AUTONOMICS_OUTPUT1=/work/features-long.parquet \
  -e AUTONOMICS_OUTPUT2=/work/feature-metadata.parquet \
  -e AUTONOMICS_OUTPUT3=/work/diagnostics.parquet \
  -e AUTONOMICS_OUTPUT4=/work/provenance.json \
  -e 'RADIOMICS_EXTRACTION={"extraction_id":"ibsi1_rtstruct","patient_id":"IBSI1","image_id":"CT","roi_id":"GTV-1","roi_name":"GTV-1","modality":"CT","preset_id":"pyradiomics_original_v1"}' \
  "$IMAGE" extract-single

"$CONTAINER_CLI" run --rm --entrypoint python \
  -v "$WORK_DIR:/work" \
  "$IMAGE" -c $'import numpy as np\nimport SimpleITK as sitk\nimage=sitk.ReadImage("/work/image.mha")\nmask=sitk.GetImageFromArray(np.zeros(image.GetSize()[::-1], dtype="uint8"))\nmask.CopyInformation(image)\nsitk.WriteImage(mask, "/work/empty.mha", True)'

if "$CONTAINER_CLI" run --rm \
  -v "$WORK_DIR:/work" \
  -e AUTONOMICS_INPUT0=/work/image.mha \
  -e AUTONOMICS_INPUT1=/work/empty.mha \
  -e AUTONOMICS_OUTPUT0=/work/empty-features.parquet \
  -e AUTONOMICS_OUTPUT1=/work/empty-features-long.parquet \
  -e AUTONOMICS_OUTPUT2=/work/empty-metadata.parquet \
  -e AUTONOMICS_OUTPUT3=/work/empty-diagnostics.parquet \
  -e AUTONOMICS_OUTPUT4=/work/empty-provenance.json \
  -e 'RADIOMICS_EXTRACTION={"extraction_id":"empty","patient_id":"IBSI1","image_id":"CT","roi_id":"GTV-1","roi_name":"GTV-1","modality":"CT","preset_id":"pyradiomics_original_v1"}' \
  "$IMAGE" extract-single >"$WORK_DIR/empty-extract.log" 2>&1
then
  printf '%s\n' 'empty-mask extraction unexpectedly succeeded' >&2
  exit 1
fi

"$CONTAINER_CLI" run --rm --entrypoint python \
  -v "$PHANTOM_DIR:/data:ro" \
  -v "$WORK_DIR:/work:ro" \
  "$IMAGE" -c $'import json\nimport numpy as np\nimport pandas as pd\nimport SimpleITK as sitk\nimage_meta=json.load(open("/work/image-meta.json"))\nroi_meta=json.load(open("/work/roi-meta.json"))\nwide=pd.read_parquet("/work/features-wide.parquet")\nlong=pd.read_parquet("/work/features-long.parquet")\nactual=sitk.GetArrayFromImage(sitk.ReadImage("/work/mask.mha"))\nexpected=sitk.GetArrayFromImage(sitk.ReadImage("/data/nifti/mask/mask.nii.gz"))\nordered=image_meta["dicom_series"]["z_sort_files"]\nassert image_meta["size"] == [204, 201, 60]\nassert ordered[0].endswith("DCM_IMG_00059.dcm")\nassert ordered[-1].endswith("DCM_IMG_00000.dcm")\nassert roi_meta["voxel_count"] == 125256\nassert np.array_equal(actual, expected)\nassert wide.loc[0, "status"] == "valid"\nassert len(long) == 107'

# The public phantom omits ContourImageSequence, so inject its slice UIDs to
# exercise the exact-reference mapping path as well as the geometric fallback.
"$CONTAINER_CLI" run --rm --entrypoint python \
  -v "$PHANTOM_DIR:/data:ro" \
  -v "$WORK_DIR:/work" \
  "$IMAGE" -c $'import glob\nimport pydicom\nrs=pydicom.dcmread("/data/dicom/mask/DCM_RS_00060.dcm")\nimages=[pydicom.dcmread(path, stop_before_pixels=True) for path in glob.glob("/data/dicom/image/*.dcm")]\nfor contour in rs.ROIContourSequence[0].ContourSequence:\n    z=float(contour.ContourData[2::3][0])\n    image=min(images, key=lambda item: abs(float(item.ImagePositionPatient[2]) - z))\n    reference=pydicom.Dataset()\n    reference.ReferencedSOPClassUID=image.SOPClassUID\n    reference.ReferencedSOPInstanceUID=image.SOPInstanceUID\n    contour.ContourImageSequence=[reference]\npydicom.dcmwrite("/work/rs-with-references.dcm", rs)'

"$CONTAINER_CLI" run --rm \
  -v "$PHANTOM_DIR:/data:ro" \
  -v "$WORK_DIR:/work" \
  -e "AUTONOMICS_INPUT0=$DICOM_INPUTS" \
  -e AUTONOMICS_INPUT1=/work/rs-with-references.dcm \
  -e AUTONOMICS_OUTPUT0=/work/uid-mask.mha \
  -e AUTONOMICS_OUTPUT1=/work/uid-roi-meta.json \
  "$IMAGE" ingest-mask

"$CONTAINER_CLI" run --rm --entrypoint python \
  -v "$WORK_DIR:/work:ro" \
  "$IMAGE" -c $'import json\nimport SimpleITK as sitk\nmeta=json.load(open("/work/uid-roi-meta.json"))\nassert meta["contours_mapped_by_sop_instance_uid"] == 26\nassert int(sitk.GetArrayFromImage(sitk.ReadImage("/work/uid-mask.mha")).sum()) == 125256'

printf '%s\n' "IBSI-1 RTSTRUCT series regression passed"
