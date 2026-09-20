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
sha256:bccbe15b2ec8d079e1bf869c4f06bfe4143642015394453c584dc981e5403fbe
```

The wrapper combines this digest with `$ACR_ENDPOINT/autonomics/pyradiomics`.
Rebuild, retag, republish, and update both this file and
`PYRADIOMICS_IMAGE_DIGEST` whenever any dependency changes.

The local Podman build and the published ACR manifest can have different
digests after registry normalization. Always pin the digest returned by the
published ACR tag, not the pre-push local digest.

## Commands

Beyond the Phase-A ingestion, validation, extraction, QC, registration, and
delta commands exercised by `test_radiomics_phasea.sh`, the runner provides
the analysis-side commands used by the radiomics SAP:

| Command             | Purpose                                                                                              |
|---------------------|------------------------------------------------------------------------------------------------------|
| `bias-correct`      | Mask-guided N4 inhomogeneity correction at a configurable shrink factor; emits corrected image, full-resolution bias field, and convergence metadata. |
| `normalize`         | Percentile-truncated robust z-score over a tissue mask, applied to the whole image; every fitted parameter is recorded. No cohort-level statistics. |
| `peritumoral-ring`  | Physical-radius ball dilation minus tumor and an exclusion mask (bone/air/outside-body); emits ring-only, tumor=1/ring=2, and combined masks plus volume accounting. |
| `habitat-fit`       | Manifest-driven multi-case fit of common habitat centers: equal-size seeded voxel samples, per-case channel z-scores, pooled deterministic k-means, labels frozen by descending first-channel center. |
| `habitat-assign`    | Assigns every tumor voxel of one case to the frozen centers; emits a labeled habitat mask and per-habitat volume fraction, dispersion, interface fraction, and raw channel statistics. |
| `perturb-stability` | Re-runs PyRadiomics under controlled perturbations (one-voxel dilation/erosion/translations, Gaussian noise at a percentage of the ROI SD); emits per-replicate feature tables for downstream cohort-level ICC(2,1) in Rust. |

All six are deterministic given their seed settings; `habitat-fit` output is
byte-identical across runs with the same inputs and seed, which is what makes
fold-internal common centers and frozen external-center assignment leak-free.

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
