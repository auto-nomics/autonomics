//! Thin DAG wrappers for the pinned PyRadiomics Stage-A OCI image.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use container_runtime::{ContainerRuntime, PanelCache, PullPolicy};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;
use dag_core::{DataBundle, dag::DagError, dag::graph::PortOutputs};

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::acr_image;

pub const PYRADIOMICS_IMAGE_REPOSITORY: &str = "pyradiomics";
pub const PYRADIOMICS_IMAGE_DIGEST: &str =
    "sha256:4ef0fc2abbd5a85812b04bceef70b03f207494dbaa53a06c1a3eb9e24b3e7392";
pub const RADIOMICS_IMAGE_INGEST_KIND: &str = "radiomics_image_ingest";
pub const RADIOMICS_MASK_INGEST_KIND: &str = "radiomics_mask_ingest";
pub const RADIOMICS_PAIR_VALIDATE_KIND: &str = "radiomics_pair_validate";
pub const RADIOMICS_PREPROCESS_KIND: &str = "radiomics_preprocess";
pub const PYRADIOMICS_EXTRACT_KIND: &str = "pyradiomics_extract";
pub const PYRADIOMICS_BATCH_EXTRACT_KIND: &str = "pyradiomics_batch_extract";
pub const RADIOMICS_DICOM_METADATA_KIND: &str = "radiomics_dicom_metadata";
pub const RADIOMICS_PHI_SCRUB_KIND: &str = "radiomics_phi_scrub";
pub const RADIOMICS_VOI_SIMILARITY_KIND: &str = "radiomics_voi_dice_hausdorff";
pub const RADIOMICS_IMAGE_QC_KIND: &str = "radiomics_image_qc";
pub const RADIOMICS_RTSTRUCT_GEOMETRY_KIND: &str = "radiomics_rtstruct_geometry";
pub const RADIOMICS_IVH_KIND: &str = "radiomics_ivh_extract";
pub const RADIOMICS_SHAPE_TOPOLOGY_KIND: &str = "radiomics_shape_topology";
pub const RADIOMICS_REGISTER_KIND: &str = "radiomics_register";
pub const RADIOMICS_DELTA_FEATURES_KIND: &str = "radiomics_delta_features";

const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const PYRADIOMICS_IMAGE_TYPES: &[&str] = &[
    "Original",
    "LoG",
    "Wavelet",
    "Square",
    "SquareRoot",
    "Logarithm",
    "Exponential",
    "Gradient",
    "LBP2D",
    "LBP3D",
];
const PYRADIOMICS_FEATURE_CLASSES: &[&str] = &[
    "shape",
    "shape2D",
    "firstorder",
    "glcm",
    "glrlm",
    "glszm",
    "gldm",
    "ngtdm",
];

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RadiomicsDicomSortMode {
    Lexical,
    InstanceNumber,
    Position,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RadiomicsDicomDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
pub struct RadiomicsDicomOrderSettings {
    #[serde(default = "default_dicom_sort_mode")]
    pub z_sort: RadiomicsDicomSortMode,
    #[serde(default = "default_dicom_direction")]
    pub z_direction: RadiomicsDicomDirection,
}

fn default_dicom_sort_mode() -> RadiomicsDicomSortMode {
    RadiomicsDicomSortMode::Position
}

fn default_dicom_direction() -> RadiomicsDicomDirection {
    RadiomicsDicomDirection::Ascending
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsArtifactSpec {
    #[serde(flatten)]
    pub dicom_order: RadiomicsDicomOrderSettings,
    #[serde(default = "default_image_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_image_prefix() -> String {
    "/artifacts/radiomics_image_ingest".into()
}
fn default_mask_prefix() -> String {
    "/artifacts/radiomics_mask_ingest".into()
}
fn default_validate_prefix() -> String {
    "/artifacts/radiomics_pair_validate".into()
}
fn default_preprocess_prefix() -> String {
    "/artifacts/radiomics_preprocess".into()
}
fn default_extract_prefix() -> String {
    "/artifacts/pyradiomics_extract".into()
}
fn default_batch_prefix() -> String {
    "/artifacts/pyradiomics_batch_extract".into()
}
fn default_dicom_metadata_prefix() -> String {
    "/artifacts/radiomics_dicom_metadata".into()
}
fn default_phi_scrub_prefix() -> String {
    "/artifacts/radiomics_phi_scrub".into()
}
fn default_voi_prefix() -> String {
    "/artifacts/radiomics_voi_dice_hausdorff".into()
}
fn default_image_qc_prefix() -> String {
    "/artifacts/radiomics_image_qc".into()
}
fn default_rt_geometry_prefix() -> String {
    "/artifacts/radiomics_rtstruct_geometry".into()
}
fn default_ivh_prefix() -> String {
    "/artifacts/radiomics_ivh_extract".into()
}
fn default_topology_prefix() -> String {
    "/artifacts/radiomics_shape_topology".into()
}
fn default_register_prefix() -> String {
    "/artifacts/radiomics_register".into()
}
fn default_delta_prefix() -> String {
    "/artifacts/radiomics_delta_features".into()
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn validate_artifact_spec(spec: &RadiomicsArtifactSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsMaskIngestSpec {
    /// Exact DICOM RTSTRUCT ROIName. The first ROI is used when omitted.
    #[serde(default)]
    pub roi_name: Option<String>,
    #[serde(flatten)]
    pub dicom_order: RadiomicsDicomOrderSettings,
    #[serde(default = "default_mask_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsPairValidateSpec {
    pub extraction_id: String,
    #[serde(default = "default_mask_label")]
    pub mask_label: i32,
    #[serde(default = "default_minimum_voxels")]
    pub minimum_mask_voxels: i64,
    #[serde(default = "default_geometry_tolerance")]
    pub geometry_tolerance_mm: f64,
    #[serde(default = "default_validate_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_mask_label() -> i32 {
    1
}
fn default_minimum_voxels() -> i64 {
    1
}
fn default_geometry_tolerance() -> f64 {
    0.01
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsPreprocessSpec {
    /// Desired isotropic or anisotropic spacing in millimeters.
    #[serde(default)]
    pub resampled_spacing: Option<Vec<f64>>,
    /// SimpleITK interpolator name, for example sitkBSpline.
    #[serde(default = "default_interpolator")]
    pub interpolator: String,
    /// Inclusive intensity resegmentation range applied to the mask.
    #[serde(default)]
    pub resegment_range: Option<(f64, f64)>,
    #[serde(default)]
    pub normalize: bool,
    #[serde(default = "default_preprocess_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_interpolator() -> String {
    "sitkBSpline".into()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsExtractionSettings {
    #[serde(default = "default_mask_label")]
    pub mask_label: i32,
    #[serde(default = "default_bin_width")]
    pub bin_width: f64,
    #[serde(default)]
    pub resampled_spacing: Option<Vec<f64>>,
    #[serde(default)]
    pub force2d: bool,
    #[serde(default = "default_force2d_dimension")]
    pub force2d_dimension: u8,
    #[serde(default = "default_image_types")]
    pub image_types: Vec<String>,
    #[serde(default = "default_feature_classes")]
    pub feature_classes: Vec<String>,
    /// LoG kernel sigmas in millimeters. Ignored unless `LoG` is enabled.
    #[serde(default = "default_log_sigmas")]
    pub log_sigmas: Vec<f64>,
}

fn default_bin_width() -> f64 {
    25.0
}
fn default_force2d_dimension() -> u8 {
    0
}
fn default_image_types() -> Vec<String> {
    vec!["Original".into()]
}
fn default_feature_classes() -> Vec<String> {
    vec![
        "shape".into(),
        "firstorder".into(),
        "glcm".into(),
        "glrlm".into(),
        "glszm".into(),
        "gldm".into(),
        "ngtdm".into(),
    ]
}
fn default_log_sigmas() -> Vec<f64> {
    vec![2.0, 3.0, 4.0, 5.0]
}

impl RadiomicsExtractionSettings {
    fn validate(&self) -> Result<(), String> {
        if self.mask_label <= 0 {
            return Err("mask_label must be positive".into());
        }
        if !(self.bin_width.is_finite() && self.bin_width > 0.0) {
            return Err("bin_width must be finite and positive".into());
        }
        if let Some(spacing) = &self.resampled_spacing
            && (spacing.len() != 3
                || spacing
                    .iter()
                    .any(|value| !(value.is_finite() && *value > 0.0)))
        {
            return Err("resampled_spacing must contain three positive numbers".into());
        }
        if self.image_types.is_empty() || self.feature_classes.is_empty() {
            return Err("image_types and feature_classes cannot be empty".into());
        }
        let unknown_image_types = self
            .image_types
            .iter()
            .filter(|value| !PYRADIOMICS_IMAGE_TYPES.contains(&value.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown_image_types.is_empty() {
            return Err(format!(
                "unsupported PyRadiomics image type(s): {}. Supported types: {}",
                unknown_image_types.join(", "),
                PYRADIOMICS_IMAGE_TYPES.join(", ")
            ));
        }
        let unknown_feature_classes = self
            .feature_classes
            .iter()
            .filter(|value| !PYRADIOMICS_FEATURE_CLASSES.contains(&value.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown_feature_classes.is_empty() {
            return Err(format!(
                "unsupported PyRadiomics feature class(es): {}. PyRadiomics 3.1 supports: {} (NGLDM is not available)",
                unknown_feature_classes.join(", "),
                PYRADIOMICS_FEATURE_CLASSES.join(", ")
            ));
        }
        if self.feature_classes.iter().any(|value| value == "shape2D") && !self.force2d {
            return Err("shape2D requires force2d=true".into());
        }
        if self.log_sigmas.is_empty()
            || self
                .log_sigmas
                .iter()
                .any(|value| !(value.is_finite() && *value > 0.0))
            || self.log_sigmas.len() > 10
        {
            return Err("log_sigmas must contain 1-10 positive finite values".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PyradiomicsExtractSpec {
    pub extraction_id: String,
    pub patient_id: String,
    pub image_id: String,
    pub roi_id: String,
    pub roi_name: Option<String>,
    pub modality: String,
    pub preset_id: String,
    #[serde(flatten)]
    pub settings: RadiomicsExtractionSettings,
    #[serde(default = "default_extract_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PyradiomicsBatchExtractSpec {
    /// Skip manifest rows with status != valid.
    #[serde(default = "default_true")]
    pub valid_only: bool,
    #[serde(flatten)]
    pub settings: RadiomicsExtractionSettings,
    #[serde(default = "default_batch_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsDicomMetadataSpec {
    /// Additional DICOM keywords or `(group,element)` tags.
    #[serde(default)]
    pub extra_tags: Vec<String>,
    #[serde(default = "default_dicom_metadata_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsPhiScrubSpec {
    /// Retain a pseudonymized PatientID instead of deleting it.
    #[serde(default)]
    pub keep_patient_id: bool,
    /// Optional replacement value for PatientID/PatientName.
    #[serde(default)]
    pub pseudonym: Option<String>,
    #[serde(default = "default_phi_scrub_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsVoiSimilaritySpec {
    pub comparison_id: String,
    #[serde(default = "default_mask_label")]
    pub mask_label: i32,
    #[serde(default = "default_surface_tolerance")]
    pub surface_tolerance_mm: f64,
    #[serde(default = "default_voi_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsImageQcSpec {
    pub extraction_id: String,
    #[serde(default = "default_mask_label")]
    pub mask_label: i32,
    #[serde(default = "default_image_qc_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsRtstructGeometrySpec {
    #[serde(default)]
    pub roi_name: Option<String>,
    #[serde(default = "default_rt_geometry_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsIvhSpec {
    pub extraction_id: String,
    #[serde(default = "default_mask_label")]
    pub mask_label: i32,
    #[serde(default = "default_volume_fractions")]
    pub volume_fractions: Vec<f64>,
    #[serde(default = "default_ivh_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsShapeTopologySpec {
    pub extraction_id: String,
    #[serde(default = "default_mask_label")]
    pub mask_label: i32,
    #[serde(default = "default_topology_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsRegisterSpec {
    /// `rigid` or `affine`.
    #[serde(default = "default_transform_type")]
    pub transform_type: String,
    #[serde(default = "default_iterations")]
    pub iterations: u32,
    #[serde(default = "default_learning_rate")]
    pub learning_rate: f64,
    #[serde(default = "default_sampling_percent")]
    pub sampling_percent: f64,
    #[serde(default = "default_register_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsDeltaFeaturesSpec {
    #[serde(default = "default_delta_id_column")]
    pub id_column: String,
    #[serde(default = "default_delta_time_column")]
    pub timepoint_column: String,
    #[serde(default = "default_baseline")]
    pub baseline: String,
    #[serde(default = "default_followup")]
    pub followup: String,
    #[serde(default = "default_delta_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}
fn default_surface_tolerance() -> f64 {
    1.0
}
fn default_volume_fractions() -> Vec<f64> {
    (1..=19).map(|value| value as f64 / 20.0).collect()
}
fn default_transform_type() -> String {
    "rigid".into()
}
fn default_iterations() -> u32 {
    100
}
fn default_learning_rate() -> f64 {
    1.0
}
fn default_sampling_percent() -> f64 {
    0.15
}
fn default_delta_id_column() -> String {
    "patient_id".into()
}
fn default_delta_time_column() -> String {
    "timepoint".into()
}
fn default_baseline() -> String {
    "baseline".into()
}
fn default_followup() -> String {
    "followup".into()
}

pub struct RadiomicsContainerNode {
    kind: &'static str,
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for RadiomicsContainerNode {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait]
impl DagNode for RadiomicsContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        self.kind
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.kind == RADIOMICS_MASK_INGEST_KIND {
            // The runner contract is input 0=image and input 1=mask/RTSTRUCT,
            // independent of scheduler edge insertion order.
            let mut ordered_inputs = inputs.to_vec();
            ordered_inputs.sort_by_key(|input| input.port);
            self.inner.execute(ctx, &ordered_inputs, reporter).await
        } else {
            self.inner.execute(ctx, inputs, reporter).await
        }
    }
}

pub struct RadiomicsContainerNodeFactory {
    kind: &'static str,
    runtime: Arc<dyn ContainerRuntime>,
    panel_cache: Arc<PanelCache>,
}

impl RadiomicsContainerNodeFactory {
    fn new(
        kind: &'static str,
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self {
            kind,
            runtime,
            panel_cache,
        }
    }

    fn build_container(
        &self,
        spec: ContainerCommandSpec,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node = ContainerCommandNode::new(spec, self.runtime.clone(), self.panel_cache.clone())
            .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(node))
    }
}

fn base_spec(command: &str, prefix: &str, timeout: u64) -> Result<ContainerCommandSpec, String> {
    Ok(ContainerCommandSpec {
        image: acr_image(PYRADIOMICS_IMAGE_REPOSITORY, PYRADIOMICS_IMAGE_DIGEST)?,
        command: vec![
            "python".into(),
            "/opt/radiomics/radiomics_runner.py".into(),
            command.into(),
        ],
        script: None,
        files: Default::default(),
        env: Default::default(),
        outputs: Vec::new(),
        workdir: None,
        artifact_prefix: prefix.into(),
        timeout_secs: timeout,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: None,
        memory: None,
        pids_limit: None,
        shm_size: None,
        user: None,
    })
}

pub fn image_ingest_container_spec(
    spec: &RadiomicsArtifactSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_artifact_spec(spec)?;
    let mut container = base_spec("ingest-image", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_IMAGE_SETTINGS".into(),
        serde_json::to_string(&spec.dicom_order).map_err(|error| error.to_string())?,
    );
    container.outputs = vec![
        output("image.mha", "mha"),
        output("image_meta.json", "json"),
    ];
    Ok(container)
}

pub fn mask_ingest_container_spec(
    spec: &RadiomicsMaskIngestSpec,
) -> Result<ContainerCommandSpec, String> {
    if !spec.artifact_prefix.starts_with('/') || spec.timeout_secs == 0 {
        return Err("invalid mask-ingest artifact prefix or timeout".into());
    }
    let mut container = base_spec("ingest-mask", &spec.artifact_prefix, spec.timeout_secs)?;
    let mut mask_settings =
        serde_json::to_value(&spec.dicom_order).map_err(|error| error.to_string())?;
    if let Some(settings) = mask_settings.as_object_mut() {
        settings.insert("roi_name".into(), serde_json::json!(spec.roi_name));
    }
    container
        .env
        .insert("RADIOMICS_MASK_SETTINGS".into(), mask_settings.to_string());
    container.outputs = vec![output("mask.mha", "mha"), output("roi_meta.json", "json")];
    Ok(container)
}

pub fn pair_validate_container_spec(
    spec: &RadiomicsPairValidateSpec,
) -> Result<ContainerCommandSpec, String> {
    if spec.extraction_id.trim().is_empty() {
        return Err("extraction_id cannot be empty".into());
    }
    if spec.mask_label <= 0 || spec.minimum_mask_voxels < 0 {
        return Err("mask_label must be positive and minimum_mask_voxels nonnegative".into());
    }
    if !(spec.geometry_tolerance_mm.is_finite() && spec.geometry_tolerance_mm >= 0.0) {
        return Err("geometry_tolerance_mm must be finite and nonnegative".into());
    }
    if !spec.artifact_prefix.starts_with('/') || spec.timeout_secs == 0 {
        return Err("invalid pair-validation artifact prefix or timeout".into());
    }
    let mut container = base_spec("validate-pair", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_VALIDATE_SETTINGS".into(),
        serde_json::json!({
            "extraction_id": spec.extraction_id,
            "mask_label": spec.mask_label,
            "minimum_mask_voxels": spec.minimum_mask_voxels,
            "geometry_tolerance_mm": spec.geometry_tolerance_mm,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("pair_validation.parquet", "parquet"),
        output("geometry.json", "json"),
    ];
    Ok(container)
}

pub fn preprocess_container_spec(
    spec: &RadiomicsPreprocessSpec,
) -> Result<ContainerCommandSpec, String> {
    if !spec.artifact_prefix.starts_with('/') || spec.timeout_secs == 0 {
        return Err("invalid preprocess artifact prefix or timeout".into());
    }
    if let Some(spacing) = &spec.resampled_spacing
        && (spacing.len() != 3
            || spacing
                .iter()
                .any(|value| !(value.is_finite() && *value > 0.0)))
    {
        return Err("resampled_spacing must contain three positive numbers".into());
    }
    if let Some((low, high)) = &spec.resegment_range
        && (!low.is_finite() || !high.is_finite() || low > high)
    {
        return Err("resegment_range must be finite and ordered".into());
    }
    let mut container = base_spec("preprocess", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_PREPROCESS_SETTINGS".into(),
        serde_json::json!({
            "resampled_spacing": spec.resampled_spacing,
            "interpolator": spec.interpolator,
            "resegment_range": spec.resegment_range,
            "normalize": spec.normalize,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("image.mha", "mha"),
        output("mask.mha", "mha"),
        output("preprocess_meta.json", "json"),
    ];
    Ok(container)
}

fn extraction_settings_json(settings: &RadiomicsExtractionSettings) -> serde_json::Value {
    serde_json::json!({
        "mask_label": settings.mask_label,
        "bin_width": settings.bin_width,
        "resampled_spacing": settings.resampled_spacing,
        "force2d": settings.force2d,
        "force2d_dimension": settings.force2d_dimension,
        "image_types": settings.image_types,
        "feature_classes": settings.feature_classes,
        "log_sigmas": settings.log_sigmas,
    })
}

pub fn extract_container_spec(
    spec: &PyradiomicsExtractSpec,
) -> Result<ContainerCommandSpec, String> {
    if [
        &spec.extraction_id,
        &spec.patient_id,
        &spec.image_id,
        &spec.roi_id,
        &spec.modality,
        &spec.preset_id,
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Err("extraction identity fields cannot be empty".into());
    }
    spec.settings.validate()?;
    if !spec.artifact_prefix.starts_with('/') || spec.timeout_secs == 0 {
        return Err("invalid extraction artifact prefix or timeout".into());
    }
    let mut container = base_spec("extract-single", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_EXTRACT_SETTINGS".into(),
        extraction_settings_json(&spec.settings).to_string(),
    );
    container.env.insert(
        "RADIOMICS_EXTRACTION".into(),
        serde_json::json!({
            "extraction_id": spec.extraction_id,
            "patient_id": spec.patient_id,
            "image_id": spec.image_id,
            "roi_id": spec.roi_id,
            "roi_name": spec.roi_name,
            "modality": spec.modality,
            "preset_id": spec.preset_id,
        })
        .to_string(),
    );
    container.outputs = extraction_outputs();
    Ok(container)
}

pub fn batch_extract_container_spec(
    spec: &PyradiomicsBatchExtractSpec,
) -> Result<ContainerCommandSpec, String> {
    spec.settings.validate()?;
    if !spec.artifact_prefix.starts_with('/') || spec.timeout_secs == 0 {
        return Err("invalid extraction artifact prefix or timeout".into());
    }
    let mut container = base_spec("extract-batch", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_EXTRACT_SETTINGS".into(),
        extraction_settings_json(&spec.settings).to_string(),
    );
    container
        .env
        .insert("RADIOMICS_VALID_ONLY".into(), spec.valid_only.to_string());
    container.outputs = extraction_outputs();
    Ok(container)
}

fn validate_prefix_timeout(prefix: &str, timeout_secs: u64) -> Result<(), String> {
    if !prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

pub fn dicom_metadata_container_spec(
    spec: &RadiomicsDicomMetadataSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.extra_tags.iter().any(|tag| tag.trim().is_empty()) {
        return Err("extra_tags cannot contain empty values".into());
    }
    let mut container = base_spec("dicom-metadata", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_DICOM_METADATA_SETTINGS".into(),
        serde_json::json!({"extra_tags": spec.extra_tags}).to_string(),
    );
    container.outputs = vec![
        output("dicom_metadata.parquet", "parquet"),
        output("dicom_metadata_report.json", "json"),
    ];
    Ok(container)
}

pub fn phi_scrub_container_spec(
    spec: &RadiomicsPhiScrubSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if let Some(pseudonym) = &spec.pseudonym
        && pseudonym.trim().is_empty()
    {
        return Err("pseudonym cannot be empty when provided".into());
    }
    if spec.pseudonym.is_some() && !spec.keep_patient_id {
        return Err("pseudonym requires keep_patient_id=true".into());
    }
    let mut container = base_spec("phi-scrub", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_PHI_SCRUB_SETTINGS".into(),
        serde_json::json!({
            "keep_patient_id": spec.keep_patient_id,
            "pseudonym": spec.pseudonym,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("scrubbed_dicom.zip", "dicom_zip"),
        output("phi_scrub_report.parquet", "parquet"),
        output("phi_scrub_report.json", "json"),
    ];
    Ok(container)
}

pub fn voi_similarity_container_spec(
    spec: &RadiomicsVoiSimilaritySpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.comparison_id.trim().is_empty() || spec.mask_label <= 0 {
        return Err("comparison_id cannot be empty and mask_label must be positive".into());
    }
    if !(spec.surface_tolerance_mm.is_finite() && spec.surface_tolerance_mm >= 0.0) {
        return Err("surface_tolerance_mm must be finite and nonnegative".into());
    }
    let mut container = base_spec("voi-similarity", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_VOI_SETTINGS".into(),
        serde_json::json!({
            "comparison_id": spec.comparison_id,
            "mask_label": spec.mask_label,
            "surface_tolerance_mm": spec.surface_tolerance_mm,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("voi_similarity.parquet", "parquet"),
        output("voi_similarity.json", "json"),
    ];
    Ok(container)
}

pub fn image_qc_container_spec(
    spec: &RadiomicsImageQcSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.extraction_id.trim().is_empty() || spec.mask_label <= 0 {
        return Err("extraction_id cannot be empty and mask_label must be positive".into());
    }
    let mut container = base_spec("image-qc", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_IMAGE_QC_SETTINGS".into(),
        serde_json::json!({
            "extraction_id": spec.extraction_id,
            "mask_label": spec.mask_label,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("image_qc.parquet", "parquet"),
        output("image_qc.json", "json"),
    ];
    Ok(container)
}

pub fn rtstruct_geometry_container_spec(
    spec: &RadiomicsRtstructGeometrySpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if let Some(roi_name) = &spec.roi_name
        && roi_name.trim().is_empty()
    {
        return Err("roi_name cannot be empty when provided".into());
    }
    let mut container = base_spec(
        "rtstruct-geometry",
        &spec.artifact_prefix,
        spec.timeout_secs,
    )?;
    container.env.insert(
        "RADIOMICS_RTSTRUCT_GEOMETRY_SETTINGS".into(),
        serde_json::json!({"roi_name": spec.roi_name}).to_string(),
    );
    container.outputs = vec![
        output("rtstruct_geometry.parquet", "parquet"),
        output("rtstruct_geometry.json", "json"),
    ];
    Ok(container)
}

pub fn ivh_container_spec(spec: &RadiomicsIvhSpec) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.extraction_id.trim().is_empty() || spec.mask_label <= 0 {
        return Err("extraction_id cannot be empty and mask_label must be positive".into());
    }
    if spec.volume_fractions.is_empty()
        || spec
            .volume_fractions
            .iter()
            .any(|value| !(value.is_finite() && *value > 0.0 && *value <= 1.0))
    {
        return Err("volume_fractions must contain values in (0, 1]".into());
    }
    let mut container = base_spec("ivh-extract", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_IVH_SETTINGS".into(),
        serde_json::json!({
            "extraction_id": spec.extraction_id,
            "mask_label": spec.mask_label,
            "volume_fractions": spec.volume_fractions,
        })
        .to_string(),
    );
    container.outputs = vec![output("ivh.parquet", "parquet"), output("ivh.json", "json")];
    Ok(container)
}

pub fn shape_topology_container_spec(
    spec: &RadiomicsShapeTopologySpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.extraction_id.trim().is_empty() || spec.mask_label <= 0 {
        return Err("extraction_id cannot be empty and mask_label must be positive".into());
    }
    let mut container = base_spec("shape-topology", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_SHAPE_TOPOLOGY_SETTINGS".into(),
        serde_json::json!({
            "extraction_id": spec.extraction_id,
            "mask_label": spec.mask_label,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("shape_topology.parquet", "parquet"),
        output("slice_profile.parquet", "parquet"),
        output("shape_topology.json", "json"),
    ];
    Ok(container)
}

pub fn register_container_spec(
    spec: &RadiomicsRegisterSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if !matches!(spec.transform_type.as_str(), "rigid" | "affine") {
        return Err("transform_type must be `rigid` or `affine`".into());
    }
    if spec.iterations == 0 {
        return Err("iterations must be greater than zero".into());
    }
    if !(spec.learning_rate.is_finite() && spec.learning_rate > 0.0) {
        return Err("learning_rate must be finite and positive".into());
    }
    if !(spec.sampling_percent.is_finite() && (0.0..=1.0).contains(&spec.sampling_percent)) {
        return Err("sampling_percent must be between 0 and 1".into());
    }
    let mut container = base_spec("register", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_REGISTER_SETTINGS".into(),
        serde_json::json!({
            "transform_type": spec.transform_type,
            "iterations": spec.iterations,
            "learning_rate": spec.learning_rate,
            "sampling_percent": spec.sampling_percent,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("registered.mha", "mha"),
        output("transform.tfm", "simpleitk_transform"),
        output("registration.json", "json"),
    ];
    Ok(container)
}

pub fn delta_features_container_spec(
    spec: &RadiomicsDeltaFeaturesSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.id_column.trim().is_empty()
        || spec.timepoint_column.trim().is_empty()
        || spec.baseline.trim().is_empty()
        || spec.followup.trim().is_empty()
    {
        return Err("delta column and timepoint values cannot be empty".into());
    }
    if spec.baseline == spec.followup {
        return Err("baseline and followup must differ".into());
    }
    let mut container = base_spec("delta-features", &spec.artifact_prefix, spec.timeout_secs)?;
    container.env.insert(
        "RADIOMICS_DELTA_SETTINGS".into(),
        serde_json::json!({
            "id_column": spec.id_column,
            "timepoint_column": spec.timepoint_column,
            "baseline": spec.baseline,
            "followup": spec.followup,
        })
        .to_string(),
    );
    container.outputs = vec![
        output("delta_features.parquet", "parquet"),
        output("delta_features_report.json", "json"),
    ];
    Ok(container)
}

fn output(path: &str, format: &str) -> ContainerCommandOutputSpec {
    ContainerCommandOutputSpec {
        path: path.into(),
        format: Some(format.into()),
    }
}

fn extraction_outputs() -> Vec<ContainerCommandOutputSpec> {
    vec![
        output("features_wide.parquet", "parquet"),
        output("features_long.parquet", "parquet"),
        output("feature_metadata.parquet", "parquet"),
        output("diagnostics.parquet", "parquet"),
        output("provenance.json", "json"),
    ]
}

fn image_ingest_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::Any)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn mask_ingest_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::Any, "reference_image")
        .add_input_port_of_type_with_label(None, PortType::Any, "mask_or_rtstruct")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn two_image_file_inputs_and_two_files() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn preprocess_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn extract_ports() -> NodePorts {
    let ports = NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File);
    extraction_outputs().into_iter().fold(ports, |ports, _| {
        ports.add_output_port_of_type(None, PortType::File)
    })
}

fn batch_extract_ports() -> NodePorts {
    let ports = NodePorts::new()
        .add_input_port_of_type(None, PortType::FileSet)
        .add_input_port_of_type(None, PortType::FileSet)
        .add_input_port_of_type(None, PortType::File);
    extraction_outputs().into_iter().fold(ports, |ports, _| {
        ports.add_output_port_of_type(None, PortType::File)
    })
}

fn dicom_metadata_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::Any)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn phi_scrub_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::Any)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn voi_similarity_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn image_qc_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn rtstruct_geometry_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::Any)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn ivh_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn shape_topology_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn register_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn delta_features_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn deserialize_spec<T: for<'de> Deserialize<'de>>(
    spec: serde_json::Value,
) -> dag_core::registry::error::Result<T> {
    serde_json::from_value(spec).map_err(dag_core::registry::error::Error::from)
}

impl NodeFactory for RadiomicsContainerNodeFactory {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn desc(&self) -> &'static str {
        match self.kind {
            RADIOMICS_IMAGE_INGEST_KIND => "Normalizes NIfTI, MHA, or a DICOM series to MHA.",
            RADIOMICS_MASK_INGEST_KIND => {
                "Normalizes a voxel mask or converts RTSTRUCT against a reference image."
            }
            RADIOMICS_PAIR_VALIDATE_KIND => "Validates image/mask geometry and ROI voxel count.",
            RADIOMICS_PREPROCESS_KIND => "Applies deterministic radiomics preprocessing.",
            PYRADIOMICS_EXTRACT_KIND => "Extracts PyRadiomics features for one image/mask pair.",
            PYRADIOMICS_BATCH_EXTRACT_KIND => {
                "Extracts PyRadiomics features from ordered manifest FileSets."
            }
            RADIOMICS_DICOM_METADATA_KIND => {
                "Extracts clinical, acquisition, reconstruction, and dose DICOM metadata."
            }
            RADIOMICS_PHI_SCRUB_KIND => {
                "Removes PHI from DICOM datasets and emits an audit report."
            }
            RADIOMICS_VOI_SIMILARITY_KIND => {
                "Computes Dice, Jaccard, Hausdorff, and surface Dice for two masks."
            }
            RADIOMICS_IMAGE_QC_KIND => {
                "Computes ROI/background SNR, CNR, uniformity, and slice completeness."
            }
            RADIOMICS_RTSTRUCT_GEOMETRY_KIND => {
                "Profiles RTSTRUCT contour geometry against a reference image."
            }
            RADIOMICS_IVH_KIND => "Extracts intensity-volume histogram values for one ROI.",
            RADIOMICS_SHAPE_TOPOLOGY_KIND => {
                "Extracts mask topology, principal axes, and per-slice geometry."
            }
            RADIOMICS_REGISTER_KIND => "Registers a moving image to a fixed image.",
            RADIOMICS_DELTA_FEATURES_KIND => {
                "Computes baseline-to-followup feature changes from a wide feature table."
            }
            _ => "Radiomics container node.",
        }
    }

    fn doc(&self) -> &'static str {
        match self.kind {
            RADIOMICS_IMAGE_INGEST_KIND => {
                "Reads one NIfTI/MHA file or a DICOM-series FileSet, writes a normalized MHA image, and records geometry plus source hashes."
            }
            RADIOMICS_MASK_INGEST_KIND => {
                "Input port 0 is the reference image and input port 1 is the mask. Reads a NIfTI/MHA mask or rasterizes a DICOM RTSTRUCT against the reference. Outputs a normalized MHA mask and ROI metadata."
            }
            RADIOMICS_PAIR_VALIDATE_KIND => {
                "Checks image/mask dimension, spacing, origin, direction, label presence, and minimum ROI voxel count. Invalid pairs are reported in the validation table rather than silently dropped."
            }
            RADIOMICS_PREPROCESS_KIND => {
                "Applies explicit deterministic resampling, intensity resegmentation, and within-image ROI normalization. No cohort-fitted transform is estimated."
            }
            PYRADIOMICS_EXTRACT_KIND => {
                "Runs official PyRadiomics 3.1.0 for one extraction unit and emits wide features, long features, metadata, diagnostics, and provenance. Feature columns use normalized lowercase IDs, for example original_shape_elongation; metadata retains exact PyRadiomics CamelCase names."
            }
            PYRADIOMICS_BATCH_EXTRACT_KIND => {
                "Runs official PyRadiomics 3.1.0 over ordered image and mask FileSets plus a CSV/Parquet manifest. One failed extraction does not discard the cohort. Feature columns use normalized lowercase IDs, for example original_shape_elongation; metadata retains exact PyRadiomics CamelCase names."
            }
            RADIOMICS_DICOM_METADATA_KIND => {
                "Reads one DICOM File or a DICOM FileSet, emits one Parquet row per instance, and supports extra keywords or `(group,element)` tags. Acquisition, reconstruction, and common dose tags are included in the standard field set."
            }
            RADIOMICS_PHI_SCRUB_KIND => {
                "Reads one DICOM File or FileSet, recursively removes identifying elements including person-name VR values, optionally replaces PatientID/PatientName with a pseudonym, and returns a DICOM ZIP plus per-file and summary audit reports."
            }
            RADIOMICS_VOI_SIMILARITY_KIND => {
                "Input port 0 is the reference mask and port 1 the comparison mask. Both use the configured positive label and must share geometry. Emits Dice, Jaccard, volume similarity, directed and symmetric Hausdorff distances, and tolerance-based surface Dice."
            }
            RADIOMICS_IMAGE_QC_KIND => {
                "Inputs are image, ROI mask, and background mask. Emits foreground/background intensity statistics, SNR, CNR, ROI coefficient of variation, active/zero-variance slice counts, and interior ROI gaps."
            }
            RADIOMICS_RTSTRUCT_GEOMETRY_KIND => {
                "Input port 0 is an RTSTRUCT File and port 1 a reference image File/FileSet. Emits ROI contour counts, point counts, geometric types, slice spacing, finite-point checks, and reference-bounds checks."
            }
            RADIOMICS_IVH_KIND => {
                "Computes cumulative intensity-volume histogram intensities at configured volume fractions for one image/mask pair."
            }
            RADIOMICS_SHAPE_TOPOLOGY_KIND => {
                "Computes connected components, Euler number and hole estimate, largest-component fraction, physical centroid and principal-axis ratios, and a per-slice area/perimeter/diameter profile."
            }
            RADIOMICS_REGISTER_KIND => {
                "Runs SimpleITK centered rigid or affine registration with Mattes mutual information, emits the resampled moving image, transform, optimizer metric, and provenance."
            }
            RADIOMICS_DELTA_FEATURES_KIND => {
                "Reads a CSV/Parquet wide feature table and pairs configured baseline/followup rows by patient ID. Emits absolute, relative, and percent feature changes plus unresolved-patient diagnostics."
            }
            _ => "Radiomics container node.",
        }
    }

    fn spec_schema(&self) -> schemars::Schema {
        match self.kind {
            RADIOMICS_IMAGE_INGEST_KIND => schema_for!(RadiomicsArtifactSpec),
            RADIOMICS_MASK_INGEST_KIND => schema_for!(RadiomicsMaskIngestSpec),
            RADIOMICS_PAIR_VALIDATE_KIND => schema_for!(RadiomicsPairValidateSpec),
            RADIOMICS_PREPROCESS_KIND => schema_for!(RadiomicsPreprocessSpec),
            PYRADIOMICS_EXTRACT_KIND => schema_for!(PyradiomicsExtractSpec),
            PYRADIOMICS_BATCH_EXTRACT_KIND => schema_for!(PyradiomicsBatchExtractSpec),
            RADIOMICS_DICOM_METADATA_KIND => schema_for!(RadiomicsDicomMetadataSpec),
            RADIOMICS_PHI_SCRUB_KIND => schema_for!(RadiomicsPhiScrubSpec),
            RADIOMICS_VOI_SIMILARITY_KIND => schema_for!(RadiomicsVoiSimilaritySpec),
            RADIOMICS_IMAGE_QC_KIND => schema_for!(RadiomicsImageQcSpec),
            RADIOMICS_RTSTRUCT_GEOMETRY_KIND => schema_for!(RadiomicsRtstructGeometrySpec),
            RADIOMICS_IVH_KIND => schema_for!(RadiomicsIvhSpec),
            RADIOMICS_SHAPE_TOPOLOGY_KIND => schema_for!(RadiomicsShapeTopologySpec),
            RADIOMICS_REGISTER_KIND => schema_for!(RadiomicsRegisterSpec),
            RADIOMICS_DELTA_FEATURES_KIND => schema_for!(RadiomicsDeltaFeaturesSpec),
            _ => schema_for!(RadiomicsArtifactSpec),
        }
    }

    fn ports(&self) -> NodePorts {
        match self.kind {
            RADIOMICS_IMAGE_INGEST_KIND => image_ingest_ports(),
            RADIOMICS_MASK_INGEST_KIND => mask_ingest_ports(),
            RADIOMICS_PAIR_VALIDATE_KIND => two_image_file_inputs_and_two_files(),
            RADIOMICS_PREPROCESS_KIND => preprocess_ports(),
            PYRADIOMICS_EXTRACT_KIND => extract_ports(),
            PYRADIOMICS_BATCH_EXTRACT_KIND => batch_extract_ports(),
            RADIOMICS_DICOM_METADATA_KIND => dicom_metadata_ports(),
            RADIOMICS_PHI_SCRUB_KIND => phi_scrub_ports(),
            RADIOMICS_VOI_SIMILARITY_KIND => voi_similarity_ports(),
            RADIOMICS_IMAGE_QC_KIND => image_qc_ports(),
            RADIOMICS_RTSTRUCT_GEOMETRY_KIND => rtstruct_geometry_ports(),
            RADIOMICS_IVH_KIND => ivh_ports(),
            RADIOMICS_SHAPE_TOPOLOGY_KIND => shape_topology_ports(),
            RADIOMICS_REGISTER_KIND => register_ports(),
            RADIOMICS_DELTA_FEATURES_KIND => delta_features_ports(),
            _ => image_ingest_ports(),
        }
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let container = match self.kind {
            RADIOMICS_IMAGE_INGEST_KIND => {
                let spec: RadiomicsArtifactSpec = deserialize_spec(spec)?;
                image_ingest_container_spec(&spec)
            }
            RADIOMICS_MASK_INGEST_KIND => {
                let spec: RadiomicsMaskIngestSpec = deserialize_spec(spec)?;
                mask_ingest_container_spec(&spec)
            }
            RADIOMICS_PAIR_VALIDATE_KIND => {
                let spec: RadiomicsPairValidateSpec = deserialize_spec(spec)?;
                pair_validate_container_spec(&spec)
            }
            RADIOMICS_PREPROCESS_KIND => {
                let spec: RadiomicsPreprocessSpec = deserialize_spec(spec)?;
                preprocess_container_spec(&spec)
            }
            PYRADIOMICS_EXTRACT_KIND => {
                let spec: PyradiomicsExtractSpec = deserialize_spec(spec)?;
                extract_container_spec(&spec)
            }
            PYRADIOMICS_BATCH_EXTRACT_KIND => {
                let spec: PyradiomicsBatchExtractSpec = deserialize_spec(spec)?;
                batch_extract_container_spec(&spec)
            }
            RADIOMICS_DICOM_METADATA_KIND => {
                let spec: RadiomicsDicomMetadataSpec = deserialize_spec(spec)?;
                dicom_metadata_container_spec(&spec)
            }
            RADIOMICS_PHI_SCRUB_KIND => {
                let spec: RadiomicsPhiScrubSpec = deserialize_spec(spec)?;
                phi_scrub_container_spec(&spec)
            }
            RADIOMICS_VOI_SIMILARITY_KIND => {
                let spec: RadiomicsVoiSimilaritySpec = deserialize_spec(spec)?;
                voi_similarity_container_spec(&spec)
            }
            RADIOMICS_IMAGE_QC_KIND => {
                let spec: RadiomicsImageQcSpec = deserialize_spec(spec)?;
                image_qc_container_spec(&spec)
            }
            RADIOMICS_RTSTRUCT_GEOMETRY_KIND => {
                let spec: RadiomicsRtstructGeometrySpec = deserialize_spec(spec)?;
                rtstruct_geometry_container_spec(&spec)
            }
            RADIOMICS_IVH_KIND => {
                let spec: RadiomicsIvhSpec = deserialize_spec(spec)?;
                ivh_container_spec(&spec)
            }
            RADIOMICS_SHAPE_TOPOLOGY_KIND => {
                let spec: RadiomicsShapeTopologySpec = deserialize_spec(spec)?;
                shape_topology_container_spec(&spec)
            }
            RADIOMICS_REGISTER_KIND => {
                let spec: RadiomicsRegisterSpec = deserialize_spec(spec)?;
                register_container_spec(&spec)
            }
            RADIOMICS_DELTA_FEATURES_KIND => {
                let spec: RadiomicsDeltaFeaturesSpec = deserialize_spec(spec)?;
                delta_features_container_spec(&spec)
            }
            _ => Err("unsupported radiomics container kind".into()),
        }
        .map_err(dag_core::registry::error::Error::Unknown)?;
        let ports = self.ports();
        let inner = self.build_container(container)?;
        Ok(Box::new(RadiomicsContainerNode {
            kind: self.kind,
            ports,
            inner,
        }))
    }
}

impl RadiomicsContainerNodeFactory {
    pub fn image_ingest(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_IMAGE_INGEST_KIND, runtime, panel_cache)
    }

    pub fn mask_ingest(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_MASK_INGEST_KIND, runtime, panel_cache)
    }

    pub fn pair_validate(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_PAIR_VALIDATE_KIND, runtime, panel_cache)
    }

    pub fn preprocess(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_PREPROCESS_KIND, runtime, panel_cache)
    }

    pub fn extract(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PYRADIOMICS_EXTRACT_KIND, runtime, panel_cache)
    }

    pub fn batch_extract(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PYRADIOMICS_BATCH_EXTRACT_KIND, runtime, panel_cache)
    }

    pub fn dicom_metadata(
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(RADIOMICS_DICOM_METADATA_KIND, runtime, panel_cache)
    }

    pub fn phi_scrub(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_PHI_SCRUB_KIND, runtime, panel_cache)
    }

    pub fn voi_similarity(
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(RADIOMICS_VOI_SIMILARITY_KIND, runtime, panel_cache)
    }

    pub fn image_qc(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_IMAGE_QC_KIND, runtime, panel_cache)
    }

    pub fn rtstruct_geometry(
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(RADIOMICS_RTSTRUCT_GEOMETRY_KIND, runtime, panel_cache)
    }

    pub fn ivh(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_IVH_KIND, runtime, panel_cache)
    }

    pub fn shape_topology(
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(RADIOMICS_SHAPE_TOPOLOGY_KIND, runtime, panel_cache)
    }

    pub fn register(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(RADIOMICS_REGISTER_KIND, runtime, panel_cache)
    }

    pub fn delta_features(
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(RADIOMICS_DELTA_FEATURES_KIND, runtime, panel_cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extraction_settings() -> RadiomicsExtractionSettings {
        RadiomicsExtractionSettings {
            mask_label: 1,
            bin_width: 25.0,
            resampled_spacing: None,
            force2d: false,
            force2d_dimension: 0,
            image_types: default_image_types(),
            feature_classes: default_feature_classes(),
            log_sigmas: default_log_sigmas(),
        }
    }

    #[test]
    fn container_contracts_are_fixed() {
        let image = image_ingest_container_spec(&RadiomicsArtifactSpec {
            dicom_order: RadiomicsDicomOrderSettings {
                z_sort: default_dicom_sort_mode(),
                z_direction: default_dicom_direction(),
            },
            artifact_prefix: default_image_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(
            image.image,
            acr_image(PYRADIOMICS_IMAGE_REPOSITORY, PYRADIOMICS_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(image.command[2], "ingest-image");
        assert_eq!(image.outputs.len(), 2);
        assert!(image.env.contains_key("RADIOMICS_IMAGE_SETTINGS"));

        let mask = mask_ingest_container_spec(&RadiomicsMaskIngestSpec {
            roi_name: Some("GTV_Mass".into()),
            dicom_order: RadiomicsDicomOrderSettings {
                z_sort: RadiomicsDicomSortMode::InstanceNumber,
                z_direction: RadiomicsDicomDirection::Descending,
            },
            artifact_prefix: default_mask_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(mask.command[2], "ingest-mask");
        assert!(mask.env.contains_key("RADIOMICS_MASK_SETTINGS"));

        let extract = extract_container_spec(&PyradiomicsExtractSpec {
            extraction_id: "case1".into(),
            patient_id: "patient1".into(),
            image_id: "image1".into(),
            roi_id: "gtv".into(),
            roi_name: Some("GTV_Mass".into()),
            modality: "CT".into(),
            preset_id: "pyradiomics_original_v1".into(),
            settings: extraction_settings(),
            artifact_prefix: default_extract_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(extract.outputs.len(), 5);
        assert!(extract.env.contains_key("RADIOMICS_EXTRACTION"));
        assert_eq!(extract.network, "isolated");
        assert!(extract.read_only_rootfs);

        let contracts = [
            (
                dicom_metadata_container_spec(&RadiomicsDicomMetadataSpec {
                    extra_tags: vec!["PatientAge".into()],
                    artifact_prefix: default_dicom_metadata_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                2,
            ),
            (
                phi_scrub_container_spec(&RadiomicsPhiScrubSpec {
                    keep_patient_id: true,
                    pseudonym: Some("IBSI".into()),
                    artifact_prefix: default_phi_scrub_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                3,
            ),
            (
                voi_similarity_container_spec(&RadiomicsVoiSimilaritySpec {
                    comparison_id: "same".into(),
                    mask_label: 1,
                    surface_tolerance_mm: 1.0,
                    artifact_prefix: default_voi_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                2,
            ),
            (
                image_qc_container_spec(&RadiomicsImageQcSpec {
                    extraction_id: "case".into(),
                    mask_label: 1,
                    artifact_prefix: default_image_qc_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                2,
            ),
            (
                rtstruct_geometry_container_spec(&RadiomicsRtstructGeometrySpec {
                    roi_name: None,
                    artifact_prefix: default_rt_geometry_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                2,
            ),
            (
                ivh_container_spec(&RadiomicsIvhSpec {
                    extraction_id: "case".into(),
                    mask_label: 1,
                    volume_fractions: default_volume_fractions(),
                    artifact_prefix: default_ivh_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                2,
            ),
            (
                shape_topology_container_spec(&RadiomicsShapeTopologySpec {
                    extraction_id: "case".into(),
                    mask_label: 1,
                    artifact_prefix: default_topology_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                3,
            ),
            (
                register_container_spec(&RadiomicsRegisterSpec {
                    transform_type: "rigid".into(),
                    iterations: 10,
                    learning_rate: 1.0,
                    sampling_percent: 0.1,
                    artifact_prefix: default_register_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                3,
            ),
            (
                delta_features_container_spec(&RadiomicsDeltaFeaturesSpec {
                    id_column: default_delta_id_column(),
                    timepoint_column: default_delta_time_column(),
                    baseline: default_baseline(),
                    followup: default_followup(),
                    artifact_prefix: default_delta_prefix(),
                    timeout_secs: default_timeout(),
                })
                .unwrap(),
                2,
            ),
        ];
        for (container, output_count) in contracts {
            assert_eq!(
                container.image,
                acr_image(PYRADIOMICS_IMAGE_REPOSITORY, PYRADIOMICS_IMAGE_DIGEST).unwrap()
            );
            assert_eq!(container.outputs.len(), output_count);
            assert_eq!(container.network, "isolated");
        }
    }

    #[test]
    fn rejects_invalid_geometry_and_extraction_settings() {
        assert!(
            pair_validate_container_spec(&RadiomicsPairValidateSpec {
                extraction_id: "case".into(),
                mask_label: 0,
                minimum_mask_voxels: 1,
                geometry_tolerance_mm: 0.01,
                artifact_prefix: default_validate_prefix(),
                timeout_secs: default_timeout(),
            })
            .is_err()
        );

        let mut settings = extraction_settings();
        settings.bin_width = 0.0;
        assert!(settings.validate().is_err());

        settings = extraction_settings();
        settings.image_types = vec!["Original".into(), "Wavelet".into()];
        settings.feature_classes = vec!["firstorder".into()];
        assert!(settings.validate().is_ok());

        settings.image_types = vec!["Nonsense".into()];
        assert!(settings.validate().is_err());

        settings.image_types = vec!["Original".into()];
        settings.feature_classes = vec!["ngldm".into()];
        assert!(settings.validate().is_err());
    }
}
