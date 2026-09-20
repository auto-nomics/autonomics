use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts};
use dag_core::registry::NodeFactory;

use crate::nodes::util::{batch_error, dataframe_output, dataframe_port, request_error};
use crate::{ContentFormat, ProtocolioClient, convert};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProtocolioStepsSpec {
    /// Numeric ID, URI, DOI, DOI/vN, or DOI/latest.
    pub protocol_id: String,
    /// Explicitly request the latest version.
    #[serde(default)]
    pub last_version: Option<bool>,
    /// `json`, `html`, or `markdown`.
    #[serde(default)]
    pub content_format: Option<String>,
}

#[derive(Clone)]
pub struct ProtocolioStepsNode {
    meta: NodePorts,
    spec: ProtocolioStepsSpec,
}

pub struct ProtocolioStepsNodeFactory;

impl NodeFactory for ProtocolioStepsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_protocolio_steps"
    }

    fn desc(&self) -> &'static str {
        "Fetch ordered protocols.io steps as a table."
    }

    fn doc(&self) -> &'static str {
        "Requests protocol steps and reconstructs execution order from previous-step links. \
        Markdown bodies and original component JSON are both retained."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProtocolioStepsSpec)
    }

    fn ports(&self) -> NodePorts {
        dataframe_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(ProtocolioStepsNode {
            meta: dataframe_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ProtocolioStepsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_protocolio_steps"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &dag_core::registry::NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> dag_core::dag::error::Result<dag_core::dag::graph::PortOutputs> {
        let format = parse_format(self.spec.content_format.as_deref())?;
        let steps = ProtocolioClient::new()
            .map_err(request_error)?
            .steps(
                &self.spec.protocol_id,
                self.spec.last_version.unwrap_or(false),
                format,
            )
            .await
            .map_err(request_error)?;
        let batch = convert::steps(&self.spec.protocol_id, None, &steps).map_err(batch_error)?;
        dataframe_output(ctx, batch)
    }
}

fn parse_format(value: Option<&str>) -> Result<ContentFormat, DagError> {
    value
        .map(ContentFormat::parse)
        .unwrap_or_default()
        .ok_or_else(|| {
            DagError::Schedule("content_format must be json, html, or markdown".to_owned())
        })
}
