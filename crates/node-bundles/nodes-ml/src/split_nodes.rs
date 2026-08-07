//! Split & cross-validation DAG nodes.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

// ── helpers ──────────────────────────────────────────────────────────────

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_split".into(), msg: "no input".into()
    })?;
    input.data.clone().collect().await.map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(), msg: format!("collect: {e}"),
    })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx.session().read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(), msg: format!("read_batch: {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

fn emit_two_batches(ctx: &NodeCtx, b0: RecordBatch, b1: RecordBatch) -> Result<PortOutputs, DagError> {
    let df0 = ctx.session().read_batch(b0).map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(), msg: format!("read_batch(0): {e}"),
    })?;
    let df1 = ctx.session().read_batch(b1).map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(), msg: format!("read_batch(1): {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df0);
    res.insert(1, df1);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// TrainTestSplit
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TrainTestSplitSpec {
    /// Stratify column (optional). If set, class proportions are preserved.
    #[serde(default)]
    pub stratify_column: Option<String>,
    /// Fraction allocated to the test set, in (0, 1). Default 0.2.
    #[serde(default = "d_test_size")]
    pub test_size: f64,
    /// Random seed. Default 42.
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_test_size() -> f64 { 0.2 }
fn d_seed() -> u64 { 42 }

pub struct TrainTestSplitFactory;
impl NodeFactory for TrainTestSplitFactory {
    fn kind(&self) -> &'static str { "ml_train_test_split" }
    fn desc(&self) -> &'static str { "Split data into train and test subsets." }
    fn doc(&self) -> &'static str { "TrainTestSplit: randomly partitions rows into train (port 0) and test (port 1) sets. Supports stratified splitting to preserve class proportions." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(TrainTestSplitSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TrainTestSplitSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TrainTestSplitNode {
            stratify_column: s.stratify_column, test_size: s.test_size, seed: s.seed, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TrainTestSplitNode {
    stratify_column: Option<String>, test_size: f64, seed: u64, meta: NodePorts,
}

#[async_trait]
impl DagNode for TrainTestSplitNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_train_test_split" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let n: usize = batches.iter().map(|b| b.num_rows()).sum();

        let stratify_labels = match &self.stratify_column {
            Some(col) => {
                let vals = common::extract_numeric_column(&batches, col).map_err(|e| DagError::NodeError {
                    node_type: "ml_train_test_split".into(), msg: e.to_string()
                })?;
                Some(vals.into_iter().map(|v| v as usize).collect::<Vec<_>>())
            }
            None => None,
        };

        let split = ml::split::train_test_split(n, self.test_size, stratify_labels.as_deref(), self.seed)
            .map_err(|e| DagError::NodeError { node_type: "ml_train_test_split".into(), msg: e.to_string() })?;

        let train_batch = select_rows(&batches, &split.train_indices)?;
        let test_batch = select_rows(&batches, &split.test_indices)?;
        emit_two_batches(ctx, train_batch, test_batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// KFold
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KFoldSpec {
    pub k: usize,
    #[serde(default = "d_shuffle")]
    pub shuffle: bool,
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_shuffle() -> bool { true }

pub struct KFoldFactory;
impl NodeFactory for KFoldFactory {
    fn kind(&self) -> &'static str { "ml_kfold" }
    fn desc(&self) -> &'static str { "K-Fold cross-validation fold assignment." }
    fn doc(&self) -> &'static str { "KFold: assigns each row to one of K folds (0..K-1) via a new `fold` column. Downstream nodes can group by fold for CV." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(KFoldSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: KFoldSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KFoldNode { k: s.k, shuffle: s.shuffle, seed: s.seed, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct KFoldNode { k: usize, shuffle: bool, seed: u64, meta: NodePorts }

#[async_trait]
impl DagNode for KFoldNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_kfold" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let n: usize = batches.iter().map(|b| b.num_rows()).sum();
        let folds = ml::split::kfold(n, self.k, self.shuffle, self.seed)
            .map_err(|e| DagError::NodeError { node_type: "ml_kfold".into(), msg: e.to_string() })?;

        // Assign fold labels
        let mut fold_labels = vec![0u32; n];
        for (fold_idx, (_, test)) in folds.iter().enumerate() {
            for &i in test { fold_labels[i] = fold_idx as u32; }
        }

        let batch = append_column(&batches, "fold", Arc::new(UInt32Array::from(fold_labels)), DataType::UInt32)?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// StratifiedKFold
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StratifiedKFoldSpec {
    /// Column containing class labels for stratification.
    pub label_column: String,
    pub k: usize,
    #[serde(default = "d_shuffle")]
    pub shuffle: bool,
    #[serde(default = "d_seed")]
    pub seed: u64,
}

pub struct StratifiedKFoldFactory;
impl NodeFactory for StratifiedKFoldFactory {
    fn kind(&self) -> &'static str { "ml_stratified_kfold" }
    fn desc(&self) -> &'static str { "Stratified K-Fold: preserves class proportions per fold." }
    fn doc(&self) -> &'static str { "StratifiedKFold: assigns folds such that each fold maintains the same class proportion as the full dataset. Requires a label column." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(StratifiedKFoldSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: StratifiedKFoldSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StratifiedKFoldNode {
            label_column: s.label_column, k: s.k, shuffle: s.shuffle, seed: s.seed, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct StratifiedKFoldNode { label_column: String, k: usize, shuffle: bool, seed: u64, meta: NodePorts }

#[async_trait]
impl DagNode for StratifiedKFoldNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_stratified_kfold" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let n: usize = batches.iter().map(|b| b.num_rows()).sum();
        let labels = common::extract_numeric_column(&batches, &self.label_column).map_err(|e| DagError::NodeError {
            node_type: "ml_stratified_kfold".into(), msg: e.to_string()
        })?;
        let labels_usize: Vec<usize> = labels.into_iter().map(|v| v as usize).collect();
        let folds = ml::split::stratified_kfold(&labels_usize, self.k, self.shuffle, self.seed)
            .map_err(|e| DagError::NodeError { node_type: "ml_stratified_kfold".into(), msg: e.to_string() })?;

        let mut fold_labels = vec![0u32; n];
        for (fold_idx, (_, test)) in folds.iter().enumerate() {
            for &i in test { fold_labels[i] = fold_idx as u32; }
        }

        let batch = append_column(&batches, "fold", Arc::new(UInt32Array::from(fold_labels)), DataType::UInt32)?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shared batch helpers
// ═══════════════════════════════════════════════════════════════════════

fn select_rows(batches: &[RecordBatch], indices: &[usize]) -> Result<RecordBatch, DagError> {
    use arrow::datatypes::SchemaRef;
    use arrow_array::RecordBatchOptions;
    use arrow_select::interleave::interleave;

    let schema: SchemaRef = batches.first().ok_or(DagError::NodeError {
        node_type: "ml_split".into(), msg: "no input rows".into()
    })?.schema();

    let n_cols = schema.fields().len();
    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(n_cols);
    for col_i in 0..n_cols {
        // Gather column from all batches
        let col_arrays: Vec<&dyn Array> = batches.iter().map(|b| b.column(col_i).as_ref()).collect();
        let indices_pairs: Vec<(usize, usize)> = indices.iter().map(|&global_idx| {
            // Find batch + offset
            let mut offset = global_idx;
            for (batch_i, b) in batches.iter().enumerate() {
                if offset < b.num_rows() {
                    return (batch_i, offset);
                }
                offset -= b.num_rows();
            }
            (0, 0)
        }).collect();
        let result = interleave(&col_arrays, &indices_pairs).map_err(|e| DagError::NodeError {
            node_type: "ml_split".into(), msg: format!("row selection: {e}"),
        })?;
        arrays.push(result);
    }

    RecordBatch::try_new_with_options(
        schema,
        arrays,
        &RecordBatchOptions::default(),
    ).map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(), msg: format!("build batch: {e}"),
    })
}

fn append_column(
    batches: &[RecordBatch],
    name: &str,
    array: Arc<dyn Array>,
    dtype: DataType,
) -> Result<RecordBatch, DagError> {
    let schema = batches.first().ok_or(DagError::NodeError {
        node_type: "ml_split".into(), msg: "no input rows".into()
    })?.schema();
    let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
    let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
        .map(|i| batches.first().unwrap().column(i).clone())
        .collect();
    fields.push(Arc::new(Field::new(name, dtype, true)));
    arrays.push(array);
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_split".into(), msg: format!("append column: {e}"),
    })
}
