//! K-means clustering node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::{build_cluster_output, collect_batches, emit_batch};
use crate::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use ml::cluster::{KMeansOptions, kmeans, kmeans_predict};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KMeansSpec {
    /// Feature columns for clustering.
    pub features: Vec<String>,
    /// Number of clusters K.
    pub k: usize,
    /// Maximum iterations (default 300).
    #[serde(default = "d_max_iter")]
    pub max_iter: u32,
    /// Number of restarts with different init (default 10).
    #[serde(default = "d_n_init")]
    pub n_init: u32,
    /// Convergence tolerance (default 1e-4).
    #[serde(default = "d_tol")]
    pub tolerance: f64,
    /// Random seed (default 42).
    #[serde(default = "d_seed")]
    pub seed: u64,
}

fn d_max_iter() -> u32 {
    300
}

fn d_n_init() -> u32 {
    10
}

fn d_tol() -> f64 {
    1e-4
}

fn d_seed() -> u64 {
    42
}

pub struct KMeansFactory;

impl NodeFactory for KMeansFactory {
    fn kind(&self) -> &'static str {
        "ml_kmeans"
    }

    fn desc(&self) -> &'static str {
        "K-means clustering with k-means++ initialisation."
    }

    fn doc(&self) -> &'static str {
        "KMeans: partitions data into K clusters using Lloyd's algorithm with k-means++ seeding. Outputs cluster assignments + centroid distances."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KMeansSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: KMeansSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KMeansNode {
            features: s.features,
            k: s.k,
            max_iter: s.max_iter,
            n_init: s.n_init,
            tolerance: s.tolerance,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct KMeansNode {
    features: Vec<String>,
    k: usize,
    max_iter: u32,
    n_init: u32,
    tolerance: f64,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for KMeansNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "ml_kmeans"
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
                node_type: "ml_kmeans".into(),
                msg: e.to_string(),
            })?;
        let opts = KMeansOptions {
            k: self.k,
            max_n_iterations: self.max_iter,
            n_init: self.n_init,
            tolerance: self.tolerance,
            seed: self.seed,
        };
        let model = kmeans(&data, &opts).map_err(|e| DagError::NodeError {
            node_type: "ml_kmeans".into(),
            msg: e.to_string(),
        })?;
        let labels = kmeans_predict(&model, &data);

        // Compute distance to assigned centroid.
        let nrows = data.nrows();
        let distances: Vec<f64> = (0..nrows)
            .map(|i| {
                let centroid = &model.centroids[labels[i]];
                let point: Vec<f64> = (0..data.ncols()).map(|j| data[(i, j)]).collect();
                point
                    .iter()
                    .zip(centroid)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .collect();

        let batch = build_cluster_output(
            &batches,
            &labels,
            &distances,
            "cluster",
            "distance_to_centroid",
        )?;
        emit_batch(ctx, batch)
    }
}
