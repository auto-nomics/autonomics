//! Shared numeric-column extraction and validation utilities for statkit/epi
//! nodes.
//!
//! Centralising Arrow → `Vec<f64>` conversion here ensures consistent null
//! handling, type checking, and error reporting across every node that
//! consumes numeric columns. Previously each node (e.g. `linear_regression`)
//! inlined its own copy of this logic.

use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::DataType;

// ── Errors ─────────────────────────────────────────────────────────────────

/// Error type for column-level operations. Each variant carries enough context
/// to produce a user-actionable error message (which column, what type).
#[derive(Debug)]
pub enum ColumnError {
    /// The named column does not exist in the schema.
    Missing { name: String },
    /// The column exists but its Arrow data type is not supported by the
    /// requesting operation.
    WrongType {
        name: String,
        dtype: String,
        expected: &'static str,
    },
    /// The column exists and is numeric, but one or more values are null
    /// after filtering. `n_null` counts nulls.
    HasNulls { name: String, n_null: usize },
    /// A binary column contains values other than 0/1.
    NotBinary { name: String, bad_value: f64 },
    /// The input stream produced zero rows.
    Empty,
}

impl std::fmt::Display for ColumnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColumnError::Missing { name } => {
                write!(f, "column '{name}' not found in input schema")
            }
            ColumnError::WrongType {
                name,
                dtype,
                expected,
            } => {
                write!(f, "column '{name}' has type {dtype}, expected {expected}")
            }
            ColumnError::HasNulls { name, n_null } => {
                write!(
                    f,
                    "column '{name}' has {n_null} null value(s); drop or impute before analysis"
                )
            }
            ColumnError::NotBinary { name, bad_value } => {
                write!(
                    f,
                    "column '{name}' must be binary (0/1), found value {bad_value}"
                )
            }
            ColumnError::Empty => write!(f, "no input rows"),
        }
    }
}

impl std::error::Error for ColumnError {}

// ── Numeric column extraction ──────────────────────────────────────────────

/// Return the Arrow column index of `name`, or a descriptive error.
pub fn column_index(batches: &[RecordBatch], name: &str) -> Result<usize, ColumnError> {
    let schema = batches.first().ok_or(ColumnError::Empty)?.schema();
    schema.index_of(name).map_err(|_| ColumnError::Missing {
        name: name.to_string(),
    })
}

/// Return the Arrow [`DataType`] of the named column, or an error if the
/// column is missing.
pub fn column_dtype(batches: &[RecordBatch], name: &str) -> Result<DataType, ColumnError> {
    let idx = column_index(batches, name)?;
    Ok(batches[0].schema().field(idx).data_type().clone())
}

/// Extract a named numeric column from `batches` into `Vec<f64>`, **dropping
/// rows where the value is null** and returning the surviving row indices.
///
/// Use this when the node's analysis cannot tolerate NaN values (logistic
/// regression, LASSO, WQS). The returned indices allow the caller to align
/// other columns.
///
/// Returns `(values, kept_indices)`.
pub fn extract_numeric_strict(
    batches: &[RecordBatch],
    name: &str,
) -> Result<(Vec<f64>, Vec<usize>), ColumnError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !is_numeric(&dtype) {
        return Err(ColumnError::WrongType {
            name: name.to_string(),
            dtype: dtype.to_string(),
            expected: "numeric (int* or float*)",
        });
    }

    let mut values = Vec::new();
    let mut kept = Vec::new();
    let mut null_count = 0usize;
    let mut global_row = 0usize;

    for batch in batches {
        let col = batch.column(idx);
        extract_numeric_dispatch(col, &mut |v: Option<f64>| {
            match v {
                Some(x) => {
                    values.push(x);
                    kept.push(global_row);
                }
                None => null_count += 1,
            }
            global_row += 1;
        });
    }

    if values.is_empty() && null_count > 0 {
        return Err(ColumnError::HasNulls {
            name: name.to_string(),
            n_null: null_count,
        });
    }

    Ok((values, kept))
}

/// Extract a numeric column, converting nulls to `NaN`. Use only when the
/// downstream algorithm handles NaN (e.g. OLS filtering).
pub fn extract_numeric_lenient(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<f64>, ColumnError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
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

/// Extract a binary (0/1) column, **rejecting nulls and non-binary values**.
///
/// Returns `(values_as_f64, kept_indices)` — values are guaranteed to be
/// exactly 0.0 or 1.0.
pub fn extract_binary_strict(
    batches: &[RecordBatch],
    name: &str,
) -> Result<(Vec<f64>, Vec<usize>), ColumnError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();

    let mut values = Vec::new();
    let mut kept = Vec::new();
    let mut global_row = 0usize;

    // Support both boolean and numeric columns.
    if dtype == DataType::Boolean {
        for batch in batches {
            if let Some(arr) = batch.column(idx).as_any().downcast_ref::<BooleanArray>() {
                for v in arr.iter() {
                    match v {
                        Some(b) => {
                            values.push(if b { 1.0 } else { 0.0 });
                            kept.push(global_row);
                        }
                        None => {}
                    }
                    global_row += 1;
                }
            }
        }
    } else if is_numeric(&dtype) {
        for batch in batches {
            let col = batch.column(idx);
            extract_numeric_dispatch(col, &mut |v: Option<f64>| {
                match v {
                    Some(x) if x == 0.0 || x == 1.0 => {
                        values.push(x);
                        kept.push(global_row);
                    }
                    Some(x) => {
                        // Non-binary value — record NaN sentinel for later detection.
                        let _ = x;
                        values.push(f64::NAN);
                        kept.push(global_row);
                    }
                    None => {}
                }
                global_row += 1;
            });
        }
        // Validate no bad values slipped through.
        if let Some(&bad) = values.iter().find(|&&v| v.is_nan()) {
            // Find the actual bad value — re-scan is overkill for an error path.
            // Report the NaN sentinel as evidence of non-binary data.
            return Err(ColumnError::NotBinary {
                name: name.to_string(),
                bad_value: bad,
            });
        }
    } else {
        return Err(ColumnError::WrongType {
            name: name.to_string(),
            dtype: dtype.to_string(),
            expected: "boolean or numeric binary (0/1)",
        });
    }

    if values.is_empty() {
        return Err(ColumnError::Empty);
    }
    Ok((values, kept))
}

/// Extract a categorical/string column into `Vec<String>`, for use in
/// cross-tabulation (χ² test).
pub fn extract_string_column(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<String>, ColumnError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();

    use arrow_array::LargeStringArray;
    match dtype {
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {}
        _ => {
            return Err(ColumnError::WrongType {
                name: name.to_string(),
                dtype: dtype.to_string(),
                expected: "string (Utf8/LargeUtf8/Utf8View)",
            });
        }
    }

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        // Handle Utf8View (DataFusion ≥42), Utf8, and LargeUtf8.
        if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringArray>() {
            for v in arr.iter() {
                values.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
            for v in arr.iter() {
                values.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringViewArray>() {
            for v in arr.iter() {
                values.push(v.unwrap_or("").to_string());
            }
        }
    }
    Ok(values)
}

// ── Cross-tabulation ───────────────────────────────────────────────────────

/// Build an r×c contingency table (as `Vec<Vec<u64>>`) from two string
/// columns. Row labels and column labels are returned alongside.
///
/// Used by the χ² node to convert raw observations into the matrix expected
/// by `epi::chisq::chi_squared_test`.
pub fn crosstab(
    row_col: &[String],
    col_col: &[String],
) -> (Vec<Vec<u64>>, Vec<String>, Vec<String>) {
    use std::collections::BTreeMap;
    // BTreeMap for deterministic ordering of categories.
    let mut row_cats: BTreeMap<String, usize> = BTreeMap::new();
    let mut col_cats: BTreeMap<String, usize> = BTreeMap::new();

    for (r, c) in row_col.iter().zip(col_col) {
        row_cats.entry(r.clone()).or_default();
        col_cats.entry(c.clone()).or_default();
    }

    let row_labels: Vec<String> = row_cats.keys().cloned().collect();
    let col_labels: Vec<String> = col_cats.keys().cloned().collect();
    for (i, k) in row_labels.iter().enumerate() {
        row_cats.insert(k.clone(), i);
    }
    for (i, k) in col_labels.iter().enumerate() {
        col_cats.insert(k.clone(), i);
    }

    let mut counts = vec![vec![0u64; col_labels.len()]; row_labels.len()];
    for (r, c) in row_col.iter().zip(col_col) {
        let ri = row_cats[r];
        let ci = col_cats[c];
        counts[ri][ci] += 1;
    }

    (counts, row_labels, col_labels)
}

/// Intersect two sets of kept-row indices, returning only the indices present
/// in both. Used when multiple columns are extracted with null-dropping and
/// we need the common rows.
pub fn intersect_kept(a: &[usize], b: &[usize]) -> Vec<usize> {
    let set_b: std::collections::HashSet<usize> = b.iter().copied().collect();
    a.iter().copied().filter(|i| set_b.contains(i)).collect()
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn is_numeric(dtype: &DataType) -> bool {
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

/// Dispatch on Arrow array type, calling `emit` for each element.
fn extract_numeric_dispatch(col: &dyn Array, emit: &mut impl FnMut(Option<f64>)) {
    macro_rules! cast {
        ($arr:expr, $T:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    emit(v.map(|val| val as f64));
                }
                return;
            }
        };
    }
    cast!(col, Int8Array);
    cast!(col, Int16Array);
    cast!(col, Int32Array);
    cast!(col, Int64Array);
    cast!(col, UInt8Array);
    cast!(col, UInt16Array);
    cast!(col, UInt32Array);
    cast!(col, UInt64Array);
    cast!(col, Float32Array);
    cast!(col, Float64Array);
    // Float16 is uncommon; fall through silently (treated as null).
    for _ in 0..col.len() {
        emit(None);
    }
}

// ── DagError integration ───────────────────────────────────────────────────

/// Allow `?` to convert [`ColumnError`] directly into [`DagError`] inside node
/// `execute()` methods. Uses a generic `"input"` node_type since column errors
/// are shared across nodes. Nodes that want a more specific node_type in the
/// error should `.map_err(NodeSpecificError::from)?` before the `DagError`
/// conversion.
impl From<ColumnError> for crate::dag::DagError {
    fn from(e: ColumnError) -> Self {
        crate::dag::DagError::NodeError {
            node_type: "input".to_string(),
            msg: e.to_string(),
        }
    }
}
