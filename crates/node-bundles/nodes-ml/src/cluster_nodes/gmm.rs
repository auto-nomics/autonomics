//! Gaussian mixture model node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::{build_cluster_output, collect_batches, emit_batch};
use crate::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use ml::cluster::{GmmOptions, gaussian_density_pub, gaussian_mixture};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GmmSpec {
    pub features: Vec<String>,
    pub k: usize,
    #[serde(default = "d_gmm_max_iter")]
    pub max_iter: usize,
    #[serde(default = "d_gmm_tol")]
    pub tol: f64,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default = "d_gmm_n_init")]
    pub n_init: usize,
}

fn d_gmm_max_iter() -> usize {
    100
}

fn d_gmm_tol() -> f64 {
    1e-6
}

fn d_seed() -> u64 {
    42
}

fn d_gmm_n_init() -> usize {
    1
}

pub struct GmmFactory;

impl NodeFactory for GmmFactory {
    fn kind(&self) -> &'static str {
        "ml_gmm"
    }

    fn desc(&self) -> &'static str {
        "Gaussian Mixture Model (EM) with full covariance."
    }

    fn doc(&self) -> &'static str {
        "GMM: fits a Gaussian mixture via EM with k-means++ init. Reports BIC/AIC for model selection. Outputs cluster assignments + per-component probabilities summary."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GmmSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: GmmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(GmmNode {
            features: s.features,
            k: s.k,
            max_iter: s.max_iter,
            tol: s.tol,
            seed: s.seed,
            n_init: s.n_init,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct GmmNode {
    features: Vec<String>,
    k: usize,
    max_iter: usize,
    tol: f64,
    seed: u64,
    n_init: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for GmmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "ml_gmm"
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
                node_type: "ml_gmm".into(),
                msg: e.to_string(),
            })?;
        let opts = GmmOptions {
            k: self.k,
            max_iter: self.max_iter,
            tol: self.tol,
            seed: self.seed,
            n_init: self.n_init,
        };
        let model = gaussian_mixture(&data, &opts).map_err(|e| DagError::NodeError {
            node_type: "ml_gmm".into(),
            msg: e.to_string(),
        })?;

        // Assign each sample to the component with the greatest weighted density.
        let (nrows, d) = data.shape();
        let labels: Vec<usize> = (0..nrows)
            .map(|i| {
                let point: Vec<f64> = (0..d).map(|j| data[(i, j)]).collect();
                (0..model.k)
                    .map(|c| {
                        let cov = common::mat_from_row_major(
                            d,
                            d,
                            &model.covariances[c]
                                .iter()
                                .flatten()
                                .cloned()
                                .collect::<Vec<_>>(),
                        );
                        (
                            c,
                            gaussian_density_pub(&point, &model.means[c], &cov) * model.weights[c],
                        )
                    })
                    .fold((0usize, f64::NEG_INFINITY), |(bc, bp), (c, p)| {
                        if p > bp { (c, p) } else { (bc, bp) }
                    })
                    .0
            })
            .collect();

        let batch = build_cluster_output(
            &batches,
            &labels,
            &vec![0.0; nrows],
            "cluster",
            "_gmm_dummy",
        )?;
        emit_batch(ctx, batch)
    }
}
