//! `dl_train_val_test_split` — three-way data partition.

use std::sync::Arc;

use arrow_array::RecordBatchOptions;
use arrow_array::{Array, RecordBatch};
use arrow_select::interleave::interleave;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::common;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;

const NODE: &str = "dl_train_val_test_split";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TrainValTestSplitSpec {
    /// Fraction of data for the validation set (0–1).
    #[serde(default = "d_val")]
    pub val_size: f64,
    /// Fraction of data for the test set (0–1).
    #[serde(default = "d_test")]
    pub test_size: f64,
    /// Optional stratification column (preserves class proportions).
    #[serde(default)]
    pub stratify_column: Option<String>,
    /// Random seed.
    #[serde(default = "d_seed")]
    pub seed: u64,
}

fn d_val() -> f64 {
    0.15
}
fn d_test() -> f64 {
    0.15
}
fn d_seed() -> u64 {
    42
}

pub struct TrainValTestSplitFactory;
impl NodeFactory for TrainValTestSplitFactory {
    fn kind(&self) -> &'static str {
        NODE
    }
    fn desc(&self) -> &'static str {
        "Three-way split into train/validation/test sets."
    }
    fn doc(&self) -> &'static str {
        "dl_train_val_test_split: partitions data into train (port 0), validation (port 1), \
        and test (port 2) sets. Supports stratified splitting to preserve class/event \
        proportions across all three subsets."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TrainValTestSplitSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None) // 0: train
            .add_output_port(None) // 1: validation
            .add_output_port(None) // 2: test
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TrainValTestSplitSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TrainValTestSplitNode {
            val_size: s.val_size,
            test_size: s.test_size,
            stratify_column: s.stratify_column,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TrainValTestSplitNode {
    val_size: f64,
    test_size: f64,
    stratify_column: Option<String>,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for TrainValTestSplitNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        NODE
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = common::collect_batches(inputs, NODE).await?;
        let n: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n == 0 {
            return Err(common::err(NODE, "empty input"));
        }

        // Stratification labels.
        let stratify: Option<Vec<usize>> = if let Some(ref col) = self.stratify_column {
            let vals = common::extract_numeric_column(&batches, col)?;
            Some(vals.into_iter().map(|v| v as usize).collect())
        } else {
            None
        };

        let (train_idx, val_idx, test_idx) = three_way_split(
            n,
            self.val_size,
            self.test_size,
            stratify.as_deref(),
            self.seed,
        )?;

        let train_batch = select_rows(&batches, &train_idx)?;
        let val_batch = select_rows(&batches, &val_idx)?;
        let test_batch = select_rows(&batches, &test_idx)?;

        let df0 = ctx
            .session()
            .read_batch(train_batch)
            .map_err(|e| common::err(NODE, format!("read_batch(0): {e}")))?;
        let df1 = ctx
            .session()
            .read_batch(val_batch)
            .map_err(|e| common::err(NODE, format!("read_batch(1): {e}")))?;
        let df2 = ctx
            .session()
            .read_batch(test_batch)
            .map_err(|e| common::err(NODE, format!("read_batch(2): {e}")))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        res.insert(2, df2);
        Ok(res)
    }
}

fn three_way_split(
    n: usize,
    val_size: f64,
    test_size: f64,
    stratify: Option<&[usize]>,
    seed: u64,
) -> Result<(Vec<usize>, Vec<usize>, Vec<usize>), DagError> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    let (train, val, test) = match stratify {
        None => {
            let mut indices: Vec<usize> = (0..n).collect();
            indices.shuffle(&mut rng);

            let n_test = (n as f64 * test_size).round() as usize;
            let n_val = (n as f64 * val_size).round() as usize;

            let test = indices.split_off(n - n_test);
            let val = indices.split_off(n - n_test - n_val);
            (indices, val, test)
        }
        Some(labels) => {
            let mut classes: std::collections::HashMap<usize, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, &label) in labels.iter().enumerate() {
                classes.entry(label).or_default().push(i);
            }

            let mut train = Vec::new();
            let mut val = Vec::new();
            let mut test = Vec::new();

            for (_, mut indices) in classes {
                indices.shuffle(&mut rng);
                let nc = indices.len();
                let n_test = (nc as f64 * test_size).round() as usize;
                let n_val = (nc as f64 * val_size).round() as usize;

                let t = indices.split_off(nc - n_test);
                let v = indices.split_off(nc - n_test - n_val);
                train.extend(indices);
                val.extend(v);
                test.extend(t);
            }

            train.sort_unstable();
            val.sort_unstable();
            test.sort_unstable();
            (train, val, test)
        }
    };

    Ok((train, val, test))
}

fn select_rows(batches: &[RecordBatch], indices: &[usize]) -> Result<RecordBatch, DagError> {
    use arrow::datatypes::SchemaRef;
    let schema: SchemaRef = batches
        .first()
        .ok_or(common::err(NODE, "no input"))?
        .schema();
    let n_cols = schema.fields().len();
    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(n_cols);

    for col_i in 0..n_cols {
        let col_arrays: Vec<&dyn Array> =
            batches.iter().map(|b| b.column(col_i).as_ref()).collect();
        let indices_pairs: Vec<(usize, usize)> = indices
            .iter()
            .map(|&global_idx| {
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
        let result = interleave(&col_arrays, &indices_pairs)
            .map_err(|e| common::err(NODE, format!("row selection: {e}")))?;
        arrays.push(result);
    }

    RecordBatch::try_new_with_options(schema, arrays, &RecordBatchOptions::default())
        .map_err(|e| common::err(NODE, format!("build batch: {e}")))
}
