//! DAG node: `grf_causal_survival_forest`
//!
//! Trains a causal survival forest for right-censored time-to-event data.
//! Mirrors grf R's `causal_survival_forest`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{CausalSurvivalSpec, CausalSurvivalTrainer, ForestBlob, ForestStats};
use crate::nodes::regression_forest::{NodeTrainOptions, arrow_batches_to_matrix};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CausalSurvivalForestSpec {
    #[serde(default)]
    pub x_column_names: Vec<String>,
    pub time_column_name: String,
    pub w_column_name: String,
    pub censor_column_name: String,
    /// `0` = RMST, `1` = survival probability. Default RMST.
    #[serde(default)]
    pub target: i32,
    /// Time horizon (required).
    pub horizon: f64,
    /// Optional fixed grid of failure times. None → observed event times.
    #[serde(default)]
    pub failure_times: Option<Vec<f64>>,
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    #[serde(default)]
    pub options: NodeTrainOptions,
}

#[derive(Debug, Clone)]
pub struct CausalSurvivalForestOutput {
    pub forest: ForestBlob,
    pub stats: ForestStats,
}

pub struct CausalSurvivalForestFactory;

impl CausalSurvivalForestFactory {
    pub fn kind() -> &'static str {
        "grf_causal_survival_forest"
    }
}

impl CausalSurvivalForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<CausalSurvivalForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows".into()));
        }
        let schema = batches[0].schema();
        let reserved = [
            &self.time_column_name,
            &self.w_column_name,
            &self.censor_column_name,
        ];
        let x_cols = if self.x_column_names.is_empty() {
            schema
                .fields()
                .iter()
                .filter(|f| {
                    !reserved.contains(&f.name()) && matches!(f.data_type(), DataType::Float64)
                })
                .map(|f| f.name().clone())
                .collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let time = arrow_batches_to_time(batches, &self.time_column_name, n_rows)?;
        let w = arrow_batches_to_f64_safe(batches, &self.w_column_name, n_rows)?;
        let censor_i64 = arrow_batches_to_int64(batches, &self.censor_column_name, n_rows)?;
        let censor: Vec<f64> = censor_i64.into_iter().map(|c| c as f64).collect();
        let weights = self
            .sample_weights_column
            .as_ref()
            .map(|c| arrow_batches_to_f64_safe(batches, c, n_rows))
            .transpose()?;

        let trained = CausalSurvivalTrainer::fit(CausalSurvivalSpec {
            x: x_matrix,
            time,
            w,
            censor,
            target: self.target,
            horizon: self.horizon,
            failure_times: self.failure_times.clone(),
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;
        let stats = ForestStats::from(&trained);
        Ok(CausalSurvivalForestOutput {
            forest: trained,
            stats,
        })
    }
}

fn arrow_batches_to_time(batches: &[RecordBatch], col: &str, n_rows: usize) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch
            .column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Float64", col)))?;
        for i in 0..batch.num_rows() {
            let v = if arr.is_null(i) {
                f64::NAN
            } else {
                arr.value(i)
            };
            if v.is_nan() || v < 0.0 {
                return Err(GrfError::Shape(format!(
                    "time column '{}' has non-positive / NaN value",
                    col
                )));
            }
            out.push(v);
        }
    }
    Ok(out)
}

fn arrow_batches_to_f64_safe(
    batches: &[RecordBatch],
    col: &str,
    n_rows: usize,
) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch
            .column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Float64", col)))?;
        for i in 0..batch.num_rows() {
            out.push(if arr.is_null(i) {
                f64::NAN
            } else {
                arr.value(i)
            });
        }
    }
    Ok(out)
}

fn arrow_batches_to_int64(batches: &[RecordBatch], col: &str, n_rows: usize) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch
            .column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr
            .as_any()
            .downcast_ref::<Int64Array>()
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
fn _schemars() -> schemars::Schema {
    schema_for!(CausalSurvivalForestSpec)
}

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}
