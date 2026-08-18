//! Supervised learning DAG nodes — classification + regression.
//!
//! Classification: ml_logistic, ml_gaussian_nb, ml_knn, ml_decision_tree
//! Regression: ml_linear_regress, ml_elastic_net, ml_lars, ml_pls

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_supervised".into(),
        msg: "no input".into(),
    })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_supervised".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_supervised".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

mod logistic;
pub use logistic::LogisticFactory;

mod gaussian_nb;
pub use gaussian_nb::GaussianNbFactory;

mod knn;
pub use knn::KnnFactory;

mod decision_tree;
pub use decision_tree::DecisionTreeFactory;

mod linear_regress;
pub use linear_regress::LinearRegressFactory;

mod elastic_net;
pub use elastic_net::ElasticNetFactory;
