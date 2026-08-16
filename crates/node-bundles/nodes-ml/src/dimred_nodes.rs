//! Dimensionality reduction DAG nodes — PCA, ICA, t-SNE, NMF, Truncated SVD.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
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
        node_type: "ml_dimred".into(),
        msg: "no input".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_dimred".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_dimred".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

/// Build an output RecordBatch from embedding coordinates + original data.
fn build_embedding_output(
    batches: &[RecordBatch],
    embedding: &[Vec<f64>],
    prefix: &str,
) -> Result<RecordBatch, DagError> {
    let (_schema, mut fields, mut arrays) = common::concat_input(batches)?;

    let n_dims = embedding.first().map(|r| r.len()).unwrap_or(0);
    for d in 0..n_dims {
        let col_data: Vec<f64> = embedding.iter().map(|row| row[d]).collect();
        fields.push(Arc::new(Field::new(
            format!("{prefix}_{d}"),
            DataType::Float64,
            true,
        )));
        arrays.push(Arc::new(Float64Array::from(col_data)));
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_dimred".into(),
        msg: format!("build output: {e}"),
    })
}

/// Build a summary table from key-value float metrics.
fn build_summary_output(rows: &[(String, f64)], node_type: &str) -> Result<RecordBatch, DagError> {
    let (names, values): (Vec<String>, Vec<f64>) = rows.iter().cloned().unzip();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("component", DataType::Utf8, false),
            Field::new("value", DataType::Float64, false),
        ])),
        vec![
            Arc::new(arrow_array::StringArray::from(names)),
            Arc::new(Float64Array::from(values)),
        ],
    )
    .map_err(|e| DagError::NodeError {
        node_type: node_type.into(),
        msg: e.to_string(),
    })
}

mod pca;
pub use pca::PcaFactory;

mod ica;
pub use ica::IcaFactory;

mod tsne;
pub use tsne::TsneFactory;

mod nmf;
pub use nmf::NmfFactory;

mod truncated_svd;
pub use truncated_svd::TruncatedSvdFactory;
