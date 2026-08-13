//! Self-contained data-generating node for causal-forest tests.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use grf::nodes::GenerateCausalDataSpec;

use crate::common;

pub struct GenerateCausalDataNode {
    spec: GenerateCausalDataSpec,
    meta: NodePorts,
}
impl Clone for GenerateCausalDataNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}
pub struct GenerateCausalDataNodeFactory;
impl NodeFactory for GenerateCausalDataNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_generate_causal_data"
    }
    fn desc(&self) -> &'static str {
        "Generate synthetic causal data (DGP)."
    }
    fn doc(&self) -> &'static str {
        "grf_generate_causal_data: no inputs; emits a synthetic dataset with y, w, true_tau, and x0..x{p-1}."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GenerateCausalDataSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(GenerateCausalDataNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for GenerateCausalDataNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_generate_causal_data"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        let batch = self
            .spec
            .generate()
            .map_err(|e| dag_err(node, &e.to_string()))?;
        common::emit(ctx, node, batch)
    }
}

fn dag_err(node: &str, msg: &str) -> DagError {
    DagError::NodeError {
        node_type: node.into(),
        msg: msg.to_string(),
    }
}
