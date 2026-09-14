//! Containerized TimesFM univariate forecasting node.
//!
//! The pinned image contains the Apache-2.0 TimesFM 2.5 checkpoint, so the
//! node can run with the container runtime's default isolated network. The
//! wrapper owns only the file and parameter contract; forecasting remains in
//! the official Python implementation inside the image.

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

pub const TIMESFM_FORECAST_CONTAINER_KIND: &str = "timesfm_forecast_container";
pub const TIMESFM_ORIGINAL_IMAGE_REPOSITORY: &str = "timesfm";
pub const TIMESFM_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

const DEFAULT_HORIZON: u16 = 12;
const DEFAULT_MAX_CONTEXT: u32 = 1024;
const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/timesfm_forecast_container";
const DEFAULT_TIMEOUT_SECS: u64 = 1800;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TimesfmForecastContainerSpec {
    /// Number of future points to forecast for every input series.
    #[serde(default = "default_horizon")]
    pub horizon: u16,
    /// Context window compiled into the official TimesFM decoder.
    #[serde(default = "default_max_context")]
    pub max_context: u32,
    /// VFS prefix used to publish declared outputs.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Wall-clock timeout for model loading and inference.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_horizon() -> u16 {
    DEFAULT_HORIZON
}

fn default_max_context() -> u32 {
    DEFAULT_MAX_CONTEXT
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct TimesfmForecastContainerNodeFactory {
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl TimesfmForecastContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct TimesfmForecastContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for TimesfmForecastContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for TimesfmForecastContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        TIMESFM_FORECAST_CONTAINER_KIND
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

pub fn validate(spec: &TimesfmForecastContainerSpec) -> Result<(), String> {
    if spec.horizon == 0 || spec.horizon > 256 {
        return Err("horizon must be between 1 and 256".into());
    }
    if spec.max_context == 0 || spec.max_context > 16_128 {
        return Err("max_context must be between 1 and 16128".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

pub fn container_spec(spec: &TimesfmForecastContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    Ok(ContainerCommandSpec {
        image: acr_image(
            TIMESFM_ORIGINAL_IMAGE_REPOSITORY,
            TIMESFM_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec![
            "python".into(),
            "-m".into(),
            "timesfm_service.runner".into(),
        ],
        script: None,
        files: Default::default(),
        env: [
            (
                "TIMESFM_CHECKPOINT".into(),
                "/opt/timesfm/checkpoint".into(),
            ),
            (
                "TIMESFM_REVISION".into(),
                "1d952420fba87f3c6dee4f240de0f1a0fbc790e3".into(),
            ),
            ("TIMESFM_HORIZON".into(), spec.horizon.to_string()),
            ("TIMESFM_MAX_CONTEXT".into(), spec.max_context.to_string()),
            ("TIMESFM_LOCAL_FILES_ONLY".into(), "true".into()),
        ]
        .into(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "forecast_result.json".into(),
                format: Some("timesfm_forecast_json".into()),
            },
            ContainerCommandOutputSpec {
                path: "forecast.log".into(),
                format: Some("timesfm_log".into()),
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
        cpus: Some(4.0),
        memory: Some("8Gi".into()),
        pids_limit: Some(512),
        shm_size: Some("1Gi".into()),
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for TimesfmForecastContainerNodeFactory {
    fn kind(&self) -> &'static str {
        TIMESFM_FORECAST_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official TimesFM 2.5 univariate forecasting in an offline OCI image."
    }

    fn doc(&self) -> &'static str {
        "Input is one JSON File with either a `target` numeric array or a \
        two-dimensional `target` batch. Returns a forecast JSON artifact and \
        execution log. The pinned image contains the Apache-2.0 TimesFM 2.5 \
        checkpoint, allowing reproducible inference without network egress."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TimesfmForecastContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: TimesfmForecastContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(TimesfmForecastContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: TimesfmForecastContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TimesfmForecastContainerSpec {
        TimesfmForecastContainerSpec {
            horizon: 12,
            max_context: 128,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_timesfm_contract() {
        let container = container_spec(&spec()).unwrap();

        assert_eq!(
            container.image,
            acr_image(
                TIMESFM_ORIGINAL_IMAGE_REPOSITORY,
                TIMESFM_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert_eq!(
            container.command,
            vec![
                "python".to_string(),
                "-m".to_string(),
                "timesfm_service.runner".to_string()
            ]
        );
        assert_eq!(
            container.env.get("TIMESFM_HORIZON").map(String::as_str),
            Some("12")
        );
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.outputs.len(), 2);
    }

    #[test]
    fn validates_model_limits() {
        assert!(validate(&spec()).is_ok());

        let mut value = spec();
        value.horizon = 257;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.max_context = 16_384;
        assert!(validate(&value).is_err());
    }
}
