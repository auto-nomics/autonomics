use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts};
use dag_core::registry::NodeFactory;

use crate::nodes::util::{batch_error, dataframe_output, dataframe_port, request_error};
use crate::{ContentFormat, ProtocolioClient, convert};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProtocolioProtocolSpec {
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
pub struct ProtocolioProtocolNode {
    meta: NodePorts,
    spec: ProtocolioProtocolSpec,
}

pub struct ProtocolioProtocolNodeFactory;

impl NodeFactory for ProtocolioProtocolNodeFactory {
    fn kind(&self) -> &'static str {
        "source_protocolio_protocol"
    }

    fn desc(&self) -> &'static str {
        "Fetch one protocols.io protocol metadata record as a table."
    }

    fn doc(&self) -> &'static str {
        "Resolves a protocol by ID, URI, or DOI and emits one metadata row. \
        Rich text is rendered as Markdown by default. Resolved version identifiers \
        are retained for provenance."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProtocolioProtocolSpec)
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
        Ok(Box::new(ProtocolioProtocolNode {
            meta: dataframe_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ProtocolioProtocolNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_protocolio_protocol"
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
        let protocol = ProtocolioClient::new()
            .map_err(request_error)?
            .protocol(
                &self.spec.protocol_id,
                self.spec.last_version.unwrap_or(false),
                format,
            )
            .await
            .map_err(request_error)?;
        let batch = convert::protocol(&protocol).map_err(batch_error)?;
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
