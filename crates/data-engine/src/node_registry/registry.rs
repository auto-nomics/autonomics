use std::sync::Arc;

use datafusion::{common::HashMap, prelude::SessionContext};
use datalake::Datalake;

use serde::Serialize;

use super::error::{Error, Result};

use crate::data_engine::{
    dag::DagNode,
    nodes::{
        ldsc_hsq::LdscHsqNodeFactory,
        linear_regression::LinearRegressionNodeFactory,
        mock_node::MockNodeFactory,
        sink::SinkNodeFactory,
        source::SourceNodeFactory,
        sql_node::SqlNodeFactory,
        univariate_mixer::UnivariateMixerNodeFactory,
    },
};

pub trait NodeFactory: Send + Sync {
    fn kind(&self) -> &'static str;
    fn spec_schema(&self) -> schemars::Schema;
    fn build(&self, spec: serde_json::Value, node_ctx: NodeCtx) -> Result<Box<dyn DagNode>>;
}

/// Context dependencies for DagNodes
#[derive(Clone)]
pub struct NodeCtx {
    /// Datafusion SessionContext shared by whole data engine thread.
    ///
    /// TODO: Instead of passing whole SessionContext, it is better to provider ingrediants that
    /// consititude SessionContext, then let nodes to build their own ctx by need.
    pub session: SessionContext,
    pub datalake: Arc<Datalake>,
}

/// Summary of a registered node kind returned by [`NodeRegistry::list_nodes`].
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub kind: String,
    pub schema: schemars::Schema,
}

/// The single source of truth of "which node kinds exist and how to build one from spec."
///
/// This object handles DagNode building and generalize operation of different nodes into uniformed
/// methods.
pub struct NodeRegistry {
    node_ctx: NodeCtx,
    nodes: HashMap<String, Box<dyn NodeFactory>>,
}

impl NodeRegistry {
    pub fn new(ctx: SessionContext, datalake: Arc<Datalake>) -> Self {
        let node_ctx = NodeCtx {
            session: ctx,
            datalake,
        };

        let mut registry = Self {
            node_ctx,
            nodes: Default::default(),
        };
        registry.register(Box::new(SqlNodeFactory {}));
        registry.register(Box::new(SourceNodeFactory {}));
        registry.register(Box::new(SinkNodeFactory {}));
        registry.register(Box::new(LdscHsqNodeFactory {}));
        registry.register(Box::new(LinearRegressionNodeFactory {}));
        registry.register(Box::new(MockNodeFactory {}));
        registry.register(Box::new(UnivariateMixerNodeFactory {}));
        registry
    }

    pub fn register(&mut self, factory: Box<dyn NodeFactory>) {
        self.nodes.insert(factory.kind().to_string(), factory);
    }

    fn get_node_factory(&self, node_kind: &str) -> Result<&dyn NodeFactory> {
        self.nodes
            .get(node_kind)
            .map(|b| b.as_ref())
            .ok_or(Error::FactoryNotFound {
                kind: node_kind.to_string(),
            })
    }

    pub fn build_node(&self, node_kind: &str, spec: serde_json::Value) -> Result<Box<dyn DagNode>> {
        let node_factory = self.get_node_factory(node_kind)?;
        let node = node_factory.build(spec, self.node_ctx.clone())?;
        Ok(node)
    }

    pub fn get_node_spec(&self, node_kind: &str) -> Result<schemars::Schema> {
        Ok(self.get_node_factory(node_kind)?.spec_schema())
    }

    /// Return metadata of every registered node kind (kind + JSON Schema).
    pub fn list_nodes(&self) -> Vec<NodeInfo> {
        self.nodes
            .iter()
            .map(|(kind, factory)| NodeInfo {
                kind: kind.clone(),
                schema: factory.spec_schema(),
            })
            .collect()
    }
}
