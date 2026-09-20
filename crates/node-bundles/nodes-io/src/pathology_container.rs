//! Thin DAG wrappers for the pinned pathology Stage-A/B OCI image.
//!
//! The runner contract mirrors the radiomics containers: inputs arrive as
//! `AUTONOMICS_INPUT{n}`, outputs are declared files under `/work`, and every
//! command reads its knobs from a `PATHOLOGY_*_SETTINGS` JSON environment
//! variable. Embedding weights are never downloaded — `wsi-embed` takes the
//! operator-staged checkpoint as an input FileSet because UNI-class models
//! are license-gated.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use container_runtime::{PanelCache, PodmanConnection, PullPolicy};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;
use dag_core::{DataBundle, dag::DagError, dag::graph::PortOutputs};

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::acr_image;

pub const PATHOLOGY_IMAGE_REPOSITORY: &str = "pathology";
pub const PATHOLOGY_IMAGE_DIGEST: &str =
    "sha256:a0edcb6cca25f009f669723406207651284960425f7255891be5b91b29b63f2f";
pub const PATHOLOGY_WSI_INGEST_KIND: &str = "pathology_wsi_ingest";
pub const PATHOLOGY_WSI_QC_KIND: &str = "pathology_wsi_qc";
pub const PATHOLOGY_PATCH_SAMPLE_KIND: &str = "pathology_patch_sample";
pub const PATHOLOGY_WSI_EMBED_KIND: &str = "pathology_wsi_embed";
pub const PATHOLOGY_DOMAIN_CHECK_KIND: &str = "pathology_domain_check";
pub const PATHOLOGY_IHC_QUANT_KIND: &str = "pathology_ihc_quant";
pub const PATHOLOGY_QUPATH_IMPORT_KIND: &str = "pathology_qupath_import";

const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_EMBED_TIMEOUT_SECS: u64 = 7200;

fn default_wsi_ingest_prefix() -> String {
    "/artifacts/pathology_wsi_ingest".into()
}
fn default_wsi_qc_prefix() -> String {
    "/artifacts/pathology_wsi_qc".into()
}
fn default_patch_sample_prefix() -> String {
    "/artifacts/pathology_patch_sample".into()
}
fn default_wsi_embed_prefix() -> String {
    "/artifacts/pathology_wsi_embed".into()
}
fn default_domain_check_prefix() -> String {
    "/artifacts/pathology_domain_check".into()
}
fn default_ihc_quant_prefix() -> String {
    "/artifacts/pathology_ihc_quant".into()
}
fn default_qupath_import_prefix() -> String {
    "/artifacts/pathology_qupath_import".into()
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}
fn default_embed_timeout() -> u64 {
    DEFAULT_EMBED_TIMEOUT_SECS
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

fn unit_interval(value: f64, name: &str) -> Result<(), String> {
    if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
        return Err(format!("{name} must lie in [0, 1]"));
    }
    Ok(())
}

/// Serializes every field of the spec except the artifact plumbing, so the
/// runner's `PATHOLOGY_*_SETTINGS` sees exactly the command knobs.
fn settings_json<T: Serialize>(spec: &T) -> Result<String, String> {
    let mut value = serde_json::to_value(spec).map_err(|error| error.to_string())?;
    if let Some(object) = value.as_object_mut() {
        object.remove("artifact_prefix");
        object.remove("timeout_secs");
    }
    serde_json::to_string(&value).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyWsiIngestSpec {
    /// Longest thumbnail edge in pixels.
    #[serde(default = "default_thumbnail_width")]
    pub max_thumbnail_width: u32,
    #[serde(default = "default_wsi_ingest_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_thumbnail_width() -> u32 {
    2048
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyWsiQcSpec {
    /// Pyramid level to sample; `-1` picks the finest level within
    /// `max_downsample`.
    #[serde(default = "default_qc_level")]
    pub level: i32,
    #[serde(default = "default_qc_max_downsample")]
    pub max_downsample: f64,
    #[serde(default = "default_qc_tile_size")]
    pub tile_size: u32,
    #[serde(default = "default_qc_max_tiles")]
    pub max_tiles: u32,
    #[serde(default = "default_saturation_threshold")]
    pub saturation_threshold: f64,
    #[serde(default = "default_value_floor")]
    pub value_floor: f64,
    #[serde(default = "default_value_ceiling")]
    pub value_ceiling: f64,
    /// Laplacian-variance floor for the focus check.
    #[serde(default = "default_focus_threshold")]
    pub focus_threshold: f64,
    #[serde(default = "default_min_slide_tissue_fraction")]
    pub min_slide_tissue_fraction: f64,
    #[serde(default = "default_wsi_qc_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_qc_level() -> i32 {
    -1
}
fn default_qc_max_downsample() -> f64 {
    16.0
}
fn default_qc_tile_size() -> u32 {
    512
}
fn default_qc_max_tiles() -> u32 {
    400
}
fn default_saturation_threshold() -> f64 {
    0.2
}
fn default_value_floor() -> f64 {
    0.15
}
fn default_value_ceiling() -> f64 {
    0.92
}
fn default_focus_threshold() -> f64 {
    40.0
}
fn default_min_slide_tissue_fraction() -> f64 {
    0.05
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyPatchSampleSpec {
    /// Pyramid level the patch grid is defined on; coordinates are emitted as
    /// level-0 pixels either way.
    #[serde(default)]
    pub level: u32,
    /// Patch edge in pixels at `level`.
    #[serde(default = "default_patch_size")]
    pub patch_size: u32,
    #[serde(default = "default_max_patches")]
    pub max_patches: u32,
    /// Minimum tissue fraction for a candidate patch to be selectable.
    #[serde(default = "default_min_tissue_fraction")]
    pub min_tissue_fraction: f64,
    /// Seed fixing the deterministic selection among candidates.
    #[serde(default)]
    pub seed: i64,
    /// Tissue-mask pyramid level cap and working-resolution bounds.
    #[serde(default = "default_mask_max_downsample")]
    pub mask_max_downsample: f64,
    #[serde(default = "default_mask_max_width")]
    pub mask_max_width: u32,
    #[serde(default = "default_mask_open_radius")]
    pub mask_open_radius: u32,
    #[serde(default = "default_mask_close_radius")]
    pub mask_close_radius: u32,
    #[serde(default = "default_mask_min_object_px")]
    pub mask_min_object_px: u32,
    #[serde(default = "default_saturation_threshold")]
    pub saturation_threshold: f64,
    #[serde(default = "default_value_floor")]
    pub value_floor: f64,
    #[serde(default = "default_value_ceiling")]
    pub value_ceiling: f64,
    #[serde(default = "default_patch_sample_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_patch_size() -> u32 {
    256
}
fn default_max_patches() -> u32 {
    5000
}
fn default_min_tissue_fraction() -> f64 {
    0.5
}
fn default_mask_max_downsample() -> f64 {
    64.0
}
fn default_mask_max_width() -> u32 {
    4096
}
fn default_mask_open_radius() -> u32 {
    3
}
fn default_mask_close_radius() -> u32 {
    3
}
fn default_mask_min_object_px() -> u32 {
    500
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyWsiEmbedSpec {
    /// Patches forwarded per inference step.
    #[serde(default = "default_embed_batch_size")]
    pub batch_size: u32,
    /// `auto` (CUDA when visible), `cpu`, or `cuda`.
    #[serde(default = "default_embed_device")]
    pub device: String,
    /// FP16 autocast on CUDA devices.
    #[serde(default)]
    pub amp: bool,
    /// GPU passthrough handed to the runtime; defaults to `all`. Pass `null`
    /// explicitly on hosts without the nvidia-container-toolkit CDI spec (the
    /// CPU image then runs on CPU via `device: auto`).
    #[serde(default = "default_embed_gpus")]
    pub gpus: Option<String>,
    #[serde(default = "default_wsi_embed_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_embed_timeout")]
    pub timeout_secs: u64,
}

fn default_embed_batch_size() -> u32 {
    32
}
fn default_embed_device() -> String {
    "auto".into()
}
fn default_embed_gpus() -> Option<String> {
    Some("all".into())
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyDomainCheckSpec {
    /// L2-normalize embeddings before the distance computations.
    #[serde(default = "default_true")]
    pub l2_normalize: bool,
    /// Ledoit-style diagonal shrinkage of the pooled covariance.
    #[serde(default = "default_shrinkage")]
    pub shrinkage: f64,
    /// Silhouette subsample cap for large embedding tables.
    #[serde(default = "default_silhouette_max_samples")]
    pub silhouette_max_samples: u32,
    /// Seed fixing the silhouette subsample.
    #[serde(default)]
    pub seed: i64,
    #[serde(default = "default_domain_check_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}
fn default_shrinkage() -> f64 {
    0.1
}
fn default_silhouette_max_samples() -> u32 {
    20_000
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyIhcQuantSpec {
    /// Label to quantify inside the ROI mask; the smallest label when omitted.
    #[serde(default)]
    pub roi_label: Option<u32>,
    /// ROI mask pixels per level-0 pixel (mask level / level-0 scale factor).
    #[serde(default = "default_mask_downsample")]
    pub mask_downsample: f64,
    /// Hematoxylin-and-DAB thresholds in rgb2hed concentration units.
    #[serde(default = "default_dab_weak_threshold")]
    pub dab_weak_threshold: f64,
    #[serde(default = "default_dab_strong_threshold")]
    pub dab_strong_threshold: f64,
    /// Finest analysis level cap as a downsample factor.
    #[serde(default = "default_ihc_max_downsample")]
    pub max_downsample: f64,
    #[serde(default = "default_qc_tile_size")]
    pub tile_size: u32,
    #[serde(default = "default_ihc_max_tiles")]
    pub max_tiles: u32,
    #[serde(default = "default_ihc_quant_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_mask_downsample() -> f64 {
    16.0
}
fn default_dab_weak_threshold() -> f64 {
    0.15
}
fn default_dab_strong_threshold() -> f64 {
    0.35
}
fn default_ihc_max_downsample() -> f64 {
    4.0
}
fn default_ihc_max_tiles() -> u32 {
    20_000
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathologyQupathImportSpec {
    #[serde(default = "default_mask_downsample")]
    pub mask_downsample: f64,
    /// Label value to classification name, for example `{"1": "Tumor"}`.
    #[serde(default)]
    pub label_names: BTreeMap<String, String>,
    /// Douglas-Peucker tolerance in mask pixels.
    #[serde(default = "default_simplify_tolerance_px")]
    pub simplify_tolerance_px: f64,
    /// Regions below this many mask pixels are dropped.
    #[serde(default = "default_min_region_px")]
    pub min_region_px: u32,
    #[serde(default = "default_qupath_import_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_simplify_tolerance_px() -> f64 {
    2.0
}
fn default_min_region_px() -> u32 {
    200
}

pub struct PathologyContainerNode {
    kind: &'static str,
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for PathologyContainerNode {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait]
impl DagNode for PathologyContainerNode {
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
        // The runner contract is input 0=WSI, 1=table/mask, 2=weights,
        // independent of scheduler edge insertion order.
        let mut ordered_inputs = inputs.to_vec();
        ordered_inputs.sort_by_key(|input| input.port);
        self.inner.execute(ctx, &ordered_inputs, reporter).await
    }
}

pub struct PathologyContainerNodeFactory {
    kind: &'static str,
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl PathologyContainerNodeFactory {
    fn new(
        kind: &'static str,
        runtime: Arc<dyn PodmanConnection>,
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
        image: acr_image(PATHOLOGY_IMAGE_REPOSITORY, PATHOLOGY_IMAGE_DIGEST)?,
        command: vec![
            "python".into(),
            "/opt/pathology/pathology_runner.py".into(),
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
        gpus: None,
        user: None,
    })
}

pub fn wsi_ingest_container_spec(
    spec: &PathologyWsiIngestSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.max_thumbnail_width < 64 {
        return Err("max_thumbnail_width must be at least 64".into());
    }
    let mut container = base_spec("wsi-ingest", &spec.artifact_prefix, spec.timeout_secs)?;
    container
        .env
        .insert("PATHOLOGY_WSI_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("thumbnail.png", "png"),
        output("slide_meta.json", "json"),
    ];
    Ok(container)
}

pub fn wsi_qc_container_spec(spec: &PathologyWsiQcSpec) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.level < -1 {
        return Err("level must be -1 or a nonnegative pyramid level".into());
    }
    if spec.tile_size < 64 {
        return Err("tile_size must be at least 64".into());
    }
    if spec.max_tiles == 0 {
        return Err("max_tiles must be positive".into());
    }
    if !(spec.max_downsample.is_finite() && spec.max_downsample >= 1.0) {
        return Err("max_downsample must be finite and >= 1".into());
    }
    for (name, value) in [
        ("saturation_threshold", spec.saturation_threshold),
        ("value_floor", spec.value_floor),
        ("value_ceiling", spec.value_ceiling),
        ("min_slide_tissue_fraction", spec.min_slide_tissue_fraction),
    ] {
        unit_interval(value, name)?;
    }
    if spec.value_floor >= spec.value_ceiling {
        return Err("value_floor must be below value_ceiling".into());
    }
    if !spec.focus_threshold.is_finite() || spec.focus_threshold < 0.0 {
        return Err("focus_threshold must be finite and nonnegative".into());
    }
    let mut container = base_spec("wsi-qc", &spec.artifact_prefix, spec.timeout_secs)?;
    container
        .env
        .insert("PATHOLOGY_QC_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("tile_qc.parquet", "parquet"),
        output("qc_summary.json", "json"),
    ];
    Ok(container)
}

pub fn patch_sample_container_spec(
    spec: &PathologyPatchSampleSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.patch_size < 32 {
        return Err("patch_size must be at least 32".into());
    }
    if spec.max_patches == 0 {
        return Err("max_patches must be positive".into());
    }
    if spec.mask_max_width < 256 {
        return Err("mask_max_width must be at least 256".into());
    }
    if !(spec.mask_max_downsample.is_finite() && spec.mask_max_downsample >= 1.0) {
        return Err("mask_max_downsample must be finite and >= 1".into());
    }
    unit_interval(spec.min_tissue_fraction, "min_tissue_fraction")?;
    for (name, value) in [
        ("saturation_threshold", spec.saturation_threshold),
        ("value_floor", spec.value_floor),
        ("value_ceiling", spec.value_ceiling),
    ] {
        unit_interval(value, name)?;
    }
    if spec.value_floor >= spec.value_ceiling {
        return Err("value_floor must be below value_ceiling".into());
    }
    let mut container = base_spec("patch-sample", &spec.artifact_prefix, spec.timeout_secs)?;
    container
        .env
        .insert("PATHOLOGY_PATCH_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("patches.parquet", "parquet"),
        output("tissue_mask.png", "png"),
        output("patch_meta.json", "json"),
    ];
    Ok(container)
}

pub fn wsi_embed_container_spec(
    spec: &PathologyWsiEmbedSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if spec.batch_size == 0 {
        return Err("batch_size must be positive".into());
    }
    match spec.device.as_str() {
        "auto" | "cpu" | "cuda" => {}
        other => {
            return Err(format!(
                "unsupported device `{other}`; use auto, cpu, or cuda"
            ));
        }
    }
    let mut container = base_spec("wsi-embed", &spec.artifact_prefix, spec.timeout_secs)?;
    container.gpus = spec.gpus.clone();
    container
        .env
        .insert("PATHOLOGY_EMBED_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("embeddings.h5", "hdf5"),
        output("embed_meta.json", "json"),
    ];
    Ok(container)
}

pub fn domain_check_container_spec(
    spec: &PathologyDomainCheckSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if !(spec.shrinkage.is_finite() && (0.0..1.0).contains(&spec.shrinkage)) {
        return Err("shrinkage must lie in [0, 1)".into());
    }
    if spec.silhouette_max_samples < 2 {
        return Err("silhouette_max_samples must be at least 2".into());
    }
    let mut container = base_spec("domain-check", &spec.artifact_prefix, spec.timeout_secs)?;
    container
        .env
        .insert("PATHOLOGY_DOMAIN_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("domain_metrics.parquet", "parquet"),
        output("domain_meta.json", "json"),
    ];
    Ok(container)
}

pub fn ihc_quant_container_spec(
    spec: &PathologyIhcQuantSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if !(spec.mask_downsample.is_finite() && spec.mask_downsample >= 1.0) {
        return Err("mask_downsample must be finite and >= 1".into());
    }
    if !(spec.max_downsample.is_finite() && spec.max_downsample >= 1.0) {
        return Err("max_downsample must be finite and >= 1".into());
    }
    if spec.tile_size < 64 {
        return Err("tile_size must be at least 64".into());
    }
    if spec.max_tiles == 0 {
        return Err("max_tiles must be positive".into());
    }
    if !(0.0 < spec.dab_weak_threshold && spec.dab_weak_threshold < spec.dab_strong_threshold) {
        return Err("thresholds must satisfy 0 < dab_weak_threshold < dab_strong_threshold".into());
    }
    if !spec.dab_strong_threshold.is_finite() {
        return Err("dab_strong_threshold must be finite".into());
    }
    let mut container = base_spec("ihc-quant", &spec.artifact_prefix, spec.timeout_secs)?;
    container
        .env
        .insert("PATHOLOGY_IHC_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("tile_ihc.parquet", "parquet"),
        output("ihc_summary.json", "json"),
    ];
    Ok(container)
}

pub fn qupath_import_container_spec(
    spec: &PathologyQupathImportSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_prefix_timeout(&spec.artifact_prefix, spec.timeout_secs)?;
    if !(spec.mask_downsample.is_finite() && spec.mask_downsample >= 1.0) {
        return Err("mask_downsample must be finite and >= 1".into());
    }
    if !(spec.simplify_tolerance_px.is_finite() && spec.simplify_tolerance_px >= 0.0) {
        return Err("simplify_tolerance_px must be finite and nonnegative".into());
    }
    for key in spec.label_names.keys() {
        if key.parse::<u32>().is_err() {
            return Err(format!("label_names keys must be integers; got `{key}`"));
        }
    }
    let mut container = base_spec("qupath-import", &spec.artifact_prefix, spec.timeout_secs)?;
    container
        .env
        .insert("PATHOLOGY_QUPATH_SETTINGS".into(), settings_json(spec)?);
    container.outputs = vec![
        output("annotations.geojson", "geojson"),
        output("geojson_meta.json", "json"),
    ];
    Ok(container)
}

fn output(path: &str, format: &str) -> ContainerCommandOutputSpec {
    ContainerCommandOutputSpec {
        path: path.into(),
        format: Some(format.into()),
    }
}

fn wsi_input_and_two_files() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::Any)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn patch_sample_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::Any)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn wsi_embed_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::Any, "wsi")
        .add_input_port_of_type_with_label(None, PortType::File, "patch_table")
        .add_input_port_of_type_with_label(None, PortType::FileSet, "model_weights")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn two_embedding_inputs_and_two_files() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "reference_embeddings")
        .add_input_port_of_type_with_label(None, PortType::File, "comparison_embeddings")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn wsi_mask_inputs_and_two_files() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::Any, "wsi")
        .add_input_port_of_type_with_label(None, PortType::Any, "roi_mask")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn qupath_import_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::Any, "annotation_mask")
        .add_input_port_of_type_with_label(None, PortType::Any, "reference_wsi")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn deserialize_spec<T: for<'de> Deserialize<'de>>(
    spec: serde_json::Value,
) -> dag_core::registry::error::Result<T> {
    serde_json::from_value(spec).map_err(dag_core::registry::error::Error::from)
}

impl NodeFactory for PathologyContainerNodeFactory {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn desc(&self) -> &'static str {
        match self.kind {
            PATHOLOGY_WSI_INGEST_KIND => {
                "Ingests a whole-slide image and emits a thumbnail plus metadata."
            }
            PATHOLOGY_WSI_QC_KIND => "Tile-level focus, tissue, and blanking QC for one slide.",
            PATHOLOGY_PATCH_SAMPLE_KIND => {
                "Deterministic tissue-aware patch sampling for one slide."
            }
            PATHOLOGY_WSI_EMBED_KIND => "Embeds sampled patches with a staged foundation model.",
            PATHOLOGY_DOMAIN_CHECK_KIND => "Compares two embedding tables for batch effects.",
            PATHOLOGY_IHC_QUANT_KIND => "Quantifies DAB positivity and H-score inside an ROI.",
            PATHOLOGY_QUPATH_IMPORT_KIND => "Exports a label mask as QuPath GeoJSON annotations.",
            _ => "Pathology container node.",
        }
    }

    fn doc(&self) -> &'static str {
        match self.kind {
            PATHOLOGY_WSI_INGEST_KIND => {
                "Reads one WSI through tiffslide, writes a bounded thumbnail, and records dimensions, pyramid levels, downsamples, vendor, microns per pixel, and the source sha256."
            }
            PATHOLOGY_WSI_QC_KIND => {
                "Samples tiles on a strided grid at one pyramid level and emits per-tile tissue fraction, Laplacian-variance focus, and channel means, plus slide-level checks (tissue present, focus acceptable, not mostly blank)."
            }
            PATHOLOGY_PATCH_SAMPLE_KIND => {
                "Builds a morphologically cleaned tissue mask at a bounded working resolution, enumerates the full patch grid with per-patch tissue fractions, then deterministically selects up to max_patches candidates with a seeded permutation. Emits the patch table (level-0 coordinates), the tissue mask, and sampling metadata."
            }
            PATHOLOGY_WSI_EMBED_KIND => {
                "Input port 0 is the WSI, port 1 the patch table from patch-sample, port 2 the locally staged model bundle (optional model_config.json plus exactly one checkpoint; nothing is downloaded — UNI-class checkpoints are license-gated). Runs timm inference with ImageNet normalization and writes float32 embeddings, patch IDs, and coordinates to HDF5. Defaults to `gpus: all`; pass `gpus: null` on hosts without the nvidia CDI spec."
            }
            PATHOLOGY_DOMAIN_CHECK_KIND => {
                "Input port 0 is the reference embedding table and port 1 the comparison table. Emits centroid cosine and shrunk-covariance Mahalanobis distances, batch silhouette (seeded subsample), and the most shifted dimensions, for deciding whether cohorts may be pooled."
            }
            PATHOLOGY_IHC_QUANT_KIND => {
                "Input port 0 is the WSI and port 1 an ROI mask image with a configured mask-to-level-0 scale. Decomposes tiles into hematoxylin/DAB channels, thresholds DAB positivity in three intensity bins, and emits per-tile tables plus a slide-level H-score."
            }
            PATHOLOGY_QUPATH_IMPORT_KIND => {
                "Input port 0 is a label mask and port 1 the reference WSI. Traces, simplifies, and scales region contours to level-0 pixels and emits a QuPath-compatible FeatureCollection with classification names."
            }
            _ => "Pathology container node.",
        }
    }

    fn spec_schema(&self) -> schemars::Schema {
        match self.kind {
            PATHOLOGY_WSI_INGEST_KIND => schema_for!(PathologyWsiIngestSpec),
            PATHOLOGY_WSI_QC_KIND => schema_for!(PathologyWsiQcSpec),
            PATHOLOGY_PATCH_SAMPLE_KIND => schema_for!(PathologyPatchSampleSpec),
            PATHOLOGY_WSI_EMBED_KIND => schema_for!(PathologyWsiEmbedSpec),
            PATHOLOGY_DOMAIN_CHECK_KIND => schema_for!(PathologyDomainCheckSpec),
            PATHOLOGY_IHC_QUANT_KIND => schema_for!(PathologyIhcQuantSpec),
            PATHOLOGY_QUPATH_IMPORT_KIND => schema_for!(PathologyQupathImportSpec),
            _ => schema_for!(PathologyWsiIngestSpec),
        }
    }

    fn ports(&self) -> NodePorts {
        match self.kind {
            PATHOLOGY_WSI_INGEST_KIND | PATHOLOGY_WSI_QC_KIND => wsi_input_and_two_files(),
            PATHOLOGY_PATCH_SAMPLE_KIND => patch_sample_ports(),
            PATHOLOGY_WSI_EMBED_KIND => wsi_embed_ports(),
            PATHOLOGY_DOMAIN_CHECK_KIND => two_embedding_inputs_and_two_files(),
            PATHOLOGY_IHC_QUANT_KIND => wsi_mask_inputs_and_two_files(),
            PATHOLOGY_QUPATH_IMPORT_KIND => qupath_import_ports(),
            _ => wsi_input_and_two_files(),
        }
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let container = match self.kind {
            PATHOLOGY_WSI_INGEST_KIND => {
                let spec: PathologyWsiIngestSpec = deserialize_spec(spec)?;
                wsi_ingest_container_spec(&spec)
            }
            PATHOLOGY_WSI_QC_KIND => {
                let spec: PathologyWsiQcSpec = deserialize_spec(spec)?;
                wsi_qc_container_spec(&spec)
            }
            PATHOLOGY_PATCH_SAMPLE_KIND => {
                let spec: PathologyPatchSampleSpec = deserialize_spec(spec)?;
                patch_sample_container_spec(&spec)
            }
            PATHOLOGY_WSI_EMBED_KIND => {
                let spec: PathologyWsiEmbedSpec = deserialize_spec(spec)?;
                wsi_embed_container_spec(&spec)
            }
            PATHOLOGY_DOMAIN_CHECK_KIND => {
                let spec: PathologyDomainCheckSpec = deserialize_spec(spec)?;
                domain_check_container_spec(&spec)
            }
            PATHOLOGY_IHC_QUANT_KIND => {
                let spec: PathologyIhcQuantSpec = deserialize_spec(spec)?;
                ihc_quant_container_spec(&spec)
            }
            PATHOLOGY_QUPATH_IMPORT_KIND => {
                let spec: PathologyQupathImportSpec = deserialize_spec(spec)?;
                qupath_import_container_spec(&spec)
            }
            _ => Err("unsupported pathology container kind".into()),
        }
        .map_err(dag_core::registry::error::Error::Unknown)?;
        let ports = self.ports();
        let inner = self.build_container(container)?;
        Ok(Box::new(PathologyContainerNode {
            kind: self.kind,
            ports,
            inner,
        }))
    }
}

impl PathologyContainerNodeFactory {
    pub fn wsi_ingest(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_WSI_INGEST_KIND, runtime, panel_cache)
    }

    pub fn wsi_qc(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_WSI_QC_KIND, runtime, panel_cache)
    }

    pub fn patch_sample(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_PATCH_SAMPLE_KIND, runtime, panel_cache)
    }

    pub fn wsi_embed(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_WSI_EMBED_KIND, runtime, panel_cache)
    }

    pub fn domain_check(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_DOMAIN_CHECK_KIND, runtime, panel_cache)
    }

    pub fn ihc_quant(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_IHC_QUANT_KIND, runtime, panel_cache)
    }

    pub fn qupath_import(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(PATHOLOGY_QUPATH_IMPORT_KIND, runtime, panel_cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_contracts_are_fixed() {
        let ingest = wsi_ingest_container_spec(&PathologyWsiIngestSpec {
            max_thumbnail_width: default_thumbnail_width(),
            artifact_prefix: default_wsi_ingest_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(
            ingest.image,
            acr_image(PATHOLOGY_IMAGE_REPOSITORY, PATHOLOGY_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(ingest.command[2], "wsi-ingest");
        assert_eq!(ingest.outputs.len(), 2);
        assert!(ingest.env.contains_key("PATHOLOGY_WSI_SETTINGS"));
        assert_eq!(ingest.gpus.as_deref(), None);

        let qc = wsi_qc_container_spec(&PathologyWsiQcSpec {
            level: default_qc_level(),
            max_downsample: default_qc_max_downsample(),
            tile_size: default_qc_tile_size(),
            max_tiles: default_qc_max_tiles(),
            saturation_threshold: default_saturation_threshold(),
            value_floor: default_value_floor(),
            value_ceiling: default_value_ceiling(),
            focus_threshold: default_focus_threshold(),
            min_slide_tissue_fraction: default_min_slide_tissue_fraction(),
            artifact_prefix: default_wsi_qc_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(qc.command[2], "wsi-qc");
        assert!(qc.env.contains_key("PATHOLOGY_QC_SETTINGS"));

        let patches = patch_sample_container_spec(&PathologyPatchSampleSpec {
            level: 0,
            patch_size: default_patch_size(),
            max_patches: default_max_patches(),
            min_tissue_fraction: default_min_tissue_fraction(),
            seed: 0,
            mask_max_downsample: default_mask_max_downsample(),
            mask_max_width: default_mask_max_width(),
            mask_open_radius: default_mask_open_radius(),
            mask_close_radius: default_mask_close_radius(),
            mask_min_object_px: default_mask_min_object_px(),
            saturation_threshold: default_saturation_threshold(),
            value_floor: default_value_floor(),
            value_ceiling: default_value_ceiling(),
            artifact_prefix: default_patch_sample_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(patches.command[2], "patch-sample");
        assert_eq!(patches.outputs.len(), 3);
        assert!(patches.env.contains_key("PATHOLOGY_PATCH_SETTINGS"));
        let settings: serde_json::Value =
            serde_json::from_str(patches.env.get("PATHOLOGY_PATCH_SETTINGS").unwrap()).unwrap();
        assert_eq!(settings["seed"], 0);
        assert!(settings.get("artifact_prefix").is_none());

        let embed = wsi_embed_container_spec(&PathologyWsiEmbedSpec {
            batch_size: default_embed_batch_size(),
            device: default_embed_device(),
            amp: false,
            gpus: default_embed_gpus(),
            artifact_prefix: default_wsi_embed_prefix(),
            timeout_secs: default_embed_timeout(),
        })
        .unwrap();
        assert_eq!(embed.command[2], "wsi-embed");
        assert_eq!(embed.timeout_secs, DEFAULT_EMBED_TIMEOUT_SECS);
        assert_eq!(embed.gpus.as_deref(), Some("all"));
        assert!(embed.env.contains_key("PATHOLOGY_EMBED_SETTINGS"));

        let embed_cpu = wsi_embed_container_spec(&PathologyWsiEmbedSpec {
            gpus: None,
            device: "cpu".into(),
            batch_size: 8,
            amp: false,
            artifact_prefix: default_wsi_embed_prefix(),
            timeout_secs: default_embed_timeout(),
        })
        .unwrap();
        assert_eq!(embed_cpu.gpus, None);

        let domain = domain_check_container_spec(&PathologyDomainCheckSpec {
            l2_normalize: true,
            shrinkage: default_shrinkage(),
            silhouette_max_samples: default_silhouette_max_samples(),
            seed: 0,
            artifact_prefix: default_domain_check_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(domain.command[2], "domain-check");
        assert!(domain.env.contains_key("PATHOLOGY_DOMAIN_SETTINGS"));

        let ihc = ihc_quant_container_spec(&PathologyIhcQuantSpec {
            roi_label: Some(1),
            mask_downsample: default_mask_downsample(),
            dab_weak_threshold: default_dab_weak_threshold(),
            dab_strong_threshold: default_dab_strong_threshold(),
            max_downsample: default_ihc_max_downsample(),
            tile_size: default_qc_tile_size(),
            max_tiles: default_ihc_max_tiles(),
            artifact_prefix: default_ihc_quant_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(ihc.command[2], "ihc-quant");
        assert!(ihc.env.contains_key("PATHOLOGY_IHC_SETTINGS"));

        let qupath = qupath_import_container_spec(&PathologyQupathImportSpec {
            mask_downsample: default_mask_downsample(),
            label_names: BTreeMap::from([("1".to_string(), "Tumor".to_string())]),
            simplify_tolerance_px: default_simplify_tolerance_px(),
            min_region_px: default_min_region_px(),
            artifact_prefix: default_qupath_import_prefix(),
            timeout_secs: default_timeout(),
        })
        .unwrap();
        assert_eq!(qupath.command[2], "qupath-import");
        assert!(qupath.env.contains_key("PATHOLOGY_QUPATH_SETTINGS"));
    }

    #[test]
    fn invalid_settings_are_rejected() {
        assert!(
            wsi_qc_container_spec(&PathologyWsiQcSpec {
                level: -2,
                max_downsample: 16.0,
                tile_size: 512,
                max_tiles: 400,
                saturation_threshold: 0.2,
                value_floor: 0.95,
                value_ceiling: 0.92,
                focus_threshold: 40.0,
                min_slide_tissue_fraction: 0.05,
                artifact_prefix: default_wsi_qc_prefix(),
                timeout_secs: default_timeout(),
            })
            .is_err()
        );

        assert!(
            wsi_embed_container_spec(&PathologyWsiEmbedSpec {
                batch_size: 32,
                device: "tpu".into(),
                amp: false,
                gpus: None,
                artifact_prefix: default_wsi_embed_prefix(),
                timeout_secs: default_embed_timeout(),
            })
            .is_err()
        );

        assert!(
            ihc_quant_container_spec(&PathologyIhcQuantSpec {
                roi_label: None,
                mask_downsample: 16.0,
                dab_weak_threshold: 0.4,
                dab_strong_threshold: 0.35,
                max_downsample: 4.0,
                tile_size: 512,
                max_tiles: 20_000,
                artifact_prefix: default_ihc_quant_prefix(),
                timeout_secs: default_timeout(),
            })
            .is_err()
        );

        assert!(
            domain_check_container_spec(&PathologyDomainCheckSpec {
                l2_normalize: true,
                shrinkage: 1.0,
                silhouette_max_samples: 20_000,
                seed: 0,
                artifact_prefix: default_domain_check_prefix(),
                timeout_secs: default_timeout(),
            })
            .is_err()
        );

        assert!(
            patch_sample_container_spec(&PathologyPatchSampleSpec {
                level: 0,
                patch_size: 256,
                max_patches: 5000,
                min_tissue_fraction: 1.5,
                seed: 0,
                mask_max_downsample: 64.0,
                mask_max_width: 4096,
                mask_open_radius: 3,
                mask_close_radius: 3,
                mask_min_object_px: 500,
                saturation_threshold: 0.2,
                value_floor: 0.15,
                value_ceiling: 0.92,
                artifact_prefix: "artifacts/patch".into(),
                timeout_secs: default_timeout(),
            })
            .is_err()
        );
    }
}
