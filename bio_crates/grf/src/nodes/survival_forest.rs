//! DAG node: `grf_survival_forest`
//!
//! Trains a survival forest for right-censored time-to-event data.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, OobPredictions, SurvivalSpec, SurvivalTrainer};
use crate::nodes::regression_forest::{
    arrow_batches_to_f64, arrow_batches_to_matrix, NodeTrainOptions,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SurvivalForestSpec {
    /// Names of columns in `port 0` (the X RecordBatch) used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the column holding the event time (Float64, non-negative).
    pub time_column_name: String,
    /// Name of the column holding the censor indicator (Int64: 0=censored,
    /// 1=event observed).
    pub censor_column_name: String,
    /// Optional fixed grid of failure times. If None, observed event times
    /// are used.
    #[serde(default)]
    pub failure_times: Option<Vec<f64>>,
    /// Optional column of per-sample weights.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

#[derive(Debug, Clone)]
pub struct SurvivalForestOutput {
    pub forest: ForestBlob,
    /// OOB survival probabilities per sample × per failure time.
    /// Layout: column-major (`failure_times × n_rows`).
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct SurvivalForestFactory;

impl SurvivalForestFactory {
    pub fn kind() -> &'static str { "grf_survival_forest" }
}

impl SurvivalForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<SurvivalForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows in input batches".into()));
        }
        let schema = batches[0].schema();
        let reserved = [&self.time_column_name, &self.censor_column_name];
        let x_cols = if self.x_column_names.is_empty() {
            schema.fields().iter()
                .filter(|f| !reserved.contains(&f.name())
                    && matches!(f.data_type(), DataType::Float64))
                .map(|f| f.name().clone()).collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let time = arrow_batches_to_time(batches, &self.time_column_name, n_rows)?;
        // grf C++ core takes censor as f64 (0.0 or 1.0).
        let censor_i64 = arrow_batches_to_int64(batches, &self.censor_column_name, n_rows)?;
        let censor: Vec<f64> = censor_i64.iter().map(|&c| c as f64).collect();
        let weights = self.sample_weights_column.as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        // If failure_times is not given, compute unique observed event times
        // (matching grf R's default behavior).
        let failure_times = match &self.failure_times {
            Some(ft) => ft.clone(),
            None => {
                let mut ft: Vec<f64> = time.iter().zip(censor_i64.iter())
                    .filter(|(_, c)| **c == 1)
                    .map(|(t, _)| *t)
                    .collect();
                ft.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                ft.dedup();
                ft
            }
        };

        let trained = SurvivalTrainer::fit(SurvivalSpec {
            x: x_matrix,
            time,
            censor,
            failure_times: Some(failure_times),
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;

        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(SurvivalForestOutput {
            forest: trained,
            oob_predictions: oob,
            stats,
        })
    }
}

fn arrow_batches_to_time(
    batches: &[RecordBatch], col: &str, n_rows: usize,
) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch.column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr.as_any().downcast_ref::<Float64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Float64", col)))?;
        for i in 0..batch.num_rows() {
            let v = if arr.is_null(i) { f64::NAN } else { arr.value(i) };
            if v.is_nan() || v < 0.0 {
                return Err(GrfError::Shape(format!(
                    "time column '{}' has non-positive / NaN value at row {}", col, i
                )));
            }
            out.push(v);
        }
    }
    Ok(out)
}

fn arrow_batches_to_int64(
    batches: &[RecordBatch], col: &str, n_rows: usize,
) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch.column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr.as_any().downcast_ref::<Int64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Int64", col)))?;
        for i in 0..batch.num_rows() {
            out.push(if arr.is_null(i) { 0 } else { arr.value(i) });
        }
    }
    Ok(out)
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(SurvivalForestSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}