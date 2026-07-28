//! Iceberg source node: brings an Iceberg table into the DAG as a `DataFrame`.
//!
//! An [`IcebergSourceNode`] has no inputs and produces exactly one output. The
//! table is resolved by identifier (`namespace.table`) through the `iceberg`
//! catalog registered on the engine context. Symmetric to
//! [`crate::nodes::IcebergSinkNode`] for the Iceberg case.

use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::JsonSchema;
use serde::Deserialize;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::DagError,
    dag::graph::PortOutputs,
    node_registry::registry::{NodeCtx, NodeFactory},
};

#[derive(Clone)]
pub struct IcebergSourceNode {
    meta: NodePorts,
    ident: String,
}

impl IcebergSourceNode {
    pub fn new(ident: String) -> Self {
        // A source has no inputs and a single output port.
        Self {
            meta: port_layout(),
            ident,
        }
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct IcebergSourceNodeSpec {
    /// An Iceberg table identifier (`namespace.table`), resolved through the
    /// `iceberg` catalog registered on the engine context.
    pub ident: String,
}

pub struct IcebergSourceNodeFactory {}

/// Static port layout for every [`IcebergSourceNode`]: no inputs, a single
/// untyped output port (schema discovered from the source at runtime).
fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for IcebergSourceNodeFactory {
    fn kind(&self) -> &'static str {
        "source_iceberg"
    }

    fn desc(&self) -> &'static str {
        "Reads an Iceberg table into the DAG as a DataFrame."
    }

    fn doc(&self) -> &'static str {
        "An Iceberg data source node that reads an Iceberg table into the DAG \
        as a DataFrame, resolved by its `namespace.table` identifier through the \
        `iceberg` catalog registered on the engine context. No input ports; one \
        untyped output port."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schemars::schema_for!(IcebergSourceNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let node_spec: IcebergSourceNodeSpec = serde_json::from_value(spec)?;
        let node = IcebergSourceNode::new(node_spec.ident);
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for IcebergSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_iceberg"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let ctx = node_ctx.session();
        // The iceberg catalog is registered under "iceberg"; qualify the
        // identifier so DataFusion resolves it through that catalog.
        let df = ctx
            .sql(&format!("SELECT * FROM iceberg.{}", self.ident))
            .await?;
        let mut res: PortOutputs = HashMap::new();
        res.insert(0, df);
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> crate::node_registry::registry::NodeCtx {
        crate::node_registry::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }
    use super::*;
    use datalake::Datalake;

    #[tokio::test]
    #[ignore = "e2e test"]
    async fn test_load_from_iceberg() {
        let _ctx = Datalake::default().get_ctx().await.unwrap();
        let _provider = Datalake::default().get_provider().await.unwrap();
        let mut node = IcebergSourceNode::new("gwas.gwas_study".to_string());
        let res = node
            .execute(
                &node_ctx(),
                &[],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = res.get(&0).unwrap().clone();
        df.limit(0, Some(10)).unwrap().show().await.unwrap();
    }
}
