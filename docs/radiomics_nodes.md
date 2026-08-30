# Radiomics Stage-A Nodes

Stage A converts paired medical images and ROIs into auditable feature tables.
It deliberately stops before outcome-aware filtering, training-set
normalization, feature selection, cross-validation, or modeling. Those analyses
belong to the existing DataFrame DAG nodes in Stage B.

## Node catalogue

| Kind | Ports | Purpose |
|---|---|---|
| `radiomics_manifest_build` | DataFrame -> manifest, diagnostics | Normalizes extraction units and validates identifiers/paths |
| `radiomics_stage_file_set` | DataFrame -> FileSet | Resolves image or mask paths in manifest order |
| `radiomics_dcm_glob` | -> DICOM FileSet | Expands and sorts a local or VFS DICOM glob |
| `radiomics_image_ingest` | File/Any -> image File, metadata File | Reads NIfTI/MHA or a DICOM series and writes MHA |
| `radiomics_mask_ingest` | port 0 reference image Any + port 1 mask/RTSTRUCT Any -> mask File, metadata File | Reads voxel masks or rasterizes DICOM RTSTRUCT |
| `radiomics_pair_validate` | image File + mask File -> validation File, geometry File | Checks size, spacing, origin, direction, label, and ROI size |
| `radiomics_preprocess` | image File + mask File -> image, mask, metadata Files | Deterministic resampling, resegmentation, and optional within-ROI normalization |
| `pyradiomics_extract` | image File + mask File -> five Files | One PyRadiomics extraction unit |
| `pyradiomics_batch_extract` | image FileSet + mask FileSet + manifest File -> five Files | Ordered cohort extraction with per-unit failure isolation |
| `radiomics_dicom_metadata` | DICOM File/Any -> metadata, report Files | Extracts standard clinical/acquisition/reconstruction/dose tags plus custom keywords |
| `radiomics_phi_scrub` | DICOM File/Any -> scrubbed ZIP, per-file report, summary File | Recursively removes PHI and optionally pseudonymizes PatientID/PatientName |
| `radiomics_voi_dice_hausdorff` | reference mask + comparison mask -> metrics, report Files | Computes Dice, Jaccard, volume similarity, Hausdorff distances, and surface Dice |
| `radiomics_image_qc` | image + ROI mask + background mask -> metrics, report Files | Computes SNR, CNR, ROI uniformity, zero-variance slices, and interior ROI gaps |
| `radiomics_rtstruct_geometry` | RTSTRUCT + reference image -> geometry, report Files | Profiles contours, point counts, slice spacing, finite points, and bounds consistency |
| `radiomics_ivh_extract` | image + mask -> IVH, report Files | Computes intensity at configured cumulative volume fractions |
| `radiomics_shape_topology` | mask -> topology, slice profile, report Files | Computes connected components, Euler residual, convex hull, principal axes, and per-slice geometry |
| `radiomics_register` | fixed image + moving image -> registered image, transform, report Files | Runs SimpleITK rigid or affine registration |
| `radiomics_delta_features` | wide feature File -> delta, report Files | Pairs baseline/followup rows and computes absolute, relative, and percent changes |
| `radiomics_qc` | wide features -> sample QC, feature QC, filtered features | Deterministic missing/non-finite/constant checks |
| `radiomics_feature_set_assemble` | filtered features -> feature set, feature metadata | Adds stable feature-set identity and checks duplicate IDs |

The extraction unit is `extraction_id`, not `patient_id`. One patient may have
multiple examinations, modalities, lesions, ROIs, or timepoints.

Batch extraction manifests must contain `extraction_id`, `patient_id`,
`image_id`, `roi_id`, `modality`, and `preset_id`; all other manifest columns
are preserved in provenance. A per-row integer `mask_label` overrides the
node-level default. This matters for phantom variants that encode the ROI as
label 255 rather than label 1.

## Extraction outputs

`pyradiomics_extract` and `pyradiomics_batch_extract` emit:

```text
features_wide.parquet
features_long.parquet
feature_metadata.parquet
diagnostics.parquet
provenance.json
```

The wide table contains the manifest identity columns, normalized SQL-friendly
feature IDs, `status`, and `error_code`. Feature columns and `feature_id` values
are lowercase (for example, `original_shape_elongation`, not the CamelCase name
used in PyRadiomics documentation). The long table contains one row per feature
value. Metadata maps normalized IDs back to exact PyRadiomics names, feature
family, image type, modality, and preset. Provenance records package versions,
image/mask hashes, and the full extraction settings.

### PyRadiomics feature space

The extraction nodes accept every image type implemented by PyRadiomics 3.1:

```text
Original
LoG
Wavelet
Square
SquareRoot
Logarithm
Exponential
Gradient
LBP2D
LBP3D
```

Accepted feature classes are:

```text
shape
shape2D
firstorder
glcm
glrlm
glszm
gldm
ngtdm
```

`LoG` exposes configurable positive sigma values. `Wavelet` enables all eight
3D wavelet sub-bands. `shape2D` requires `force2d=true`; its ROI must also be
one voxel thick in the selected 2D extraction dimension.

PyRadiomics 3.1 does not implement NGLDM. The spec validator rejects `ngldm`
rather than silently omitting it. `LBP3D` is enabled by the pinned `trimesh`
dependency and a small SciPy 1.17 spherical-harmonics compatibility shim.

The conservative default remains `Original` plus the seven standard 3D feature
classes. Explicitly list all ten image types when a wide `pyradiomics_all_v1`
feature space is required; this can increase feature count by more than an
order of magnitude.

## Recommended pipelines

For already paired NIfTI/MHA files:

```text
cohort table
  -> radiomics_manifest_build
  -> radiomics_stage_file_set(image_uri)
  -> radiomics_stage_file_set(mask_uri)
  -> pyradiomics_batch_extract
  -> file_to_dataframe(features_wide.parquet)
  -> radiomics_qc
  -> radiomics_feature_set_assemble
```

For DICOM image series and RTSTRUCT:

```text
image File/Any -> radiomics_image_ingest -> image.mha
reference image Any + mask/RTSTRUCT Any -> radiomics_mask_ingest -> mask.mha
image.mha + mask.mha -> radiomics_pair_validate
image.mha + mask.mha -> radiomics_preprocess
image.mha + mask.mha -> pyradiomics_extract
```

The image/mask ingestion inputs accept either one File or a FileSet. A FileSet
is required to pass a multi-file DICOM series into the runtime.

For governance and longitudinal workflows:

```text
DICOM FileSet
  -> radiomics_dicom_metadata
  -> file_to_dataframe

DICOM FileSet
  -> radiomics_phi_scrub
  -> immutable scrubbed DICOM ZIP

mask.mha + comparison_mask.mha
  -> radiomics_voi_dice_hausdorff

image.mha + roi_mask.mha + background_mask.mha
  -> radiomics_image_qc

RTSTRUCT + image File/Any
  -> radiomics_rtstruct_geometry

feature_set.parquet
  -> radiomics_delta_features
```

## Image and provenance

All container nodes use:

```text
$ACR_ENDPOINT/autonomics/pyradiomics@sha256:31994246efb2426aa82db1c8c31a451aa040800489429c8a4637dcb5a03d0b74
```

The image is built from the official PyRadiomics `v3.1.0` source release and
contains no user data or reference panels. It uses an isolated network and a
read-only root filesystem. See `containers/pyradiomics/README.md` for the
pinned dependency stack.

## Validation baseline

The implementation was checked against the IBSI validation checkout at commit
`6da96021bc91faf4c0cb7fd7fa56a4225d2064a8`:

- all 153 NIfTI CT/MR/PET image-mask pairs passed geometry validation;
- `STS_001` CT shape/first-order extraction produced 32 features;
- all 51-subject CT/MR/PET batch extraction produced 153 valid rows and
  4,896 shape/first-order long-table rows;
- six IBSI-2 digital phantoms extracted successfully after staging NIfTI files
  under generic `.gz` filenames and keeping the manifest in input slot 2;
- DICOM series ingestion matched the supplied NIfTI geometry;
- single-file DICOM ingestion produced a readable single-slice MHA image;
- RTSTRUCT rasterization matched the supplied CT mask exactly (Dice = 1.0);
- resampling and resegmentation produced a valid image/mask pair.
- `STS_001` PET extraction with all ten image types and first-order features
  produced 342 valid long-table rows and ten distinct metadata image types;
- DICOM metadata, PHI scrubbing, VOI similarity, image QC, RTSTRUCT geometry,
  IVH, topology, registration, and delta-feature commands all passed the
  Phase-A smoke script.
- local and VFS DICOM glob expansion returned deterministic, sorted FileSets.

The IBSI repository does not include its numerical reference tables. Golden
feature files generated with the pinned image should be added before claiming
IBSI numerical compliance for every feature.

## Scope limits

- DICOM SEG is not yet implemented; DICOM RTSTRUCT is supported.
- PHI scrubbing removes DICOM identifying elements and pseudonymizes selected
  identifiers; it is not a legal compliance certification and does not perform
  pixel burn-in.
- Image QC requires an explicit background mask. It does not infer a modality-
  specific noise region automatically.
- Registration currently supports rigid and affine intensity registration; it
  does not provide deformable registration.
- Topology Euler/hole output is a topology residual, not an IBSI feature.
- The default preset is named `pyradiomics_original_v1`, not `ibsi_v1`, because
  PyRadiomics feature implementations are not automatically IBSI-compliant.
- Training-set normalization, batch-effect correction, feature selection, and
  predictive modeling remain Stage-B operations.
- The published ACR image must be available to the k3s/containerd runtime; the
  digest-pinned reference is intentional.
