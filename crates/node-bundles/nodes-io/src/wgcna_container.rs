//! Containerized WGCNA pipeline wrapper.
//!
//! Single-file (or two-file with optional metadata) contract: an expression
//! matrix with `gene_id` followed by sample columns. The wrapper delegates the
//! soft-threshold scan, signed adjacency, TOM, dynamic tree cut, module merge
//! and eigengene computation to the pinned official WGCNA image so memory
//! pressure never escapes the container.

use std::collections::BTreeMap;
use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::registry_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const WGCNA_CONTAINER_KIND: &str = "wgcna_container";
pub const WGCNA_IMAGE_REPOSITORY: &str = "bulk-rnaseq";
pub const WGCNA_IMAGE_DIGEST: &str =
    "sha256:c63fb113760b97bad1b1761d1daf4ad3241aa32e054ab20dd3a6bced4a552a6d";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/wgcna_container";
const DEFAULT_TIMEOUT_SECS: u64 = 7200;
const DEFAULT_CPUS: f64 = 4.0;
const DEFAULT_MEMORY: &str = "12Gi";
const DEFAULT_PIDS_LIMIT: i64 = 256;
const DEFAULT_SHM_SIZE: &str = "2Gi";

fn default_network_type() -> String {
    "signed".into()
}
fn default_cor() -> String {
    "pearson".into()
}
fn default_min_module_size() -> u32 {
    30
}
fn default_deep_split() -> u8 {
    2
}
fn default_merge_threshold() -> f64 {
    0.25
}
fn default_max_block_size() -> u32 {
    5000
}
fn default_threads() -> u8 {
    1
}
fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}
fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WgcnaContainerSpec {
    #[serde(default = "default_network_type")]
    pub network_type: String,
    #[serde(default = "default_cor")]
    pub cor: String,
    #[serde(default = "default_min_module_size")]
    pub min_module_size: u32,
    #[serde(default = "default_deep_split")]
    pub deep_split: u8,
    #[serde(default = "default_merge_threshold")]
    pub merge_threshold: f64,
    #[serde(default = "default_max_block_size")]
    pub max_block_size: u32,
    #[serde(default = "default_threads")]
    pub threads: u8,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

pub struct WgcnaContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl WgcnaContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct WgcnaContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for WgcnaContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for WgcnaContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        WGCNA_CONTAINER_KIND
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

pub fn validate(spec: &WgcnaContainerSpec) -> Result<(), String> {
    if !["signed", "unsigned"].contains(&spec.network_type.as_str()) {
        return Err("network_type must be signed or unsigned".into());
    }
    if !["pearson", "bicor"].contains(&spec.cor.as_str()) {
        return Err("cor must be pearson or bicor".into());
    }
    if spec.min_module_size < 5 {
        return Err("min_module_size must be at least 5".into());
    }
    if spec.deep_split > 4 {
        return Err("deep_split must be at most 4".into());
    }
    if spec.merge_threshold <= 0.0
        || spec.merge_threshold >= 1.0
        || !spec.merge_threshold.is_finite()
    {
        return Err("merge_threshold must lie strictly between 0 and 1".into());
    }
    if spec.max_block_size < 500 {
        return Err("max_block_size must be at least 500".into());
    }
    if spec.threads == 0 {
        return Err("threads must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

pub fn container_spec(spec: &WgcnaContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_WGCNA_NETWORK_TYPE".to_string(),
            spec.network_type.clone(),
        ),
        ("AUTONOMICS_WGCNA_COR".to_string(), spec.cor.clone()),
        (
            "AUTONOMICS_WGCNA_MIN_MODULE_SIZE".to_string(),
            spec.min_module_size.to_string(),
        ),
        (
            "AUTONOMICS_WGCNA_DEEP_SPLIT".to_string(),
            spec.deep_split.to_string(),
        ),
        (
            "AUTONOMICS_WGCNA_MERGE_THRESHOLD".to_string(),
            spec.merge_threshold.to_string(),
        ),
        (
            "AUTONOMICS_WGCNA_MAX_BLOCK_SIZE".to_string(),
            spec.max_block_size.to_string(),
        ),
        (
            "AUTONOMICS_WGCNA_THREADS".to_string(),
            spec.threads.to_string(),
        ),
    ]);
    Ok(ContainerCommandSpec {
        image: registry_image(WGCNA_IMAGE_REPOSITORY, WGCNA_IMAGE_DIGEST)?,
        command: vec![
            "Rscript".to_string(),
            "--vanilla".to_string(),
            "/opt/autonomics/wgcna_runner.R".to_string(),
        ],
        script: None,
        env,
        files: BTreeMap::new(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "soft_threshold.tsv".into(),
                format: Some("wgcna_soft_threshold_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "adjacency_stats.tsv".into(),
                format: Some("wgcna_adjacency_stats_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "tom_stats.tsv".into(),
                format: Some("wgcna_tom_stats_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "modules.tsv".into(),
                format: Some("wgcna_modules_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "module_eigengenes.tsv".into(),
                format: Some("wgcna_eigengenes_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "run_report.json".into(),
                format: Some("wgcna_run_report_json".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: Some(DEFAULT_CPUS),
        memory: Some(DEFAULT_MEMORY.into()),
        pids_limit: Some(DEFAULT_PIDS_LIMIT),
        shm_size: Some(DEFAULT_SHM_SIZE.into()),
        gpus: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "expression_matrix")
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for WgcnaContainerNodeFactory {
    fn kind(&self) -> &'static str {
        WGCNA_CONTAINER_KIND
    }
    fn desc(&self) -> &'static str {
        "Runs the official WGCNA pipeline in an isolated container."
    }
    fn doc(&self) -> &'static str {
        "Input port 0 is a gene-by-sample expression matrix (genes as rows, samples as columns, gene_id as the first column). Optional input port 1 carries sample metadata. The node performs the soft-threshold scan, builds the signed adjacency / TOM via WGCNA::blockwiseModules, applies dynamic tree cut + merge, computes kME per gene, and emits module assignments, module eigengenes, soft-threshold statistics and a run report."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(WgcnaContainerSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: WgcnaContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            self.kind(),
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(WgcnaContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }
    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: WgcnaContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> WgcnaContainerSpec {
        WgcnaContainerSpec {
            network_type: default_network_type(),
            cor: default_cor(),
            min_module_size: default_min_module_size(),
            deep_split: default_deep_split(),
            merge_threshold: default_merge_threshold(),
            max_block_size: default_max_block_size(),
            threads: default_threads(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }
    #[test]
    fn builds_official_wgcna_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            registry_image(WGCNA_IMAGE_REPOSITORY, WGCNA_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.command[2], "/opt/autonomics/wgcna_runner.R");
        assert!(container.read_only_rootfs);
        assert_eq!(container.network, "isolated");
        assert_eq!(container.outputs.len(), 6);
        assert_eq!(container.env["AUTONOMICS_WGCNA_NETWORK_TYPE"], "signed");
    }
    #[test]
    fn rejects_invalid_cor_and_network_type() {
        let mut v = spec();
        v.network_type = "hybrid".into();
        assert!(validate(&v).is_err());
        v.network_type = "signed".into();
        v.cor = "kendall".into();
        assert!(validate(&v).is_err());
    }
    #[test]
    fn rejects_non_positive_merge_threshold() {
        let mut v = spec();
        v.merge_threshold = 0.0;
        assert!(validate(&v).is_err());
        v.merge_threshold = 1.0;
        assert!(validate(&v).is_err());
    }
}
