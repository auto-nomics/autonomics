//! Containerized ggplot2 visualization node.
//!
//! Data and plot code enter as ordinary File values so the existing container
//! staging path owns all host/VFS I/O. The image provides a fixed R entrypoint
//! that loads the data as `df`, sources the caller's script, and saves the
//! required `p` plot as a PNG.

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
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const VISUALIZATION_CONTAINER_KIND: &str = "visualization_container";
pub const VISUALIZATION_IMAGE_REPOSITORY: &str = "visualization";
pub const VISUALIZATION_IMAGE_DIGEST: &str =
    "sha256:ee9592b77bc5ea0cebfafafbe39550c377204019f451d7a37e13e4ce2e884f15";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/visualization_container";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_WIDTH: f64 = 8.0;
const DEFAULT_HEIGHT: f64 = 6.0;
const DEFAULT_DPI: f64 = 150.0;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "2Gi";
const DEFAULT_PIDS_LIMIT: i64 = 256;

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualizationDataFormat {
    Csv,
    Tsv,
    Parquet,
    ArrowStream,
    ArrowFile,
}

impl VisualizationDataFormat {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Parquet => "parquet",
            Self::ArrowStream => "arrow_stream",
            Self::ArrowFile => "arrow_file",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct VisualizationContainerSpec {
    pub data_format: VisualizationDataFormat,
    #[serde(default = "default_width")]
    pub width: f64,
    #[serde(default = "default_height")]
    pub height: f64,
    #[serde(default = "default_dpi")]
    pub dpi: f64,
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

fn default_width() -> f64 {
    DEFAULT_WIDTH
}

fn default_height() -> f64 {
    DEFAULT_HEIGHT
}

fn default_dpi() -> f64 {
    DEFAULT_DPI
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct VisualizationContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl VisualizationContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct VisualizationContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for VisualizationContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for VisualizationContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        VISUALIZATION_CONTAINER_KIND
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

pub fn validate(spec: &VisualizationContainerSpec) -> Result<(), String> {
    if !spec.width.is_finite() || spec.width <= 0.0 {
        return Err("width must be finite and greater than zero".into());
    }
    if !spec.height.is_finite() || spec.height <= 0.0 {
        return Err("height must be finite and greater than zero".into());
    }
    if !spec.dpi.is_finite() || spec.dpi <= 0.0 {
        return Err("dpi must be finite and greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
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
    Ok(())
}

pub fn container_spec(spec: &VisualizationContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_VISUALIZATION_DATA_FORMAT".into(),
            spec.data_format.as_label().into(),
        ),
        (
            "AUTONOMICS_VISUALIZATION_WIDTH".into(),
            spec.width.to_string(),
        ),
        (
            "AUTONOMICS_VISUALIZATION_HEIGHT".into(),
            spec.height.to_string(),
        ),
        ("AUTONOMICS_VISUALIZATION_DPI".into(), spec.dpi.to_string()),
    ]);

    Ok(ContainerCommandSpec {
        image: acr_image(VISUALIZATION_IMAGE_REPOSITORY, VISUALIZATION_IMAGE_DIGEST)?,
        command: vec!["Rscript".into(), "/opt/autonomics/render.R".into()],
        script: None,
        files: Default::default(),
        env,
        outputs: vec![ContainerCommandOutputSpec {
            path: "plot.png".into(),
            format: Some("png".into()),
        }],
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
        shm_size: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "data")
        .add_input_port_of_type_with_label(None, PortType::File, "r_script")
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for VisualizationContainerNodeFactory {
    fn kind(&self) -> &'static str {
        VISUALIZATION_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Renders a data File and ggplot2 R script to PNG in an OCI container."
    }

    fn doc(&self) -> &'static str {
        "Renders a plot in an isolated R/ggplot2 OCI container. Input port 0 is \
        a data File (`csv`, `tsv`, `parquet`, Arrow IPC stream, or Arrow IPC \
        file); input port 1 is an R script File. The fixed container entrypoint \
        binds the loaded data to `df`, sources the script, requires the script \
        to assign a plot to `p`, and writes `plot.png` as an immutable VFS File \
        artifact. The network is disabled, the root filesystem is read-only, \
        and CPU, memory, PID, and runtime limits are enforced."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(VisualizationContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: VisualizationContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(VisualizationContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: VisualizationContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> VisualizationContainerSpec {
        VisualizationContainerSpec {
            data_format: VisualizationDataFormat::Csv,
            width: default_width(),
            height: default_height(),
            dpi: default_dpi(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            cpus: None,
            memory: None,
            pids_limit: None,
        }
    }

    #[test]
    fn builds_an_isolated_file_to_png_contract() {
        let container = container_spec(&spec()).unwrap();

        assert_eq!(
            container.image,
            acr_image(VISUALIZATION_IMAGE_REPOSITORY, VISUALIZATION_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(
            container.command,
            vec![
                "Rscript".to_string(),
                "/opt/autonomics/render.R".to_string()
            ]
        );
        assert_eq!(container.outputs.len(), 1);
        assert_eq!(container.outputs[0].path, "plot.png");
        assert_eq!(
            container.env.get("AUTONOMICS_VISUALIZATION_DATA_FORMAT"),
            Some(&"csv".to_string())
        );
        assert_eq!(container.cpus, Some(DEFAULT_CPUS));
        assert_eq!(container.memory.as_deref(), Some(DEFAULT_MEMORY));
        assert_eq!(container.pids_limit, Some(DEFAULT_PIDS_LIMIT));
    }

    #[test]
    fn rejects_nonpositive_figure_dimensions() {
        let mut value = spec();
        value.width = 0.0;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.height = f64::NAN;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.dpi = -1.0;
        assert!(validate(&value).is_err());
    }
}
