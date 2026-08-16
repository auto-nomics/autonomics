//! DBSCAN clustering node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::{build_int_cluster_output, collect_batches, emit_batch};
use crate::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use ml::cluster::{DbscanOptions, dbscan};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DbscanSpec {
    pub features: Vec<String>,
    /// Maximum distance between two samples for them to be neighbours.
    pub eps: f64,
    /// Minimum samples in a neighbourhood to form a core point.
    #[serde(default = "d_min_points")]
    pub min_points: usize,
}

fn d_min_points() -> usize {
    5
}

pub struct DbscanFactory;

impl NodeFactory for DbscanFactory {
    fn kind(&self) -> &'static str {
        "ml_dbscan"
    }

    fn desc(&self) -> &'static str {
        "DBSCAN density-based clustering."
    }

    fn doc(&self) -> &'static str {
        "DBSCAN: identifies clusters of arbitrary shape via density reachability. Noise points are labelled -1 (null)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DbscanSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: DbscanSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DbscanNode {
            features: s.features,
            eps: s.eps,
            min_points: s.min_points,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct DbscanNode {
    features: Vec<String>,
    eps: f64,
    min_points: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for DbscanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "ml_dbscan"
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
                node_type: "ml_dbscan".into(),
                msg: e.to_string(),
            })?;
        let result = dbscan(
            &data,
            &DbscanOptions {
                eps: self.eps,
                min_points: self.min_points,
            },
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_dbscan".into(),
            msg: e.to_string(),
        })?;

        let labels: Vec<i32> = result
            .labels
            .iter()
            .map(|l| l.map(|v| v as i32).unwrap_or(-1))
            .collect();
        let batch = build_int_cluster_output(&batches, &labels, "cluster")?;
        emit_batch(ctx, batch)
    }
}
