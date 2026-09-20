use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagNode, NodePorts};
use dag_core::registry::NodeFactory;

use crate::nodes::util::{batch_error, dataframe_output, dataframe_port, request_error};
use crate::{ProtocolioClient, convert};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProtocolioMaterialsSpec {
    /// Numeric ID, URI, DOI, or versioned DOI.
    pub protocol_id: String,
}

#[derive(Clone)]
pub struct ProtocolioMaterialsNode {
    meta: NodePorts,
    spec: ProtocolioMaterialsSpec,
}

pub struct ProtocolioMaterialsNodeFactory;

impl NodeFactory for ProtocolioMaterialsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_protocolio_materials"
    }

    fn desc(&self) -> &'static str {
        "Fetch materials for one protocols.io protocol as a table."
    }

    fn doc(&self) -> &'static str {
        "Emits one row per reagent or linked material with vendor, SKU, product URL, \
        molecular weight, and CiteAB status."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProtocolioMaterialsSpec)
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
        Ok(Box::new(ProtocolioMaterialsNode {
            meta: dataframe_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ProtocolioMaterialsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_protocolio_materials"
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
        let materials = ProtocolioClient::new()
            .map_err(request_error)?
            .materials(&self.spec.protocol_id)
            .await
            .map_err(request_error)?;
        let batch = convert::materials(&self.spec.protocol_id, &materials).map_err(batch_error)?;
        dataframe_output(ctx, batch)
    }
}
