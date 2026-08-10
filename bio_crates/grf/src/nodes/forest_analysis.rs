//! DAG nodes for forest introspection:
//! - `grf_get_forest_weights` — α(test_row, train_row) co-leaf counts.
//! - `grf_split_frequencies` — split depth × feature matrix.
//! - `grf_variable_importance` — weighted sum across depths.
//! - `grf_get_tree` — extract one tree (serialized).
//! - `grf_merge_forests` — concatenate N forests into one.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestKind, ForestStats};
use crate::nodes::regression_forest::arrow_batches_to_matrix;
use crate::{GrfError, Result};
use grf_sys as sys;

// ═══════════════════════════════════════════════════════════════════════
// get_forest_weights
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetForestWeightsSpec {
    /// Optional override for `num.threads`.
    #[serde(default)]
    pub num_threads: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForestWeightsOutput {
    /// Column-major buffer of shape (n_train × n_test) flattened.
    pub weights: Vec<f64>,
    pub n_train: usize,
    pub n_test: usize,
}

pub struct GetForestWeightsFactory;

impl GetForestWeightsFactory {
    pub fn kind() -> &'static str { "grf_get_forest_weights" }
}

impl GetForestWeightsSpec {
    pub fn compute(&self, forest: &ForestBlob, train_x: Matrix, test_x: Matrix) -> Result<ForestWeightsOutput> {
        let nt = self.num_threads;
        let buf = forest.compute_weights(
            &train_x.data, train_x.n_rows, train_x.n_cols,
            &test_x.data, test_x.n_rows, test_x.n_cols,
            nt,
        ).ok_or_else(|| GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestWeightsOutput {
            weights: buf,
            n_train: train_x.n_rows,
            n_test: test_x.n_rows,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// split_frequencies
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SplitFrequenciesSpec {
    pub max_depth: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SplitFrequenciesOutput {
    /// Row `d`, column `f`: number of times feature `f` was split on at depth `d`.
    pub depths_x_features: Vec<Vec<u64>>,
}

pub struct SplitFrequenciesFactory;

impl SplitFrequenciesFactory {
    pub fn kind() -> &'static str { "grf_split_frequencies" }
}

impl SplitFrequenciesSpec {
    pub fn compute(&self, forest: &ForestBlob) -> Result<SplitFrequenciesOutput> {
        let raw = forest.compute_split_frequencies(self.max_depth)
            .ok_or_else(|| GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(SplitFrequenciesOutput { depths_x_features: raw })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// variable_importance
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VariableImportanceSpec {
    pub max_depth: usize,
    #[serde(default = "default_decay")]
    pub decay_exponent: f64,
}

fn default_decay() -> f64 { 2.0 }

#[derive(Debug, Clone, Serialize)]
pub struct VariableImportanceOutput {
    pub importance: Vec<f64>,
    pub stats: ForestStats,
}

pub struct VariableImportanceFactory;

impl VariableImportanceFactory {
    pub fn kind() -> &'static str { "grf_variable_importance" }
}

impl VariableImportanceSpec {
    pub fn compute(&self, forest: &ForestBlob) -> Result<VariableImportanceOutput> {
        let raw = forest.compute_split_frequencies(self.max_depth)
            .ok_or_else(|| GrfError::Sys(sys::GrfError::NullHandle))?;
        let n_features = raw.first().map(|r| r.len()).unwrap_or(0);
        let max_depth = raw.len();
        // Per-row total (so each row sums to 1).
        let mut weights = Vec::with_capacity(max_depth);
        for row in &raw {
            let total = row.iter().map(|&c| c as f64).sum::<f64>().max(1.0);
            weights.push(row.iter().map(|&c| c as f64 / total).collect::<Vec<_>>());
        }
        // depth decay weights (smaller depth ⇒ larger weight).
        let decay: Vec<f64> = (1..=max_depth).map(|d| 1.0 / (d as f64).powf(self.decay_exponent)).collect();
        let total_decay: f64 = decay.iter().sum();
        let mut importance = vec![0.0; n_features];
        for (d, row) in weights.iter().enumerate() {
            for (f, &w) in row.iter().enumerate() {
                importance[f] += w * decay[d];
            }
        }
        for v in &mut importance {
            *v /= total_decay;
        }
        Ok(VariableImportanceOutput {
            importance,
            stats: ForestStats::from(forest),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// get_tree
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetTreeSpec {
    pub index: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GetTreeOutput {
    /// Serialized tree (binary blob, grf-internal format). The DAG layer
    /// emits this as a column value or external artifact.
    pub serialized: Vec<u8>,
    pub index: usize,
}

pub struct GetTreeFactory;

impl GetTreeFactory {
    pub fn kind() -> &'static str { "grf_get_tree" }
}

impl GetTreeSpec {
    pub fn extract(&self, forest: &ForestBlob) -> Result<GetTreeOutput> {
        // grf R's get_tree produces an R list of nodes; here we return the
        // serialized tree blob for the requested index by slicing the
        // overall forest blob. This is a fast path — deserialize would
        // be more expensive.
        // For correctness, we instead expose a per-tree serialize call:
        // grf-sys exposes the whole forest only. To support per-tree
        // extraction we need a new C ABI entry; defer that and return
        // the whole forest blob for now with a marker.
        Err(GrfError::Missing(
            "per-tree extraction requires grf-sys extension; use serialize() \
             and deserialize with a single-tree shim (TODO)".into()))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// merge_forests
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MergeForestsSpec {
    /// Forested-blob entries — caller (the DAG layer) wires multiple
    /// forest-blob ports into this node.
    pub forests: Vec<Vec<u8>>,
    /// Expected kind of every input forest (must match).
    pub kind: String,
    pub n_features: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeForestsOutput {
    pub forest: ForestBlob,
    pub stats: ForestStats,
}

pub struct MergeForestsFactory;

impl MergeForestsFactory {
    pub fn kind() -> &'static str { "grf_merge_forests" }
}

impl MergeForestsSpec {
    pub fn merge(&self) -> Result<MergeForestsOutput> {
        let kind = ForestKind::from_str(&self.kind)
            .ok_or_else(|| GrfError::Shape(format!("unknown forest kind: {}", self.kind)))?;
        let _ = kind;  // used to drive dispatch on the merged blob below
        // Concatenate all the trees across forests into a single blob.
        // grf's Forest::merge is in the C++ core but not yet exposed via
        // grf-sys. For now, deserialize each, then re-serialize the
        // concatenation (lose the split, but byte-compatible with a single
        // trained forest's blob layout).
        // TODO: add grf_forest_merge C ABI entry.
        if self.forests.is_empty() {
            return Err(GrfError::Missing("empty forests list".into()));
        }
        // Concatenate serialized blobs (simple version: concatenate the
        // raw bytes; grf core's deserialize handles arbitrary blobs but
        // only the first forest's trees. Need the merge C ABI to do this
        // properly.)
        let mut combined = Vec::new();
        for f in &self.forests {
            combined.extend_from_slice(f);
        }
        Err(GrfError::Missing(
            "merge_forests requires grf-sys Forest::merge extension; \
             concat-blob is a placeholder".into()))
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(GetForestWeightsSpec) }

#[allow(dead_code)]
fn _batch_marker(_: &[RecordBatch]) {}