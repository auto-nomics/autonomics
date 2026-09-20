use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagNode, NodePorts};
use dag_core::registry::NodeFactory;

use crate::nodes::util::{batch_error, dataframe_output, dataframe_port, request_error};
use crate::{ProtocolioClient, ReagentListQuery, convert};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProtocolioReagentsSpec {
    /// Reagent search text.
    pub query: String,
    /// Include CiteAB reagents only.
    #[serde(default)]
    pub is_citeab: Option<bool>,
    /// Unix timestamp lower bound.
    #[serde(default)]
    pub from: Option<i64>,
    /// Unix timestamp upper bound.
    #[serde(default)]
    pub to: Option<i64>,
    /// 1 to 100.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// One-based page number.
    #[serde(default)]
    pub page_id: Option<u32>,
}

#[derive(Clone)]
pub struct ProtocolioReagentsNode {
    meta: NodePorts,
    spec: ProtocolioReagentsSpec,
}

pub struct ProtocolioReagentsNodeFactory;

impl NodeFactory for ProtocolioReagentsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_protocolio_reagents"
    }

    fn desc(&self) -> &'static str {
        "Search protocols.io reagents as a table."
    }

    fn doc(&self) -> &'static str {
        "Queries the v3 reagent database and emits identifiers, names, SKUs, vendors, \
        product links, formulas, molecular weights, and CiteAB status."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProtocolioReagentsSpec)
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
        Ok(Box::new(ProtocolioReagentsNode {
            meta: dataframe_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ProtocolioReagentsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_protocolio_reagents"
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
        let query = ReagentListQuery {
            key: self.spec.query.clone(),
            is_citeab: self.spec.is_citeab,
            from: self.spec.from,
            to: self.spec.to,
            page_size: Some(self.spec.page_size.unwrap_or(20)),
            page_id: self.spec.page_id,
        };
        let response = ProtocolioClient::new()
            .map_err(request_error)?
            .list_reagents(&query)
            .await
            .map_err(request_error)?;
        let batch = convert::reagents(&response.items).map_err(batch_error)?;
        dataframe_output(ctx, batch)
    }
}
