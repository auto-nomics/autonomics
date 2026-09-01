# PyRadiomics Stage-A image

This image packages the official PyRadiomics `v3.1.0` source release with a
pinned medical-imaging stack. It is used by every radiomics container node in
`nodes-io`; the Rust wrappers contain no feature computation logic.

## Versions

- Base: `docker.io/library/python:3.11.11-slim@sha256:a8e0a3090316aed0b11037aac613aef32fb1747dcc1dcb5c0f6c727a0113a07f`
- PyRadiomics: `v3.1.0`
- NumPy: `1.26.4`
- SimpleITK: `2.3.1`
- PyWavelets: `1.5.0`
- PyArrow: `15.0.2`
- pandas: `2.2.3`
- pydicom: `2.4.4`
- scikit-image: `0.22.0`
- trimesh: `4.6.13`

Published immutable ACR image digest:

```text
sha256:4ef0fc2abbd5a85812b04bceef70b03f207494dbaa53a06c1a3eb9e24b3e7392
```

The wrapper combines this digest with `$ACR_ENDPOINT/autonomics/pyradiomics`.
Rebuild, retag, republish, and update both this file and
`PYRADIOMICS_IMAGE_DIGEST` whenever any dependency changes.

The local Podman build and the published ACR manifest can have different
digests after registry normalization. Always pin the digest returned by the
published ACR tag, not the pre-push local digest.

## Build and test

```bash
podman build \
  -f containers/pyradiomics/Dockerfile \
  -t localhost/atc/pyradiomics:3.1.0-r2 \
  containers/pyradiomics

AUTONOMICS_IBSI_DIR=/mnt/data/ibsi_dataset \
containers/pyradiomics/test_radiomics_nifti.sh

AUTONOMICS_IBSI_DIR=/mnt/data/ibsi_dataset \
containers/pyradiomics/test_radiomics_dicom.sh

AUTONOMICS_IBSI1_CT_DIR=/mnt/data/ibsi_dataset/ibsi_1_ct_radiomics_phantom \
containers/pyradiomics/test_radiomics_rtstruct_series.sh

AUTONOMICS_IBSI_DIR=/mnt/data/ibsi_dataset \
containers/pyradiomics/test_radiomics_phasea.sh
```

Publish the build to the configured Aliyun ACR registry:

```bash
ACR_IMAGE="$ACR_ENDPOINT/autonomics/pyradiomics:3.1.0-r2"
podman tag localhost/atc/pyradiomics:3.1.0-r2 "$ACR_IMAGE"
podman push "$ACR_IMAGE"
```

`test_radiomics_nifti.sh` performs geometry validation and a shape/first-order
extraction on IBSI `STS_001` CT data. `test_radiomics_dicom.sh` covers
single-file DICOM ingestion and RTSTRUCT rasterization.
`test_radiomics_rtstruct_series.sh` checks DICOM position ordering, geometric
RTSTRUCT fallback, SOPInstanceUID contour mapping, empty-mask failure behavior,
and the IBSI-1 CT phantom feature extraction.
`test_radiomics_phasea.sh` exercises every containerized Phase-A command. The
tests require the local IBSI data checkout; the image itself contains no patient
or phantom data.
