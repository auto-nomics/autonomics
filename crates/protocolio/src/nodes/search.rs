use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagNode, NodePorts};
use dag_core::registry::NodeFactory;

use crate::nodes::util::{batch_error, dataframe_output, dataframe_port, request_error};
use crate::{ProtocolListQuery, ProtocolioClient, convert};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProtocolioSearchSpec {
    /// Search title, description, and authors.
    pub query: String,
    /// `public`, `user_public`, `user_private`, or `shared_with_user`.
    #[serde(default)]
    pub filter: Option<String>,
    /// `activity`, `relevance`, `date`, `name`, or `id`.
    #[serde(default)]
    pub order_field: Option<String>,
    /// `asc` or `desc`.
    #[serde(default)]
    pub order_dir: Option<String>,
    /// 1 to 100.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// One-based page number.
    #[serde(default)]
    pub page_id: Option<u32>,
    /// Set true to include only peer-reviewed protocols.
    #[serde(default)]
    pub peer_reviewed: Option<bool>,
}

#[derive(Clone)]
pub struct ProtocolioSearchNode {
    meta: NodePorts,
    spec: ProtocolioSearchSpec,
}

pub struct ProtocolioSearchNodeFactory;

impl NodeFactory for ProtocolioSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_protocolio_protocols"
    }

    fn desc(&self) -> &'static str {
        "Search protocols.io and return protocol summaries as a table."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node for the protocols.io v3 protocol list API. \
        Output columns include protocol_id, title, doi, uri, url, version_id, \
        publication and creation timestamps, creators, authors, step count, \
        public flag, and view count."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProtocolioSearchSpec)
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
        Ok(Box::new(ProtocolioSearchNode {
            meta: dataframe_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ProtocolioSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_protocolio_protocols"
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
        let query = ProtocolListQuery {
            filter: Some(self.spec.filter.clone().unwrap_or_else(|| "public".into())),
            key: self.spec.query.clone(),
            order_field: self.spec.order_field.clone(),
            order_dir: self.spec.order_dir.clone(),
            page_size: Some(self.spec.page_size.unwrap_or(20)),
            page_id: self.spec.page_id,
            peer_reviewed: self.spec.peer_reviewed,
            ..Default::default()
        };
        let response = ProtocolioClient::new()
            .map_err(request_error)?
            .list_protocols(&query)
            .await
            .map_err(request_error)?;
        let batch = convert::protocol_summaries(&response.items).map_err(batch_error)?;
        dataframe_output(ctx, batch)
    }
}
