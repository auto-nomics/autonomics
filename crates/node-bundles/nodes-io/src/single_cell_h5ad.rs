//! Containerized H5AD-first single-cell workflow nodes.
//!
//! Each node keeps expression data behind a File port. The generic container
//! runtime stages the H5AD and publishes immutable outputs; only the Python
//! scverse process interprets the matrix payload.

use std::collections::BTreeMap;
use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const H5AD_QC_FILTER_KIND: &str = "h5ad_qc_filter";
pub const H5AD_PCA_NEIGHBORS_UMAP_LEIDEN_KIND: &str = "h5ad_pca_neighbors_umap_leiden";
pub const H5AD_CELLTYPIST_ANNOTATE_KIND: &str = "h5ad_celltypist_annotate";
pub const H5AD_OBS_TO_PARQUET_KIND: &str = "h5ad_obs_to_parquet";
pub const H5AD_SUBSET_BY_OBS_KIND: &str = "h5ad_subset_by_obs";
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
}

impl Workflow {
    fn kind(self) -> &'static str {
        match self {
            Self::QcFilter => H5AD_QC_FILTER_KIND,
            Self::EmbedCluster => H5AD_PCA_NEIGHBORS_UMAP_LEIDEN_KIND,
            Self::Celltypist => H5AD_CELLTYPIST_ANNOTATE_KIND,
            Self::ObsProjection => H5AD_OBS_TO_PARQUET_KIND,
            Self::Subset => H5AD_SUBSET_BY_OBS_KIND,
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::QcFilter => "qc_filter",
            Self::EmbedCluster => "pca_neighbors_umap_leiden",
            Self::Celltypist => "celltypist_annotate",
            Self::ObsProjection => "obs_to_parquet",
            Self::Subset => "subset_by_obs",
        }
    }

    fn ports(self) -> NodePorts {
        let mut ports =
            NodePorts::new().add_input_port_of_type_with_label(None, PortType::File, "h5ad");
        if matches!(self, Self::Celltypist | Self::Subset) {
            let label = if self == Self::Celltypist {
                "model"
            } else {
                "selection_parquet"
            };
            ports = ports.add_input_port_of_type_with_label(None, PortType::File, label);
        }
        ports = ports.add_output_port_of_type(None, PortType::File);
        if self != Self::ObsProjection {
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
fn default_artifact_prefix_obs() -> String {
    format!("/artifacts/{H5AD_OBS_TO_PARQUET_KIND}")
}
fn default_artifact_prefix_subset() -> String {
    format!("/artifacts/{H5AD_SUBSET_BY_OBS_KIND}")
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}
fn default_embed_timeout() -> u64 {
    DEFAULT_EMBED_TIMEOUT_SECS
}

pub struct SingleCellH5adContainerNode {
    kind: &'static str,
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for SingleCellH5adContainerNode {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            ports: self.ports.clone(),
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
            Workflow::Celltypist => "Annotates an H5AD with a local CellTypist model file.",
            Workflow::ObsProjection => "Projects H5AD obs and selected obsm keys to Parquet.",
            Workflow::Subset => "Subsets an H5AD by cell IDs read from a Parquet sidecar.",
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
                "Input ports are H5AD then a local CellTypist model File. Output ports \
                are output.h5ad then report.json. Network access is disabled."
            }
            Workflow::ObsProjection => {
                "Input port 0 is H5AD and the single output is cells.parquet. The first \
                column is cell_id derived from obs_names; include_obsm projects explicit embeddings."
            }
            Workflow::Subset => {
                "Input ports are H5AD then selection Parquet. Output ports are output.h5ad \
                then report.json. By default the Parquet cell_id column selects obs_names."
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
        }
    }

    fn ports(&self) -> NodePorts {
        self.workflow.ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
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
            ports: self.workflow.ports(),
            inner: Box::new(node),
        }))
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
    fn invalid_embedding_parameters_are_rejected() {
        let mut spec = resource_json();
        spec["n_neighbors"] = serde_json::json!(1);
        assert!(validate(Workflow::EmbedCluster, &spec).is_err());
    }
}
