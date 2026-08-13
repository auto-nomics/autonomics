//! DAG node: `grf_predict_forest`
//!
//! Predicts with a trained forest on new data (or OOB if `oob = true`).

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::forest::{ForestBlob, PredictRequest, Predictions};
use crate::nodes::regression_forest::{arrow_batches_to_f64, arrow_batches_to_matrix};
use crate::{GrfError, Result};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PredictForestSpec {
    /// Column names in `port 1` (the test-X RecordBatch). If empty, all
    /// Float64 columns are used.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// If true, request variance estimates (requires `ci_group_size >= 2` at
    /// training time).
    #[serde(default)]
    pub estimate_variance: bool,
    /// If true, run OOB prediction on the training set; `port 1` is ignored.
    #[serde(default)]
    pub oob: bool,
    /// Optional override for `num.threads`. 0 = hardware default.
    #[serde(default)]
    pub num_threads: u32,
}

#[derive(Debug, Clone)]
pub struct PredictForestOutput {
    /// Column-major flat buffer of shape (pred_length, n_samples).
    pub predictions: Vec<f64>,
    pub variance: Option<Vec<f64>>,
    pub debiased_error: Option<Vec<f64>>,
    pub excess_error: Option<Vec<f64>>,
    pub pred_length: usize,
    pub n_samples: usize,
}

impl PredictForestOutput {
    pub fn n_samples(&self) -> usize {
        self.n_samples
    }
}

impl From<Predictions> for PredictForestOutput {
    fn from(p: Predictions) -> Self {
        let n_samples = p.n_samples();
        PredictForestOutput {
            predictions: p.values,
            variance: p.variance,
            debiased_error: p.debiased_error,
            excess_error: p.excess_error,
            pred_length: p.pred_length,
            n_samples,
        }
    }
}

pub struct PredictForestFactory;

impl PredictForestFactory {
    pub fn kind() -> &'static str {
        "grf_predict_forest"
    }
}

impl PredictForestSpec {
    pub fn predict(
        &self,
        forest: &ForestBlob,
        train_batches: &[RecordBatch],
        train_outcome_index: usize,
        test_batches: Option<&[RecordBatch]>,
    ) -> Result<PredictForestOutput> {
        let train_n = train_batches.iter().map(|b| b.num_rows()).sum();
        // Pass ALL columns from train_batches to grf. Forests using
        // DefaultPredictionStrategy (quantile, probability) need the
        // original Y values for prediction, so we must not strip the
        // outcome column. The grf core predict functions use the
        // outcome_index to locate Y internally.
        let train_schema = train_batches
            .first()
            .map(|b| b.schema())
            .ok_or_else(|| GrfError::Missing("empty train batches".into()))?;
        let train_all_cols: Vec<String> = (0..train_schema.fields().len())
            .map(|i| train_schema.field(i).name().clone())
            .collect();
        let train_x = arrow_batches_to_matrix(train_batches, &train_all_cols, train_n)?;

        let request = if self.oob || test_batches.is_none() {
            PredictRequest::oob(train_x, train_outcome_index, self.num_threads_opt())
        } else if let Some(test_batches) = test_batches {
            let test_n = test_batches.iter().map(|b| b.num_rows()).sum();
            let test_schema = test_batches
                .first()
                .map(|b| b.schema())
                .ok_or_else(|| GrfError::Missing("empty test batches".into()))?;
            let test_cols: Vec<String> = if self.x_column_names.is_empty() {
                test_schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect()
            } else {
                self.x_column_names.clone()
            };
            let test_x = arrow_batches_to_matrix(test_batches, &test_cols, test_n)?;
            PredictRequest::new_data(train_x, train_outcome_index, test_x, self.estimate_variance)
        } else {
            unreachable!()
        };

        let preds = forest.predict(request)?;
        Ok(PredictForestOutput::from(preds))
    }

    fn num_threads_opt(&self) -> Option<u32> {
        if self.num_threads == 0 {
            None
        } else {
            Some(self.num_threads)
        }
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("predictions", DataType::Float64, false),
        Field::new("variance", DataType::Float64, true),
    ]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(PredictForestSpec)
}

// `arrow_batches_to_f64` is used by other nodes; surface here to keep the
// import set stable.
#[allow(dead_code)]
fn _f64_marker(_: Vec<f64>) {}
