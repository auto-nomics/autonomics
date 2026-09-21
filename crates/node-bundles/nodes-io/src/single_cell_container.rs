//! Containerized Scanpy single-cell preprocessing node.
//!
//! The four biological inputs remain ordinary File artifacts and are staged by
//! the generic container runtime. The wrapper owns only the file contract and
//! pinned Scanpy invocation; expression data never enters the image.

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

pub const SINGLE_CELL_PREPROCESSOR_CONTAINER_KIND: &str = "single_cell_preprocessor_container";
pub const SINGLE_CELL_PREPROCESSOR_IMAGE_REPOSITORY: &str = "single-cell-preprocessor";
pub const SINGLE_CELL_PREPROCESSOR_IMAGE_DIGEST: &str =
    "sha256:c34c26428d13804c2528bc734805c1ccfe909353604ac08da0ce9c68890e7e18";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/single_cell_preprocessor_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "8Gi";
const DEFAULT_PIDS_LIMIT: i64 = 512;
const DEFAULT_SHM_SIZE: &str = "1Gi";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SingleCellOperation {
    /// Read the MatrixMarket header and validate the companion tables without
    /// loading expression values.
    Inspect,
    /// Load expression values with Scanpy and write a full H5AD.
    Ingest,
}

impl SingleCellOperation {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Ingest => "ingest",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SingleCellPreprocessorContainerSpec {
    #[serde(default = "default_operation")]
    pub operation: SingleCellOperation,
    #[serde(default)]
    pub min_genes: u32,
    #[serde(default)]
    pub min_cells: u32,
    #[serde(default)]
    pub normalize_total: bool,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

fn default_operation() -> SingleCellOperation {
    SingleCellOperation::Inspect
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct SingleCellPreprocessorContainerNodeFactory {
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl SingleCellPreprocessorContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct SingleCellPreprocessorContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for SingleCellPreprocessorContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for SingleCellPreprocessorContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        SINGLE_CELL_PREPROCESSOR_CONTAINER_KIND
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

pub fn validate(spec: &SingleCellPreprocessorContainerSpec) -> Result<(), String> {
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if let Some(cpus) = spec.cpus
        && (!cpus.is_finite() || cpus <= 0.0)
    {
        return Err("cpus must be finite and greater than zero".into());
    }
    if let Some(limit) = spec.pids_limit
        && limit <= 0
    {
        return Err("pids_limit must be greater than zero".into());
    }
    if let Some(memory) = &spec.memory
        && memory.trim().is_empty()
    {
        return Err("memory cannot be empty".into());
    }
    if spec.operation == SingleCellOperation::Inspect
        && (spec.min_genes > 0 || spec.min_cells > 0 || spec.normalize_total)
    {
        return Err(
            "filtering and normalization require operation `ingest`; `inspect` is header-only"
                .into(),
        );
    }
    Ok(())
}

pub fn container_spec(
    spec: &SingleCellPreprocessorContainerSpec,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_SINGLE_CELL_OPERATION".into(),
            spec.operation.as_label().into(),
        ),
        (
            "AUTONOMICS_SINGLE_CELL_MIN_GENES".into(),
            spec.min_genes.to_string(),
        ),
        (
            "AUTONOMICS_SINGLE_CELL_MIN_CELLS".into(),
            spec.min_cells.to_string(),
        ),
        (
            "AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL".into(),
            spec.normalize_total.to_string(),
        ),
    ]);

    Ok(ContainerCommandSpec {
        image: registry_image(
            SINGLE_CELL_PREPROCESSOR_IMAGE_REPOSITORY,
            SINGLE_CELL_PREPROCESSOR_IMAGE_DIGEST,
        )?,
        command: vec!["python".into(), "/opt/autonomics/preprocess.py".into()],
        script: None,
        files: Default::default(),
        env,
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "preprocess_report.json".into(),
                format: Some("single_cell_preprocess_report_json".into()),
            },
            ContainerCommandOutputSpec {
                path: "preprocessed.h5ad".into(),
                format: Some("h5ad".into()),
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
        cpus: Some(spec.cpus.unwrap_or(DEFAULT_CPUS)),
        memory: Some(spec.memory.clone().unwrap_or_else(|| DEFAULT_MEMORY.into())),
        pids_limit: Some(spec.pids_limit.unwrap_or(DEFAULT_PIDS_LIMIT)),
        shm_size: Some(DEFAULT_SHM_SIZE.into()),
        gpus: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "matrix_mtx")
        .add_input_port_of_type_with_label(None, PortType::File, "barcodes_tsv")
        .add_input_port_of_type_with_label(None, PortType::File, "features_tsv")
        .add_input_port_of_type_with_label(None, PortType::File, "cell_metadata")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for SingleCellPreprocessorContainerNodeFactory {
    fn kind(&self) -> &'static str {
        SINGLE_CELL_PREPROCESSOR_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Inspects or ingests 10x MatrixMarket single-cell inputs with Scanpy."
    }

    fn doc(&self) -> &'static str {
        "Input ports are the MatrixMarket matrix, barcodes, features, and cell \
        metadata. The default inspect operation reads only the matrix header, \
        validates companion-table alignment, and emits a JSON report plus a \
        metadata-only H5AD. The ingest operation loads expression values in a \
        pinned Scanpy image and emits a full H5AD. Both operations validate \
        metadata-table alignment with barcodes. The network is disabled and \
        the root filesystem is read-only."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SingleCellPreprocessorContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: SingleCellPreprocessorContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(SingleCellPreprocessorContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: SingleCellPreprocessorContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SingleCellPreprocessorContainerSpec {
        SingleCellPreprocessorContainerSpec {
            operation: SingleCellOperation::Inspect,
            min_genes: 0,
            min_cells: 0,
            normalize_total: false,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            cpus: None,
            memory: None,
            pids_limit: None,
        }
    }

    #[test]
    fn builds_isolated_four_input_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            registry_image(
                SINGLE_CELL_PREPROCESSOR_IMAGE_REPOSITORY,
                SINGLE_CELL_PREPROCESSOR_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert_eq!(
            container.command,
            vec![
                "python".to_string(),
                "/opt/autonomics/preprocess.py".to_string()
            ]
        );
        assert_eq!(container.outputs.len(), 2);
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.memory.as_deref(), Some(DEFAULT_MEMORY));
        assert_eq!(container.env["AUTONOMICS_SINGLE_CELL_OPERATION"], "inspect");
    }

    #[test]
    fn inspect_rejects_expression_filters() {
        let mut invalid = spec();
        invalid.min_genes = 10;
        let error = validate(&invalid).unwrap_err();
        assert!(error.contains("header-only"));
    }

    #[test]
    fn ingest_passes_filter_contract() {
        let mut valid = spec();
        valid.operation = SingleCellOperation::Ingest;
        valid.min_genes = 10;
        valid.min_cells = 20;
        valid.normalize_total = true;
        let container = container_spec(&valid).unwrap();
        assert_eq!(container.env["AUTONOMICS_SINGLE_CELL_MIN_GENES"], "10");
        assert_eq!(container.env["AUTONOMICS_SINGLE_CELL_MIN_CELLS"], "20");
        assert_eq!(
            container.env["AUTONOMICS_SINGLE_CELL_NORMALIZE_TOTAL"],
            "true"
        );
    }

    #[test]
    fn exposes_fixed_labeled_ports() {
        let ports = port_layout();
        assert_eq!(ports.input_ports().len(), 4);
        assert_eq!(ports.output_ports().len(), 2);
        assert!(ports.is_fixed_input());
    }
}
