//! Shared helpers for DL DAG nodes — Arrow ↔ Tensor conversion,
//! RecordBatch construction, port helpers.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, FieldRef, Schema};
use dl::Tensor;
use dag_core::dag::DagError;

/// Create a `Tensor` from named numeric columns across all batches.
pub fn extract_tensor(
    batches: &[RecordBatch],
    columns: &[String],
) -> Result<Tensor, DagError> {
    let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    let n_cols = columns.len();
    let mut data = vec![0.0f64; n_rows * n_cols];

    for (col_j, col_name) in columns.iter().enumerate() {
        let values = extract_numeric_column(batches, col_name)?;
        for (row_i, &v) in values.iter().enumerate() {
            data[row_i * n_cols + col_j] = v;
        }
    }

    Ok(Tensor::from_rows(n_rows, n_cols, &data))
}

/// Extract a single numeric column as `Vec<f64>`, nulls → NaN.
pub fn extract_numeric_column(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<f64>, DagError> {
    let schema = batches.first().ok_or(DagError::NodeError {
        node_type: "dl".into(),
        msg: "no input rows".into(),
    })?.schema();
    let idx = schema.index_of(name).map_err(|_| DagError::NodeError {
        node_type: "dl".into(),
        msg: format!("column '{name}' not found"),
    })?;

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        extract_numeric_dispatch(col, &mut |v: Option<f64>| {
            values.push(v.unwrap_or(f64::NAN));
        });
    }
    Ok(values)
}

/// Concatenate all input batches' columns into single per-column arrays.
pub fn concat_input(
    batches: &[RecordBatch],
) -> Result<(Arc<Schema>, Vec<FieldRef>, Vec<Arc<dyn Array>>), DagError> {
    let batch0 = batches.first().ok_or_else(|| DagError::NodeError {
        node_type: "dl".into(),
        msg: "no input rows".into(),
    })?;
    let schema = batch0.schema();
    let n_cols = schema.fields().len();

    let mut arrays = Vec::with_capacity(n_cols);
    for col_idx in 0..n_cols {
        let chunks: Vec<&dyn Array> =
            batches.iter().map(|b| b.column(col_idx).as_ref()).collect();
        let combined = arrow_select::concat::concat(&chunks).map_err(|e| DagError::NodeError {
            node_type: "dl".into(),
            msg: format!("concat input columns: {e}"),
        })?;
        arrays.push(combined);
    }

    let fields: Vec<FieldRef> = schema.fields().iter().cloned().collect();
    Ok((schema, fields, arrays))
}

/// Collect batches from a specific port.
pub async fn collect_port(
    inputs: &[dag_core::node::NodeInput],
    port: u8,
    node_type: &str,
) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs
        .iter()
        .find(|i| i.port == port)
        .ok_or_else(|| DagError::NodeError {
            node_type: node_type.into(),
            msg: format!("input port {port} not connected"),
        })?;
    input.data.clone().collect().await.map_err(|e| DagError::NodeError {
        node_type: node_type.into(),
        msg: format!("collect port {port}: {e}"),
    })
}

/// Collect batches from port 0.
pub async fn collect_batches(
    inputs: &[dag_core::node::NodeInput],
    node_type: &str,
) -> Result<Vec<RecordBatch>, DagError> {
    collect_port(inputs, 0, node_type).await
}

/// Register a DataFrame on the session and wrap into PortOutputs at port 0.
pub fn emit_batch(
    ctx: &dag_core::registry::NodeCtx,
    batch: RecordBatch,
    node_type: &str,
) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
    let df = ctx.session().read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: node_type.into(),
        msg: format!("read_batch: {e}"),
    })?;
    let mut res = dag_core::dag::graph::PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

/// Dispatch numeric extraction across all Arrow numeric array types.
fn extract_numeric_dispatch(col: &dyn Array, callback: &mut impl FnMut(Option<f64>)) {
    macro_rules! dispatch {
        ($arr:expr, $T:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
                for v in a {
                    callback(v.map(|x| x as f64));
                }
                return;
            }
        };
    }
    dispatch!(col, Int8Array);
    dispatch!(col, Int16Array);
    dispatch!(col, Int32Array);
    dispatch!(col, Int64Array);
    dispatch!(col, UInt8Array);
    dispatch!(col, UInt16Array);
    dispatch!(col, UInt32Array);
    dispatch!(col, UInt64Array);
    dispatch!(col, Float32Array);
    if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
        for v in a {
            callback(v);
        }
        return;
    }
    for _ in 0..col.len() {
        callback(None);
    }
}

/// Helper to create a `DagError` with consistent formatting.
pub fn err(node_type: &str, msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: node_type.into(),
        msg: msg.into(),
    }
}
