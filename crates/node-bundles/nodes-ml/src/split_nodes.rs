//! Split & cross-validation DAG nodes.

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

// ── helpers ──────────────────────────────────────────────────────────────

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_split".into(),
        msg: "no input".into(),
    })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_split".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_split".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

fn emit_two_batches(
    ctx: &NodeCtx,
    b0: RecordBatch,
    b1: RecordBatch,
) -> Result<PortOutputs, DagError> {
    let df0 = ctx
        .session()
        .read_batch(b0)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_split".into(),
            msg: format!("read_batch(0): {e}"),
        })?;
    let df1 = ctx
        .session()
        .read_batch(b1)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_split".into(),
            msg: format!("read_batch(1): {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df0);
    res.insert(1, df1);
    Ok(res)
}

mod train_test_split;
pub use train_test_split::TrainTestSplitFactory;

mod k_fold;
pub use k_fold::KFoldFactory;

mod stratified_k_fold;
pub use stratified_k_fold::StratifiedKFoldFactory;

mod group_k_fold;
pub use group_k_fold::GroupKFoldFactory;

// ═══════════════════════════════════════════════════════════════════════
// Shared batch helpers
// ═══════════════════════════════════════════════════════════════════════

fn select_rows(batches: &[RecordBatch], indices: &[usize]) -> Result<RecordBatch, DagError> {
    use arrow::datatypes::SchemaRef;
    use arrow_array::RecordBatchOptions;
    use arrow_select::interleave::interleave;

    let schema: SchemaRef = batches
        .first()
        .ok_or(DagError::NodeError {
            node_type: "ml_split".into(),
            msg: "no input rows".into(),
        })?
        .schema();

    let n_cols = schema.fields().len();
    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(n_cols);
    for col_i in 0..n_cols {
        // Gather column from all batches
        let col_arrays: Vec<&dyn Array> =
            batches.iter().map(|b| b.column(col_i).as_ref()).collect();
        let indices_pairs: Vec<(usize, usize)> = indices
            .iter()
            .map(|&global_idx| {
                // Find batch + offset
                let mut offset = global_idx;
                for (batch_i, b) in batches.iter().enumerate() {
                    if offset < b.num_rows() {
                        return (batch_i, offset);
                    }
                    offset -= b.num_rows();
                }
                (0, 0)
            })
            .collect();
        let result = interleave(&col_arrays, &indices_pairs).map_err(|e| DagError::NodeError {
            node_type: "ml_split".into(),
            msg: format!("row selection: {e}"),
        })?;
        arrays.push(result);
    }

    RecordBatch::try_new_with_options(schema, arrays, &RecordBatchOptions::default()).map_err(|e| {
        DagError::NodeError {
            node_type: "ml_split".into(),
            msg: format!("build batch: {e}"),
        }
    })
}

fn append_column(
    batches: &[RecordBatch],
    name: &str,
    array: Arc<dyn Array>,
    dtype: DataType,
) -> Result<RecordBatch, DagError> {
    let (_schema, mut fields, mut arrays) = common::concat_input(batches)?;
    fields.push(Arc::new(Field::new(name, dtype, true)));
    arrays.push(array);
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(),
        msg: format!("append column: {e}"),
    })
}
