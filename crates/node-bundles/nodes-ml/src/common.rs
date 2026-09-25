//! Shared helpers for ML DAG nodes — Arrow ↔ faer matrix conversion,
//! result-batch construction.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, FieldRef, Schema};
use faer::Mat;

use dag_core::dag::DagError;

use dag_core::arrow_util::ColumnError;

/// Create a `Mat<f64>` from row-major data (faer 0.24 lacks from_row_major).
pub fn mat_from_row_major(nrows: usize, ncols: usize, data: &[f64]) -> Mat<f64> {
    Mat::from_fn(nrows, ncols, |i, j| data[i * ncols + j])
}

/// Extract multiple named numeric columns into a row-major `Mat<f64>`.
///
/// Null values are replaced with `NaN`.
pub fn extract_matrix(
    batches: &[RecordBatch],
    columns: &[String],
) -> Result<Mat<f64>, ColumnError> {
    let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    let n_cols = columns.len();
    let mut data = vec![0.0f64; n_rows * n_cols];

    for (col_j, col_name) in columns.iter().enumerate() {
        let values = extract_numeric_column(batches, col_name)?;
        for (row_i, &v) in values.iter().enumerate() {
            data[row_i * n_cols + col_j] = v;
        }
    }

    Ok(mat_from_row_major(n_rows, n_cols, &data))
}

/// Extract a single numeric column as `Vec<f64>`, nulls → NaN.
pub fn extract_numeric_column(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<f64>, ColumnError> {
    let schema = batches.first().ok_or(ColumnError::Empty)?.schema();
    let idx = schema.index_of(name).map_err(|_| ColumnError::Missing {
        name: name.to_string(),
    })?;
    let dtype = schema.field(idx).data_type().clone();
    if !is_numeric(&dtype) {
        return Err(ColumnError::WrongType {
            name: name.to_string(),
            dtype: dtype.to_string(),
            expected: "numeric",
        });
    }

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        extract_numeric_dispatch(col, &mut |v: Option<f64>| {
            values.push(v.unwrap_or(f64::NAN))
        });
    }
    Ok(values)
}

/// Extract a single string column as `Vec<String>`, nulls → empty string.
pub fn extract_string_column(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, DagError> {
    let batch0 = batches.first().ok_or_else(|| DagError::NodeError {
        node_type: "ml".into(),
        msg: "no input rows".into(),
    })?;
    let schema = batch0.schema();
    let idx = schema.index_of(name).map_err(|_| DagError::NodeError {
        node_type: "ml".into(),
        msg: format!("column '{name}' not found"),
    })?;

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
            for v in arr.iter() {
                values.push(v.unwrap_or("").to_string());
            }
        } else {
            // Fallback: try to stringify via Display
            for i in 0..col.len() {
                if col.is_null(i) {
                    values.push(String::new());
                } else {
                    values.push(format!("{:?}", col));
                }
            }
        }
    }
    Ok(values)
}

/// Canonical string keys for a string-or-numeric group column, concatenated
/// across batches in row order.
///
/// Strings pass through verbatim; numerics render via Arrow's display
/// formatting so mixed numeric types compare consistently.  Grouping nodes
/// rank these keys lexicographically into dense ids.
pub fn group_keys(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, String> {
    let mut keys: Vec<String> = Vec::new();
    for batch in batches {
        let idx = batch
            .schema()
            .index_of(name)
            .map_err(|_| format!("group column '{name}' not found"))?;
        let col = batch.column(idx);
        match col.data_type() {
            DataType::Utf8 => {
                let s = col
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or("group column cast failed")?;
                for i in 0..s.len() {
                    keys.push(s.value(i).to_string());
                }
            }
            DataType::LargeUtf8 => {
                let s = col
                    .as_any()
                    .downcast_ref::<arrow_array::LargeStringArray>()
                    .ok_or("group column cast failed")?;
                for i in 0..s.len() {
                    keys.push(s.value(i).to_string());
                }
            }
            DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64 => {
                for i in 0..col.len() {
                    let key = arrow::util::display::array_value_to_string(col, i)
                        .map_err(|e| format!("group value: {e}"))?;
                    keys.push(key);
                }
            }
            other => {
                return Err(format!(
                    "group column must be string or numeric, got {other}"
                ));
            }
        }
    }
    Ok(keys)
}

/// Dense `0..n_groups` ids following lexicographic order of the keys,
/// independent of row order (two passes: collect the set, then map).
pub fn dense_group_ids(keys: &[String]) -> Vec<usize> {
    let sorted: std::collections::BTreeSet<&str> = keys.iter().map(|s| s.as_str()).collect();
    let rank: std::collections::HashMap<&str, usize> = sorted
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    keys.iter().map(|k| rank[k.as_str()]).collect()
}

/// Check if an Arrow data type is numeric.
pub fn is_numeric(dtype: &DataType) -> bool {
    matches!(
        dtype,
        DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    )
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

/// Concatenate all input batches' columns into single per-column arrays.
///
/// ML nodes receive a `Vec<RecordBatch>` that may contain multiple chunks
/// (Arrow's default batch size is 8,192 rows).  Building the output
/// `RecordBatch` from only `batches[0]` produces columns whose length is
/// shorter than the prediction/probability vectors computed over **all**
/// rows, causing:
///
/// ```text
/// Invalid argument error: all columns in a record batch must have the same length
/// ```
///
/// This helper concatenates every column across all batches so the returned
/// arrays are the full row count.  It also returns the shared schema and the
/// cloned field list, which is what every node needs to build its output batch.
pub fn concat_input(
    batches: &[RecordBatch],
) -> Result<(Arc<Schema>, Vec<FieldRef>, Vec<Arc<dyn Array>>), DagError> {
    let batch0 = batches.first().ok_or_else(|| DagError::NodeError {
        node_type: "ml".into(),
        msg: "no input rows".into(),
    })?;
    let schema = batch0.schema();
    let n_cols = schema.fields().len();

    let mut arrays = Vec::with_capacity(n_cols);
    for col_idx in 0..n_cols {
        let chunks: Vec<&dyn Array> = batches.iter().map(|b| b.column(col_idx).as_ref()).collect();
        let combined = arrow_select::concat::concat(&chunks).map_err(|e| DagError::NodeError {
            node_type: "ml".into(),
            msg: format!("concat input columns: {e}"),
        })?;
        arrays.push(combined);
    }

    let fields: Vec<FieldRef> = schema.fields().iter().cloned().collect();
    Ok((schema, fields, arrays))
}
