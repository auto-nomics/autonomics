//! Shared helpers for the grf node bundle.
//!
//! grf forests are `!Send` (opaque C++ handles), so they cannot travel
//! through the arrow dataflow directly. We transport a trained forest as a
//! **single-row RecordBatch** carrying the serialized grf binary blob plus
//! enough metadata (`ForestKind`, feature count) to rebuild it downstream.
//! Trainer nodes emit this batch on port 0; predict/analysis nodes consume
//! it on port 0 and collect the training data they need from later ports.

use std::sync::Arc;

use arrow_array::{Array, BinaryArray, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{NodeInput, PortId};
use dag_core::registry::NodeCtx;
use grf::forest::{ForestBlob, ForestKind};

/// Columns of the single-row forest-exchange batch.
pub const FOREST_BYTES: &str = "forest_bytes";
pub const FOREST_KIND: &str = "forest_kind";
pub const N_FEATURES: &str = "n_features";

fn dag_err(node: &str, msg: &str) -> DagError {
    DagError::NodeError {
        node_type: node.into(),
        msg: msg.to_string(),
    }
}

/// Arrow schema of the single-row forest-exchange batch.
pub fn forest_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(FOREST_BYTES, DataType::Binary, false),
        Field::new(FOREST_KIND, DataType::Utf8, false),
        Field::new(N_FEATURES, DataType::Int32, false),
    ]))
}

/// Encode a trained forest into a single-row RecordBatch for DAG transport.
pub fn encode_forest(forest: &ForestBlob) -> Result<RecordBatch, DagError> {
    let bytes = forest
        .serialize()
        .map_err(|e| dag_err("grf", &format!("forest serialize: {e}")))?;
    RecordBatch::try_new(
        forest_schema(),
        vec![
            Arc::new(BinaryArray::from(vec![Some(bytes.as_slice())])),
            Arc::new(StringArray::from(vec![forest.kind().as_str()])),
            Arc::new(Int32Array::from(vec![forest.n_features() as i32])),
        ],
    )
    .map_err(|e| dag_err("grf", &format!("forest batch: {e}")))
}

/// Decode a forest from its single-row RecordBatch.
pub fn decode_forest(node: &str, batch: &RecordBatch) -> Result<ForestBlob, DagError> {
    let bytes = get_binary(node, batch, FOREST_BYTES)?;
    let kind_str = get_string(node, batch, FOREST_KIND)?;
    let kind = ForestKind::from_str(kind_str)
        .ok_or_else(|| dag_err(node, &format!("unknown forest kind '{kind_str}'")))?;
    let n_features = get_i32(node, batch, N_FEATURES)? as usize;
    ForestBlob::deserialize(bytes, kind, n_features)
        .map_err(|e| dag_err(node, &format!("forest deserialize: {e}")))
}

// ── batch column readers (single-row forest batch) ─────────────────────

pub fn get_binary<'a>(node: &str, batch: &'a RecordBatch, col: &str) -> Result<&'a [u8], DagError> {
    let arr = batch
        .column_by_name(col)
        .ok_or_else(|| dag_err(node, &format!("missing column '{col}'")))?;
    let arr = arr
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| dag_err(node, &format!("column '{col}' is not Binary")))?;
    if arr.len() != 1 {
        return Err(dag_err(
            node,
            &format!("column '{col}' must have exactly 1 row"),
        ));
    }
    Ok(arr.value(0))
}

pub fn get_string<'a>(node: &str, batch: &'a RecordBatch, col: &str) -> Result<&'a str, DagError> {
    let arr = batch
        .column_by_name(col)
        .ok_or_else(|| dag_err(node, &format!("missing column '{col}'")))?;
    let arr = arr
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| dag_err(node, &format!("column '{col}' is not Utf8")))?;
    Ok(arr.value(0))
}

pub fn get_i32(node: &str, batch: &RecordBatch, col: &str) -> Result<i32, DagError> {
    let arr = batch
        .column_by_name(col)
        .ok_or_else(|| dag_err(node, &format!("missing column '{col}'")))?;
    let arr = arr
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| dag_err(node, &format!("column '{col}' is not Int32")))?;
    Ok(arr.value(0))
}

// ── input collection ───────────────────────────────────────────────────

/// Collect a single input port's DataFrame into its constituent RecordBatches.
pub async fn collect_port(
    node: &str,
    inputs: &[NodeInput],
    port: PortId,
) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs
        .iter()
        .find(|i| i.port == port)
        .ok_or_else(|| dag_err(node, &format!("input port {port} not connected")))?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| dag_err(node, &format!("collect port {port}: {e}")))
}

/// Collect every connected input port, concatenated in port-index order.
pub async fn collect_all(node: &str, inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let mut out = Vec::new();
    for input in inputs {
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| dag_err(node, &format!("collect port {}: {e}", input.port)))?;
        out.extend(batches);
    }
    Ok(out)
}

/// Fetch the forest-exchange batch from port 0 and rebuild the forest.
pub async fn take_forest(node: &str, inputs: &[NodeInput]) -> Result<ForestBlob, DagError> {
    let batches = collect_port(node, inputs, 0).await?;
    let first = batches
        .first()
        .ok_or_else(|| dag_err(node, "empty forest batch"))?;
    decode_forest(node, first)
}

// ── output emission ────────────────────────────────────────────────────

pub fn emit(ctx: &NodeCtx, node: &str, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| dag_err(node, &format!("read_batch: {e}")))?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

pub fn emit_two(
    ctx: &NodeCtx,
    node: &str,
    b0: RecordBatch,
    b1: RecordBatch,
) -> Result<PortOutputs, DagError> {
    let df0 = ctx
        .session()
        .read_batch(b0)
        .map_err(|e| dag_err(node, &format!("read_batch(0): {e}")))?;
    let df1 = ctx
        .session()
        .read_batch(b1)
        .map_err(|e| dag_err(node, &format!("read_batch(1): {e}")))?;
    let mut res = PortOutputs::new();
    res.insert(0, df0);
    res.insert(1, df1);
    Ok(res)
}

// ── f64 vector <-> raw bytes (causal aux exchange) ─────────────────────

/// Pack a `Vec<f64>` as a raw little-endian byte blob (no allocation waste,
/// no extra dependency).
pub fn f64vec_to_bytes(v: &[f64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 8);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Unpack a raw little-endian byte blob back into a `Vec<f64>`.
pub fn bytes_to_f64vec(b: &[u8]) -> Result<Vec<f64>, DagError> {
    if b.len() % 8 != 0 {
        return Err(dag_err("grf", "f64 blob length not a multiple of 8"));
    }
    Ok(b.chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

// ── causal-forest exchange batch ───────────────────────────────────────
//
// The causal analysis nodes (ATE, BLP, test_calibration, get_scores) consume
// a `CausalForestOutput`, not a bare `ForestBlob`. To transport it, the causal
// trainer emits a single-row batch carrying the serialized forest plus the
// aux vectors (y_hat, w_hat, y_orig, w_orig) and the OOB tau predictions.

pub const CAUSAL_Y_HAT: &str = "causal_y_hat";
pub const CAUSAL_W_HAT: &str = "causal_w_hat";
pub const CAUSAL_Y_ORIG: &str = "causal_y_orig";
pub const CAUSAL_W_ORIG: &str = "causal_w_orig";
pub const OOB_BIN: &str = "oob_bin";
pub const OOB_PRED_LENGTH: &str = "oob_pred_length";

/// Causal-exchange schema: base forest columns + aux + OOB.
pub fn causal_forest_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(FOREST_BYTES, DataType::Binary, false),
        Field::new(FOREST_KIND, DataType::Utf8, false),
        Field::new(N_FEATURES, DataType::Int32, false),
        Field::new(CAUSAL_Y_HAT, DataType::Binary, false),
        Field::new(CAUSAL_W_HAT, DataType::Binary, false),
        Field::new(CAUSAL_Y_ORIG, DataType::Binary, false),
        Field::new(CAUSAL_W_ORIG, DataType::Binary, false),
        Field::new(OOB_BIN, DataType::Binary, true),
        Field::new(OOB_PRED_LENGTH, DataType::Int32, false),
    ]))
}

/// Encode a `CausalForestOutput` into a single-row exchange batch.
pub fn encode_causal_forest(out: &grf::nodes::CausalForestOutput) -> Result<RecordBatch, DagError> {
    let oob = out.oob_predictions.as_ref();
    let forest_bytes = out
        .forest
        .serialize()
        .map_err(|e| dag_err("grf", &format!("forest serialize: {e}")))?;
    let (y_orig, w_orig) = out.original_outcomes().unwrap_or((&[], &[]));
    let oob_bytes = oob.map(|o| f64vec_to_bytes(&o.values));
    RecordBatch::try_new(
        causal_forest_schema(),
        vec![
            Arc::new(BinaryArray::from(vec![Some(forest_bytes.as_slice())])),
            Arc::new(StringArray::from(vec![out.forest.kind().as_str()])),
            Arc::new(Int32Array::from(vec![out.forest.n_features() as i32])),
            Arc::new(BinaryArray::from(vec![Some(
                f64vec_to_bytes(&out.y_hat).as_slice(),
            )])),
            Arc::new(BinaryArray::from(vec![Some(
                f64vec_to_bytes(&out.w_hat).as_slice(),
            )])),
            Arc::new(BinaryArray::from(vec![Some(
                f64vec_to_bytes(y_orig).as_slice(),
            )])),
            Arc::new(BinaryArray::from(vec![Some(
                f64vec_to_bytes(w_orig).as_slice(),
            )])),
            Arc::new(BinaryArray::from(vec![oob_bytes.as_deref()])),
            Arc::new(Int32Array::from(vec![
                oob.map(|o| o.pred_length as i32).unwrap_or(0),
            ])),
        ],
    )
    .map_err(|e| dag_err("grf", &format!("causal batch: {e}")))
}

/// Decode a causal-forest exchange batch into a `CausalForestOutput`.
pub fn decode_causal_forest(
    node: &str,
    batch: &RecordBatch,
) -> Result<grf::nodes::CausalForestOutput, DagError> {
    let forest = decode_forest(node, batch)?;
    let y_hat = bytes_to_f64vec(get_binary(node, batch, CAUSAL_Y_HAT)?)?;
    let w_hat = bytes_to_f64vec(get_binary(node, batch, CAUSAL_W_HAT)?)?;
    let y_orig = bytes_to_f64vec(get_binary(node, batch, CAUSAL_Y_ORIG)?)?;
    let w_orig = bytes_to_f64vec(get_binary(node, batch, CAUSAL_W_ORIG)?)?;
    let oob = match get_binary(node, batch, OOB_BIN) {
        Ok(b) if !b.is_empty() => {
            let pred_length = get_i32(node, batch, OOB_PRED_LENGTH)? as usize;
            let values = bytes_to_f64vec(b)?;
            Some(OobPredictions {
                values,
                pred_length,
            })
        }
        _ => None,
    };
    let stats = grf::forest::ForestStats {
        kind: forest.kind(),
        num_trees: forest.num_trees(),
        n_features: forest.n_features(),
        pred_length: oob.as_ref().map(|o| o.pred_length).unwrap_or(1),
        has_oob_predictions: oob.is_some(),
    };
    // Validate length consistency to catch encoding bugs early.
    let n = y_orig.len();
    if w_orig.len() != n || y_hat.len() != n || w_hat.len() != n {
        return Err(dag_err(
            node,
            &format!(
                "causal batch length mismatch: y_orig={}, w_orig={}, y_hat={}, w_hat={}",
                n,
                w_orig.len(),
                y_hat.len(),
                w_hat.len()
            ),
        ));
    }
    Ok(grf::nodes::CausalForestOutput::from_parts(
        forest, y_hat, w_hat, oob, stats, y_orig, w_orig,
    ))
}

// ── matrix from batches (for get_forest_weights) ───────────────────────

use grf::data::Matrix;

/// Build a column-major `Matrix` from every Float64 (or Int64) column of the
/// given batches. Returns the chosen column names so the caller can match
/// training-time features.
pub fn batch_to_matrix(
    node: &str,
    batches: &[RecordBatch],
) -> Result<(Matrix, Vec<String>), DagError> {
    if batches.is_empty() {
        return Err(dag_err(node, "empty batches for matrix"));
    }
    let schema = batches[0].schema();
    let cols: Vec<String> = schema
        .fields()
        .iter()
        .filter(|f| matches!(f.data_type(), DataType::Float64 | DataType::Int64))
        .map(|f| f.name().clone())
        .collect();
    if cols.is_empty() {
        return Err(dag_err(node, "no numeric columns for matrix"));
    }
    let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    let mut buf = vec![0f64; n_rows * cols.len()];
    let mut offset = 0;
    for batch in batches {
        let r = batch.num_rows();
        for (j, name) in cols.iter().enumerate() {
            let arr = batch
                .column_by_name(name)
                .ok_or_else(|| dag_err(node, &format!("column '{name}' not found")))?;
            if let Some(f) = arr.as_any().downcast_ref::<Float64Array>() {
                for i in 0..r {
                    buf[j * n_rows + offset + i] = if f.is_null(i) { f64::NAN } else { f.value(i) };
                }
            } else if let Some(iv) = arr.as_any().downcast_ref::<arrow_array::Int64Array>() {
                for i in 0..r {
                    buf[j * n_rows + offset + i] = if iv.is_null(i) {
                        f64::NAN
                    } else {
                        iv.value(i) as f64
                    };
                }
            } else {
                return Err(dag_err(node, &format!("column '{name}' is not numeric")));
            }
        }
        offset += r;
    }
    Ok((
        Matrix {
            data: buf,
            n_rows,
            n_cols: cols.len(),
        },
        cols,
    ))
}

// ── OOB predictions emission ───────────────────────────────────────────

use grf::forest::OobPredictions;

/// Turn column-major OOB predictions (pred_length × n) into a RecordBatch with
/// one Float64 column per prediction output.
pub fn oob_to_batch(node: &str, oob: &OobPredictions) -> Result<RecordBatch, DagError> {
    let n = if oob.pred_length == 0 {
        0
    } else {
        oob.values.len() / oob.pred_length
    };
    let fields: Vec<Field> = (0..oob.pred_length)
        .map(|k| Field::new(format!("pred_{k}"), DataType::Float64, false))
        .collect();
    let mut cols: Vec<Arc<dyn Array>> = Vec::with_capacity(oob.pred_length);
    for k in 0..oob.pred_length {
        let mut data = Vec::with_capacity(n);
        for i in 0..n {
            data.push(oob.values[k * n + i]);
        }
        cols.push(Arc::new(Float64Array::from(data)));
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
        .map_err(|e| dag_err(node, &format!("oob batch: {e}")))
}

/// A single unused-forecast type marker to keep the import set stable.
pub(crate) fn _kind_marker(_: ForestKind) {}
