//! Clustering DAG nodes.
//!
//! Each node implementation lives in its own module so CPU-heavy nodes can be
//! migrated to blocking execution without touching unrelated node bodies.

mod dbscan;
mod gmm;
mod hierarchical;
mod kmeans;
mod spectral;

pub use dbscan::{DbscanFactory, DbscanSpec};
pub use gmm::{GmmFactory, GmmSpec};
pub use hierarchical::{HierarchicalFactory, HierarchicalSpec};
pub use kmeans::{KMeansFactory, KMeansSpec};
pub use spectral::SpectralClusteringFactory;

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};

use crate::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::NodeInput;
use dag_core::registry::NodeCtx;

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_cluster".into(),
        msg: "no input port connected".into(),
    })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_cluster".into(),
            msg: format!("collect failed: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_cluster".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

fn build_cluster_output(
    batches: &[RecordBatch],
    labels: &[usize],
    distances: &[f64],
    cluster_col: &str,
    dist_col: &str,
) -> Result<RecordBatch, DagError> {
    let (_schema, mut fields, mut arrays) = common::concat_input(batches)?;

    fields.push(Arc::new(Field::new(cluster_col, DataType::UInt32, true)));
    arrays.push(Arc::new(UInt32Array::from(
        labels.iter().map(|&v| v as u32).collect::<Vec<_>>(),
    )));

    // Distance columns beginning with '_' are internal placeholders for nodes
    // whose algorithm does not produce a per-sample distance.
    if !dist_col.starts_with('_') {
        fields.push(Arc::new(Field::new(dist_col, DataType::Float64, true)));
        arrays.push(Arc::new(Float64Array::from(Vec::from(distances))));
    }

    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_cluster".into(),
        msg: format!("build output: {e}"),
    })
}

fn build_int_cluster_output(
    batches: &[RecordBatch],
    labels: &[i32],
    cluster_col: &str,
) -> Result<RecordBatch, DagError> {
    let (_schema, mut fields, mut arrays) = common::concat_input(batches)?;
    fields.push(Arc::new(Field::new(cluster_col, DataType::Int32, true)));
    arrays.push(Arc::new(Int32Array::from(Vec::from(labels))));

    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_cluster".into(),
        msg: format!("build output: {e}"),
    })
}
