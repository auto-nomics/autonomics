use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util::{file_port, request_error, write_pdf};
use crate::{ProtocolPdfView, ProtocolioClient};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProtocolioPdfSpec {
    /// Numeric protocol ID or URI.
    pub protocol_id: String,
    /// `vfs://...` or an absolute local path.
    pub path: String,
    /// `full`, `compact`, `materials`, `commands`, or `steps`.
    #[serde(default)]
    pub view: Option<String>,
}

#[derive(Clone)]
pub struct ProtocolioPdfNode {
    meta: NodePorts,
    spec: ProtocolioPdfSpec,
}

pub struct ProtocolioPdfNodeFactory;

impl NodeFactory for ProtocolioPdfNodeFactory {
    fn kind(&self) -> &'static str {
        "source_protocolio_pdf"
    }

    fn desc(&self) -> &'static str {
        "Download one protocols.io protocol as a PDF FileRef."
    }

    fn doc(&self) -> &'static str {
        "Uses the independently rate-limited PDF export endpoint and writes through the \
        engine VFS when the destination starts with `vfs://`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProtocolioPdfSpec)
    }

    fn ports(&self) -> NodePorts {
        file_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(ProtocolioPdfNode {
            meta: file_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ProtocolioPdfNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_protocolio_pdf"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let view = self
            .spec
            .view
            .as_deref()
            .map(ProtocolPdfView::parse)
            .transpose()
            .map_err(request_error)?
            .unwrap_or_default();
        let bytes = ProtocolioClient::new()
            .map_err(request_error)?
            .pdf(&self.spec.protocol_id, view)
            .await
            .map_err(request_error)?;
        let file = write_pdf(ctx, &self.spec.path, bytes).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}
