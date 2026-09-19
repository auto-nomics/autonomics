//! Containerized H5AD-first single-cell workflow nodes.
//!
//! Each node keeps expression data behind a File port. The generic container
//! runtime stages the H5AD and publishes immutable outputs; only the Python
//! scverse process interprets the matrix payload.

use std::collections::BTreeMap;
use std::sync::Arc;

use dag_core::node::{DagNode, DataBundleBinding};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::file_reference::FileReferenceNode;
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const H5AD_QC_FILTER_KIND: &str = "h5ad_qc_filter";
pub const H5AD_PCA_NEIGHBORS_UMAP_LEIDEN_KIND: &str = "h5ad_pca_neighbors_umap_leiden";
pub const H5AD_CELLTYPIST_ANNOTATE_KIND: &str = "h5ad_celltypist_annotate";
pub const H5AD_OBS_TO_PARQUET_KIND: &str = "h5ad_obs_to_parquet";
pub const H5AD_SUBSET_BY_OBS_KIND: &str = "h5ad_subset_by_obs";
pub const SC_DENSE_INGEST_KIND: &str = "sc_dense_ingest";
pub const H5AD_RANK_GENES_GROUPS_KIND: &str = "h5ad_rank_genes_groups";
pub const H5AD_CLUSTER_MEAN_EXPRESSION_KIND: &str = "h5ad_cluster_mean_expression";
pub const H5AD_GENE_SET_SCORE_KIND: &str = "gene_set_score";
pub const CELLTYPIST_MODEL_BUNDLE: &str = "celltypist.models.pan_immune";
pub const DEFAULT_CELLTYPIST_MODEL_FILE: &str = "Immune_All_Low.pkl";
pub const SINGLE_CELL_WORKFLOW_IMAGE_REPOSITORY: &str = "single-cell-preprocessor";
pub const SINGLE_CELL_WORKFLOW_IMAGE_DIGEST: &str =
    "sha256:7a7397f45775a4c4b6c4c220711db2b95dd37fdc40b7b7f06d181a220903e467";

const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_EMBED_TIMEOUT_SECS: u64 = 7200;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_EMBED_CPUS: f64 = 4.0;
const DEFAULT_MEMORY: &str = "8Gi";
const DEFAULT_EMBED_MEMORY: &str = "16Gi";
const DEFAULT_PIDS_LIMIT: i64 = 512;
const DEFAULT_SHM_SIZE: &str = "1Gi";
const DEFAULT_EMBED_SHM_SIZE: &str = "2Gi";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workflow {
    QcFilter,
    EmbedCluster,
    Celltypist,
    ObsProjection,
    Subset,
    DenseIngest,
    RankGenesGroups,
    ClusterMeanExpression,
    GeneSetScore,
}

impl Workflow {
    fn kind(self) -> &'static str {
        match self {
            Self::QcFilter => H5AD_QC_FILTER_KIND,
            Self::EmbedCluster => H5AD_PCA_NEIGHBORS_UMAP_LEIDEN_KIND,
            Self::Celltypist => H5AD_CELLTYPIST_ANNOTATE_KIND,
            Self::ObsProjection => H5AD_OBS_TO_PARQUET_KIND,
            Self::Subset => H5AD_SUBSET_BY_OBS_KIND,
            Self::DenseIngest => SC_DENSE_INGEST_KIND,
            Self::RankGenesGroups => H5AD_RANK_GENES_GROUPS_KIND,
            Self::ClusterMeanExpression => H5AD_CLUSTER_MEAN_EXPRESSION_KIND,
            Self::GeneSetScore => H5AD_GENE_SET_SCORE_KIND,
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::QcFilter => "qc_filter",
            Self::EmbedCluster => "pca_neighbors_umap_leiden",
            Self::Celltypist => "celltypist_annotate",
            Self::ObsProjection => "obs_to_parquet",
            Self::Subset => "subset_by_obs",
            Self::DenseIngest => "dense_ingest",
            Self::RankGenesGroups => "rank_genes_groups",
            Self::ClusterMeanExpression => "cluster_mean_expression",
            Self::GeneSetScore => "gene_set_score",
        }
    }

    fn ports(self) -> NodePorts {
        let input_label = if self == Self::DenseIngest {
            "count_matrix"
        } else {
            "h5ad"
        };
        let mut ports = if self == Self::DenseIngest {
            NodePorts::new().add_optional_input_port_of_type(PortType::File)
        } else {
            NodePorts::new().add_input_port_of_type_with_label(None, PortType::File, input_label)
        };
        if self == Self::Celltypist {
            ports = ports.add_optional_input_port_of_type(PortType::File);
        } else if self == Self::Subset {
            ports =
                ports.add_input_port_of_type_with_label(None, PortType::File, "selection_parquet");
        }
        ports = ports.add_output_port_of_type(None, PortType::File);
        if !matches!(self, Self::ObsProjection | Self::ClusterMeanExpression) {
            ports = ports.add_output_port_of_type(None, PortType::File);
        }
        if self == Self::RankGenesGroups {
            ports = ports.add_output_port_of_type(None, PortType::File);
        }
        ports
    }

    fn output_specs(self) -> Vec<ContainerCommandOutputSpec> {
        match self {
            Self::ObsProjection => vec![ContainerCommandOutputSpec {
                path: "cells.parquet".into(),
                format: Some("parquet".into()),
            }],
            Self::Subset | Self::QcFilter | Self::EmbedCluster | Self::Celltypist => vec![
                ContainerCommandOutputSpec {
                    path: "output.h5ad".into(),
                    format: Some("h5ad".into()),
                },
                ContainerCommandOutputSpec {
                    path: "report.json".into(),
                    format: Some("single_cell_workflow_report_json".into()),
                },
            ],
            Self::DenseIngest | Self::GeneSetScore => vec![
                ContainerCommandOutputSpec {
                    path: "output.h5ad".into(),
                    format: Some("h5ad".into()),
                },
                ContainerCommandOutputSpec {
                    path: "report.json".into(),
                    format: Some("single_cell_workflow_report_json".into()),
                },
            ],
            Self::RankGenesGroups => vec![
                ContainerCommandOutputSpec {
                    path: "rank_genes_groups.parquet".into(),
                    format: Some("parquet".into()),
                },
                ContainerCommandOutputSpec {
                    path: "report.json".into(),
                    format: Some("single_cell_workflow_report_json".into()),
                },
                ContainerCommandOutputSpec {
                    path: "output.h5ad".into(),
                    format: Some("h5ad".into()),
                },
            ],
            Self::ClusterMeanExpression => vec![ContainerCommandOutputSpec {
                path: "cluster_mean_expression.parquet".into(),
                format: Some("parquet".into()),
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adQcFilterSpec {
    #[serde(default)]
    pub min_genes: u32,
    #[serde(default)]
    pub max_genes: u32,
    #[serde(default)]
    pub min_cells: u32,
    #[serde(default)]
    pub max_cells: u32,
    #[serde(default = "default_max_percent")]
    pub max_pct_mt: f64,
    #[serde(default = "default_max_percent")]
    pub max_pct_rb: f64,
    #[serde(default = "default_mt_pattern")]
    pub mt_gene_pattern: String,
    #[serde(default = "default_rb_pattern")]
    pub rb_gene_pattern: String,
    #[serde(default = "default_artifact_prefix_qc")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adEmbedClusterSpec {
    #[serde(default = "default_n_pcs")]
    pub n_pcs: u32,
    #[serde(default = "default_n_neighbors")]
    pub n_neighbors: u32,
    #[serde(default = "default_n_top_genes")]
    pub n_top_genes: u32,
    #[serde(default = "default_resolution")]
    pub resolution: f64,
    #[serde(default = "default_min_dist")]
    pub min_dist: f64,
    #[serde(default)]
    pub random_state: i64,
    #[serde(default = "default_true")]
    pub normalize: bool,
    #[serde(default = "default_true")]
    pub log1p: bool,
    #[serde(default)]
    pub subset_hvg: bool,
    #[serde(default)]
    pub scale: bool,
    #[serde(default = "default_artifact_prefix_embed")]
    pub artifact_prefix: String,
    #[serde(default = "default_embed_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adCelltypistSpec {
    #[serde(default)]
    pub majority_voting: bool,
    /// Use the published CellTypist catalog bundle unless `model_path` is set.
    #[serde(default = "default_true")]
    pub use_catalog_model: bool,
    #[serde(default = "default_celltypist_model_bundle")]
    pub model_bundle: String,
    #[serde(default = "default_celltypist_model_file")]
    pub model_file: String,
    /// Explicit model path override. Takes precedence over the catalog bundle.
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default = "default_artifact_prefix_celltypist")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adObsProjectionSpec {
    #[serde(default)]
    pub include_obsm: Vec<String>,
    #[serde(default = "default_artifact_prefix_obs")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adSubsetSpec {
    #[serde(default = "default_join_column")]
    pub join_column: String,
    #[serde(default = "default_artifact_prefix_subset")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DenseOrientation {
    GenesByCells,
    CellsByGenes,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ScDenseIngestSpec {
    /// Optional source path; an upstream File input takes precedence.
    pub path: Option<String>,
    pub orientation: DenseOrientation,
    #[serde(default = "default_auto_delimiter")]
    pub delimiter: String,
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default)]
    pub min_genes: u32,
    #[serde(default)]
    pub min_cells: u32,
    #[serde(default)]
    pub sample_label: Option<String>,
    #[serde(default)]
    pub condition_label: Option<String>,
    #[serde(default = "default_artifact_prefix_dense")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adRankGenesGroupsSpec {
    pub groupby: String,
    #[serde(default = "default_rank_method")]
    pub method: String,
    #[serde(default = "default_reference")]
    pub reference: String,
    #[serde(default = "default_n_genes")]
    pub n_genes: u32,
    #[serde(default = "default_artifact_prefix_rank")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adClusterMeanExpressionSpec {
    pub groupby: String,
    #[serde(default)]
    pub genes: Vec<String>,
    #[serde(default = "default_normalize_cp10k")]
    pub normalize: String,
    #[serde(default)]
    pub include_percent_expressed: bool,
    #[serde(default = "default_artifact_prefix_mean")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct H5adGeneSetScoreSpec {
    pub gene_sets: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_ctrl_size")]
    pub ctrl_size: u32,
    #[serde(default)]
    pub random_state: i64,
    #[serde(default = "default_artifact_prefix_score")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

fn default_max_percent() -> f64 {
    100.0
}
fn default_mt_pattern() -> String {
    "^MT-".into()
}
fn default_rb_pattern() -> String {
    "^RPL|^RPS".into()
}
fn default_n_pcs() -> u32 {
    30
}
fn default_n_neighbors() -> u32 {
    15
}
fn default_n_top_genes() -> u32 {
    2000
}
fn default_resolution() -> f64 {
    0.5
}
fn default_min_dist() -> f64 {
    0.5
}
fn default_true() -> bool {
    true
}
fn default_join_column() -> String {
    "cell_id".into()
}
fn default_artifact_prefix_qc() -> String {
    format!("/artifacts/{H5AD_QC_FILTER_KIND}")
}
fn default_artifact_prefix_embed() -> String {
    format!("/artifacts/{H5AD_PCA_NEIGHBORS_UMAP_LEIDEN_KIND}")
}
fn default_artifact_prefix_celltypist() -> String {
    format!("/artifacts/{H5AD_CELLTYPIST_ANNOTATE_KIND}")
}
fn default_celltypist_model_bundle() -> String {
    CELLTYPIST_MODEL_BUNDLE.into()
}
fn default_celltypist_model_file() -> String {
    DEFAULT_CELLTYPIST_MODEL_FILE.into()
}
fn default_artifact_prefix_obs() -> String {
    format!("/artifacts/{H5AD_OBS_TO_PARQUET_KIND}")
}
fn default_artifact_prefix_subset() -> String {
    format!("/artifacts/{H5AD_SUBSET_BY_OBS_KIND}")
}
fn default_artifact_prefix_dense() -> String {
    format!("/artifacts/{SC_DENSE_INGEST_KIND}")
}
fn default_artifact_prefix_rank() -> String {
    format!("/artifacts/{H5AD_RANK_GENES_GROUPS_KIND}")
}
fn default_artifact_prefix_mean() -> String {
    format!("/artifacts/{H5AD_CLUSTER_MEAN_EXPRESSION_KIND}")
}
fn default_artifact_prefix_score() -> String {
    format!("/artifacts/{H5AD_GENE_SET_SCORE_KIND}")
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}
fn default_embed_timeout() -> u64 {
    DEFAULT_EMBED_TIMEOUT_SECS
}
fn default_auto_delimiter() -> String {
    "auto".into()
}
fn default_rank_method() -> String {
    "wilcoxon".into()
}
fn default_reference() -> String {
    "rest".into()
}
fn default_n_genes() -> u32 {
    100
}
fn default_normalize_cp10k() -> String {
    "cp10k".into()
}
fn default_ctrl_size() -> u32 {
    50
}

pub struct SingleCellH5adContainerNode {
    kind: &'static str,
    workflow: Workflow,
    ports: NodePorts,
    fallback_input: Option<String>,
    fallback_model: Option<String>,
    inner: Box<dyn DagNode>,
}

impl Clone for SingleCellH5adContainerNode {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            workflow: self.workflow,
            ports: self.ports.clone(),
            fallback_input: self.fallback_input.clone(),
            fallback_model: self.fallback_model.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for SingleCellH5adContainerNode {
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
        let fallback_inputs;
        let inputs = if self.workflow == Workflow::DenseIngest
            && inputs.is_empty()
            && let Some(path) = self.fallback_input.clone()
        {
            let mut source = FileReferenceNode::new(path, None);
            let outputs = source.execute(ctx, &[], reporter).await?;
            let file = outputs.get(&0).ok_or_else(|| {
                DagError::Schedule("sc_dense_ingest path resolution produced no File".into())
            })?;
            fallback_inputs = vec![NodeInput::file(0, file.as_file()?.clone())];
            &fallback_inputs
        } else if self.workflow == Workflow::Celltypist
            && !inputs.iter().any(|input| input.port == 1)
            && let Some(path) = self.fallback_model.clone()
        {
            let mut source = FileReferenceNode::new(path, Some("pkl".into()));
            let outputs = source.execute(ctx, &[], reporter).await?;
            let file = outputs.get(&0).ok_or_else(|| {
                DagError::Schedule("CellTypist model resolution produced no File".into())
            })?;
            let mut resolved = inputs.to_vec();
            resolved.push(NodeInput::file(1, file.as_file()?.clone()));
            fallback_inputs = resolved;
            &fallback_inputs
        } else {
            inputs
        };
        self.inner.execute(ctx, inputs, reporter).await
    }
}

pub struct SingleCellH5adContainerNodeFactory {
    workflow: Workflow,
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl SingleCellH5adContainerNodeFactory {
    pub fn qc_filter(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(Workflow::QcFilter, runtime, panel_cache)
    }

    pub fn embed_cluster(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(Workflow::EmbedCluster, runtime, panel_cache)
    }

    pub fn celltypist(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(Workflow::Celltypist, runtime, panel_cache)
    }

    pub fn obs_projection(
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(Workflow::ObsProjection, runtime, panel_cache)
    }

    pub fn subset(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(Workflow::Subset, runtime, panel_cache)
    }

    pub fn dense_ingest(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(Workflow::DenseIngest, runtime, panel_cache)
    }

    pub fn rank_genes_groups(
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(Workflow::RankGenesGroups, runtime, panel_cache)
    }

    pub fn cluster_mean_expression(
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(Workflow::ClusterMeanExpression, runtime, panel_cache)
    }

    pub fn gene_set_score(
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self::new(Workflow::GeneSetScore, runtime, panel_cache)
    }

    fn new(
        workflow: Workflow,
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self {
            workflow,
            runtime,
            panel_cache,
        }
    }
}

fn validate_resource(
    artifact_prefix: &str,
    timeout_secs: u64,
    cpus: Option<f64>,
    memory: Option<&str>,
    pids_limit: Option<i64>,
) -> Result<(), String> {
    if !artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if let Some(cpus) = cpus
        && (!cpus.is_finite() || cpus <= 0.0)
    {
        return Err("cpus must be finite and greater than zero".into());
    }
    if let Some(limit) = pids_limit
        && limit <= 0
    {
        return Err("pids_limit must be greater than zero".into());
    }
    if let Some(memory) = memory
        && memory.trim().is_empty()
    {
        return Err("memory cannot be empty".into());
    }
    Ok(())
}

fn validate_bundle_relative_path(path: &str, field: &str) -> Result<(), String> {
    if path.trim().is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(format!("{field} must be a safe relative bundle path"));
    }
    Ok(())
}

fn celltypist_model_binding(spec: &H5adCelltypistSpec) -> Option<DataBundleBinding> {
    if !spec.use_catalog_model || spec.model_path.is_some() {
        None
    } else {
        Some(DataBundleBinding::new("model", spec.model_bundle.clone()))
    }
}

pub fn validate(workflow: Workflow, spec: &serde_json::Value) -> Result<(), String> {
    match workflow {
        Workflow::QcFilter => {
            let spec: H5adQcFilterSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            for (value, name) in [
                (spec.max_pct_mt, "max_pct_mt"),
                (spec.max_pct_rb, "max_pct_rb"),
            ] {
                if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                    return Err(format!("{name} must be between 0 and 100"));
                }
            }
            for (value, name) in [
                (spec.mt_gene_pattern, "mt_gene_pattern"),
                (spec.rb_gene_pattern, "rb_gene_pattern"),
            ] {
                if value.trim().is_empty() {
                    return Err(format!("{name} cannot be empty"));
                }
            }
        }
        Workflow::EmbedCluster => {
            let spec: H5adEmbedClusterSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.n_pcs == 0
                || spec.n_neighbors <= 1
                || spec.n_top_genes == 0
                || !spec.resolution.is_finite()
                || spec.resolution <= 0.0
                || !spec.min_dist.is_finite()
                || !(0.0..1.0).contains(&spec.min_dist)
            {
                return Err("embedding dimensions, neighbors, genes, resolution, and min_dist must be positive".into());
            }
        }
        Workflow::Celltypist => {
            let spec: H5adCelltypistSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.use_catalog_model {
                if spec.model_bundle.trim().is_empty() {
                    return Err("model_bundle cannot be empty".into());
                }
                validate_bundle_relative_path(&spec.model_file, "model_file")?;
            }
            if spec
                .model_path
                .as_deref()
                .is_some_and(|path| path.trim().is_empty())
            {
                return Err("model_path cannot be empty when provided".into());
            }
        }
        Workflow::ObsProjection => {
            let spec: H5adObsProjectionSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.include_obsm.iter().any(|key| key.trim().is_empty()) {
                return Err("include_obsm entries cannot be empty".into());
            }
            let mut keys = spec.include_obsm.clone();
            keys.sort_unstable();
            keys.dedup();
            if keys.len() != spec.include_obsm.len() {
                return Err("include_obsm entries must be unique".into());
            }
        }
        Workflow::Subset => {
            let spec: H5adSubsetSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.join_column.trim().is_empty() {
                return Err("join_column cannot be empty".into());
            }
        }
        Workflow::DenseIngest => {
            let spec: ScDenseIngestSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.delimiter.trim().is_empty() {
                return Err("delimiter cannot be empty".into());
            }
            if spec
                .path
                .as_deref()
                .is_some_and(|path| path.trim().is_empty())
            {
                return Err("path cannot be empty when provided".into());
            }
        }
        Workflow::RankGenesGroups => {
            let spec: H5adRankGenesGroupsSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.groupby.trim().is_empty() {
                return Err("groupby cannot be empty".into());
            }
            if !matches!(
                spec.method.as_str(),
                "wilcoxon" | "t-test" | "t-test_overestim_var" | "logreg"
            ) {
                return Err(format!(
                    "unsupported rank_genes_groups method `{}`",
                    spec.method
                ));
            }
            if spec.reference.trim().is_empty() || spec.n_genes == 0 {
                return Err("reference must be nonempty and n_genes must be positive".into());
            }
        }
        Workflow::ClusterMeanExpression => {
            let spec: H5adClusterMeanExpressionSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.groupby.trim().is_empty() {
                return Err("groupby cannot be empty".into());
            }
            if !matches!(spec.normalize.as_str(), "cp10k" | "none") {
                return Err(format!(
                    "unsupported normalize mode `{}`; expected cp10k or none",
                    spec.normalize
                ));
            }
        }
        Workflow::GeneSetScore => {
            let spec: H5adGeneSetScoreSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            validate_resource(
                &spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory.as_deref(),
                spec.pids_limit,
            )?;
            if spec.gene_sets.is_empty() {
                return Err("gene_sets cannot be empty".into());
            }
            if spec.ctrl_size == 0 {
                return Err("ctrl_size must be greater than zero".into());
            }
        }
    }
    Ok(())
}

pub fn container_spec(
    workflow: Workflow,
    spec: &serde_json::Value,
) -> Result<ContainerCommandSpec, String> {
    validate(workflow, spec)?;
    let (artifact_prefix, timeout_secs, cpus, memory, pids_limit, shm_size) = match workflow {
        Workflow::EmbedCluster => {
            let spec: H5adEmbedClusterSpec =
                serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
            (
                spec.artifact_prefix,
                spec.timeout_secs,
                spec.cpus,
                spec.memory,
                spec.pids_limit,
                DEFAULT_EMBED_SHM_SIZE.to_string(),
            )
        }
        _ => (
            string_field(spec, "artifact_prefix")
                .unwrap_or_else(|| default_artifact_prefix(workflow)),
            spec.get("timeout_secs")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
            spec.get("cpus")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite()),
            string_field(spec, "memory"),
            spec.get("pids_limit").and_then(serde_json::Value::as_i64),
            DEFAULT_SHM_SIZE.to_string(),
        ),
    };
    let env = BTreeMap::from([
        (
            "AUTONOMICS_SINGLE_CELL_WORKFLOW".to_string(),
            workflow.operation().to_string(),
        ),
        (
            "AUTONOMICS_SINGLE_CELL_PARAMS".to_string(),
            "/work/.autonomics/files/params.json".to_string(),
        ),
    ]);
    Ok(ContainerCommandSpec {
        image: acr_image(
            SINGLE_CELL_WORKFLOW_IMAGE_REPOSITORY,
            SINGLE_CELL_WORKFLOW_IMAGE_DIGEST,
        )?,
        command: vec!["python".into()],
        script: Some(
            include_str!("../../../../containers/single-cell-preprocessor/workflow.py").to_string(),
        ),
        files: BTreeMap::from([("params.json".into(), spec.to_string())]),
        env,
        outputs: workflow.output_specs(),
        workdir: None,
        artifact_prefix,
        timeout_secs,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: Some(cpus.unwrap_or(if workflow == Workflow::EmbedCluster {
            DEFAULT_EMBED_CPUS
        } else {
            DEFAULT_CPUS
        })),
        memory: Some(memory.unwrap_or_else(|| {
            if workflow == Workflow::EmbedCluster {
                DEFAULT_EMBED_MEMORY.into()
            } else {
                DEFAULT_MEMORY.into()
            }
        })),
        pids_limit: Some(pids_limit.unwrap_or(DEFAULT_PIDS_LIMIT)),
        shm_size: Some(shm_size),
        user: None,
    })
}

fn default_artifact_prefix(workflow: Workflow) -> String {
    match workflow {
        Workflow::QcFilter => default_artifact_prefix_qc(),
        Workflow::EmbedCluster => default_artifact_prefix_embed(),
        Workflow::Celltypist => default_artifact_prefix_celltypist(),
        Workflow::ObsProjection => default_artifact_prefix_obs(),
        Workflow::Subset => default_artifact_prefix_subset(),
        Workflow::DenseIngest => default_artifact_prefix_dense(),
        Workflow::RankGenesGroups => default_artifact_prefix_rank(),
        Workflow::ClusterMeanExpression => default_artifact_prefix_mean(),
        Workflow::GeneSetScore => default_artifact_prefix_score(),
    }
}

fn string_field(spec: &serde_json::Value, name: &str) -> Option<String> {
    spec.get(name)?.as_str().map(str::to_string)
}

impl NodeFactory for SingleCellH5adContainerNodeFactory {
    fn kind(&self) -> &'static str {
        self.workflow.kind()
    }

    fn desc(&self) -> &'static str {
        match self.workflow {
            Workflow::QcFilter => "Filters cells and genes in an H5AD and emits a QC report.",
            Workflow::EmbedCluster => {
                "Runs normalization/HVG/PCA plus neighbors, UMAP, and Leiden on an H5AD."
            }
            Workflow::Celltypist => {
                "Annotates an H5AD with the catalog-backed or an explicit CellTypist model."
            }
            Workflow::ObsProjection => "Projects H5AD obs and selected obsm keys to Parquet.",
            Workflow::Subset => "Subsets an H5AD by cell IDs read from a Parquet sidecar.",
            Workflow::DenseIngest => {
                "Ingests a dense CSV/TSV gene-by-cell or cell-by-gene count matrix into H5AD."
            }
            Workflow::RankGenesGroups => {
                "Runs Scanpy rank_genes_groups and emits a tidy Parquet marker table."
            }
            Workflow::ClusterMeanExpression => {
                "Exports cluster-by-gene mean expression and optionally expression fractions."
            }
            Workflow::GeneSetScore => "Scores per-cell gene sets with Scanpy score_genes.",
        }
    }

    fn doc(&self) -> &'static str {
        match self.workflow {
            Workflow::QcFilter => {
                "Input port 0 is H5AD. Output ports are output.h5ad then report.json. \
                The matrix remains opaque to Rust and is processed only in the pinned Python image."
            }
            Workflow::EmbedCluster => {
                "Input port 0 is H5AD. Output ports are output.h5ad then report.json. \
                If X_pca is absent, normalization, HVG selection, and PCA run before neighbors, UMAP, and Leiden."
            }
            Workflow::Celltypist => {
                "Input port 0 is H5AD. Input port 1 is an optional CellTypist model File; \
                when omitted, the published `celltypist.models.pan_immune` bundle is used. \
                Output ports are output.h5ad then report.json. Network access is disabled."
            }
            Workflow::ObsProjection => {
                "Input port 0 is H5AD and the single output is cells.parquet. The first \
                column is cell_id derived from obs_names; include_obsm projects explicit embeddings."
            }
            Workflow::Subset => {
                "Input ports are H5AD then selection Parquet. Output ports are output.h5ad \
                then report.json. By default the Parquet cell_id column selects obs_names."
            }
            Workflow::DenseIngest => {
                "Input port 0 is a CSV/TSV(.gz) count matrix, or set `path` directly. \
                Output ports are output.h5ad then report.json. genes_by_cells expects \
                genes in rows; cells_by_genes is transposed automatically."
            }
            Workflow::RankGenesGroups => {
                "Input port 0 is H5AD. Outputs are rank_genes_groups.parquet, report.json, \
                and output.h5ad with the Scanpy result stored in uns."
            }
            Workflow::ClusterMeanExpression => {
                "Input port 0 is H5AD. The single output is a long Parquet table with \
                cluster, gene, mean_expression, and optionally pct_expressed columns."
            }
            Workflow::GeneSetScore => {
                "Input port 0 is H5AD. Output ports are output.h5ad then report.json; each \
                score is added to obs using the configured gene-set key."
            }
        }
    }

    fn spec_schema(&self) -> schemars::Schema {
        match self.workflow {
            Workflow::QcFilter => schema_for!(H5adQcFilterSpec),
            Workflow::EmbedCluster => schema_for!(H5adEmbedClusterSpec),
            Workflow::Celltypist => schema_for!(H5adCelltypistSpec),
            Workflow::ObsProjection => schema_for!(H5adObsProjectionSpec),
            Workflow::Subset => schema_for!(H5adSubsetSpec),
            Workflow::DenseIngest => schema_for!(ScDenseIngestSpec),
            Workflow::RankGenesGroups => schema_for!(H5adRankGenesGroupsSpec),
            Workflow::ClusterMeanExpression => schema_for!(H5adClusterMeanExpressionSpec),
            Workflow::GeneSetScore => schema_for!(H5adGeneSetScoreSpec),
        }
    }

    fn ports(&self) -> NodePorts {
        self.workflow.ports()
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        if self.workflow == Workflow::Celltypist {
            vec![DataBundleBinding::new("model", CELLTYPIST_MODEL_BUNDLE)]
        } else {
            Vec::new()
        }
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let fallback_input = spec
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let fallback_model = if self.workflow == Workflow::Celltypist {
            let parsed: H5adCelltypistSpec = serde_json::from_value(spec.clone())?;
            if let Some(path) = parsed.model_path {
                Some(path)
            } else if celltypist_model_binding(&parsed).is_some() {
                let bundle = node_ctx.bound_data_bundle("model")?;
                Some(format!(
                    "{}/{}",
                    bundle.vpath.trim_end_matches('/'),
                    parsed.model_file.trim_start_matches('/')
                ))
            } else {
                None
            }
        } else {
            None
        };
        let spec = container_spec(self.workflow, &spec)
            .map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(SingleCellH5adContainerNode {
            kind: self.workflow.kind(),
            workflow: self.workflow,
            ports: self.workflow.ports(),
            fallback_input,
            fallback_model,
            inner: Box::new(node),
        }))
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        if self.workflow != Workflow::Celltypist {
            return Ok(Vec::new());
        }
        let spec: H5adCelltypistSpec = serde_json::from_value(spec)?;
        Ok(celltypist_model_binding(&spec).into_iter().collect())
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        validate(self.workflow, &spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(self.workflow.ports())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use container_runtime::{PanelCache, PodmanRuntime};
    use dag_core::node::{BundleRegistry, DataBundle};
    use dag_core::registry::NodeRegistry;

    fn resource_json() -> serde_json::Value {
        serde_json::json!({
            "artifact_prefix": "/artifacts/single-cell-test",
            "timeout_secs": 60
        })
    }

    #[test]
    fn qc_contract_is_h5ad_report_and_isolated() {
        let spec = resource_json();
        let container = container_spec(Workflow::QcFilter, &spec).unwrap();
        assert_eq!(container.command.as_slice(), ["python"]);
        assert!(
            container
                .script
                .as_deref()
                .is_some_and(|script| script.contains("def qc_filter"))
        );
        assert_eq!(
            container.env["AUTONOMICS_SINGLE_CELL_WORKFLOW"],
            "qc_filter"
        );
        assert_eq!(container.outputs[0].path, "output.h5ad");
        assert_eq!(container.outputs[1].path, "report.json");
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
    }

    #[test]
    fn projection_has_a_single_parquet_output() {
        let container = container_spec(Workflow::ObsProjection, &resource_json()).unwrap();
        assert_eq!(container.outputs.len(), 1);
        assert_eq!(container.outputs[0].path, "cells.parquet");
    }

    #[test]
    fn omitted_artifact_prefix_stays_node_specific() {
        let container = container_spec(Workflow::Subset, &serde_json::json!({})).unwrap();
        assert_eq!(container.artifact_prefix, "/artifacts/h5ad_subset_by_obs");
    }

    #[test]
    fn celltypist_defaults_to_published_model_bundle() {
        let spec: H5adCelltypistSpec = serde_json::from_value(serde_json::json!({})).unwrap();
        let binding = celltypist_model_binding(&spec).unwrap();
        assert_eq!(binding.binding, "model");
        assert_eq!(binding.bundle_id, CELLTYPIST_MODEL_BUNDLE);
        assert_eq!(spec.model_file, DEFAULT_CELLTYPIST_MODEL_FILE);
        assert!(!Workflow::Celltypist.ports().input_port(1).unwrap().required);

        let explicit: H5adCelltypistSpec = serde_json::from_value(serde_json::json!({
            "model_path": "/bundles/custom/model.pkl"
        }))
        .unwrap();
        assert!(celltypist_model_binding(&explicit).is_none());
    }

    #[test]
    fn celltypist_registry_resolves_default_model_path() {
        let catalog = Arc::new(
            BundleRegistry::from_bundles([DataBundle::new(
                CELLTYPIST_MODEL_BUNDLE,
                "test CellTypist models",
                "/bundles/celltypist.models.pan_immune",
            )])
            .unwrap(),
        );
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
        .with_bundle_registry(catalog);
        let mut registry = NodeRegistry::new(ctx);
        let cache = Arc::new(PanelCache::new(tempfile::tempdir().unwrap().path()));
        registry.register(Box::new(SingleCellH5adContainerNodeFactory::celltypist(
            Arc::new(PodmanRuntime::default()),
            cache,
        )));
        let node = registry
            .build_node(H5AD_CELLTYPIST_ANNOTATE_KIND, serde_json::json!({}))
            .unwrap();
        let node = node
            .as_any()
            .downcast_ref::<SingleCellH5adContainerNode>()
            .unwrap();
        assert_eq!(
            node.fallback_model.as_deref(),
            Some("/bundles/celltypist.models.pan_immune/Immune_All_Low.pkl")
        );
    }

    #[test]
    fn dense_ingest_declares_h5ad_report_contract() {
        let container = container_spec(
            Workflow::DenseIngest,
            &serde_json::json!({"orientation": "genes_by_cells"}),
        )
        .unwrap();
        assert_eq!(
            container.env["AUTONOMICS_SINGLE_CELL_WORKFLOW"],
            "dense_ingest"
        );
        assert_eq!(container.outputs[0].path, "output.h5ad");
        assert_eq!(container.outputs[1].path, "report.json");
        assert!(container.network == "isolated" && container.read_only_rootfs);
    }

    #[test]
    fn marker_and_mean_workflows_emit_parquet() {
        let rank = container_spec(
            Workflow::RankGenesGroups,
            &serde_json::json!({"groupby": "leiden"}),
        )
        .unwrap();
        assert_eq!(rank.outputs[0].path, "rank_genes_groups.parquet");
        assert_eq!(rank.outputs[2].path, "output.h5ad");

        let mean = container_spec(
            Workflow::ClusterMeanExpression,
            &serde_json::json!({"groupby": "leiden", "genes": ["MS4A1"]}),
        )
        .unwrap();
        assert_eq!(mean.outputs[0].path, "cluster_mean_expression.parquet");
        assert_eq!(mean.outputs.len(), 1);
    }

    #[test]
    fn invalid_embedding_parameters_are_rejected() {
        let mut spec = resource_json();
        spec["n_neighbors"] = serde_json::json!(1);
        assert!(validate(Workflow::EmbedCluster, &spec).is_err());
    }
}
