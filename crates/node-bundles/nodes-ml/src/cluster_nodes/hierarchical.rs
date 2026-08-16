//! Agglomerative hierarchical clustering node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::{build_cluster_output, collect_batches, emit_batch};
use crate::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use ml::cluster::{Linkage, hierarchical};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HierarchicalSpec {
    pub features: Vec<String>,
    pub k: usize,
    #[serde(default = "d_linkage")]
    pub linkage: String,
}

fn d_linkage() -> String {
    "ward".into()
}

pub struct HierarchicalFactory;

impl NodeFactory for HierarchicalFactory {
    fn kind(&self) -> &'static str {
        "ml_hierarchical"
    }

    fn desc(&self) -> &'static str {
        "Agglomerative hierarchical clustering."
    }

    fn doc(&self) -> &'static str {
        "Hierarchical: bottom-up agglomerative clustering with Ward/complete/average/single linkage. Cuts the dendrogram at K clusters."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HierarchicalSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: HierarchicalSpec = serde_json::from_value(spec)?;
        Ok(Box::new(HierarchicalNode {
            features: s.features,
            k: s.k,
            linkage: s.linkage,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct HierarchicalNode {
    features: Vec<String>,
    k: usize,
    linkage: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for HierarchicalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "ml_hierarchical"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_hierarchical".into(),
                msg: e.to_string(),
            })?;
        let linkage = match self.linkage.as_str() {
            "complete" => Linkage::Complete,
            "average" => Linkage::Average,
            "single" => Linkage::Single,
            _ => Linkage::Ward,
        };
        let result = hierarchical(&data, self.k, linkage).map_err(|e| DagError::NodeError {
            node_type: "ml_hierarchical".into(),
            msg: e.to_string(),
        })?;
        let zeros = vec![0.0; data.nrows()];
        let batch =
            build_cluster_output(&batches, &result.labels, &zeros, "cluster", "_hier_dummy")?;
        emit_batch(ctx, batch)
    }
}
