//! Deep learning DAG nodes — MLP classifier, Autoencoder.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, UInt32Array};
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
        node_type: "ml_deep".into(),
        msg: "no input".into(),
    })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_deep".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_deep".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

mod mlp;
pub use mlp::MlpFactory;

mod autoencoder;
pub use autoencoder::AutoencoderFactory;
