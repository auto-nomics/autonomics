//! DAG node: `grf_generate_causal_data`
//!
//! Pure-Rust data-generating process for the causal forest demos / tests.
//! Mirrors grf R's `generate_causal_data`:
//! - n observations, p covariates (first p/3 are confounders, rest noise)
//! - W = Bernoulli(σ(X_conf))
//! - τ(x) = 1 + x₁ - 0.5 x₂ + 0.1 x₃ (or custom)
//! - Y = τ(X) · W + ε

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::{GrfError, Result};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GenerateCausalDataSpec {
    pub n: usize,
    pub p: usize,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerateCausalDataOutput {
    /// Column-major matrix of X (p × n), emitted as row-major `Vec<Vec<f64>>`
    /// (n rows × p cols) for Arrow compatibility.
    pub x_rows: Vec<Vec<f64>>,
    pub y: Vec<f64>,
    pub w: Vec<f64>,
    pub true_tau: Vec<f64>,
    // CATE (mean of true_tau for treated subpopulation, etc.) — omitted for now.
}

pub struct GenerateCausalDataFactory;

impl GenerateCausalDataFactory {
    pub fn kind() -> &'static str {
        "grf_generate_causal_data"
    }
}

impl GenerateCausalDataSpec {
    /// Generate the dataset and return it as a single RecordBatch on the DAG
    /// output port.
    pub fn generate(&self) -> Result<RecordBatch> {
        if self.p < 3 {
            return Err(GrfError::Shape(
                "generate_causal_data needs p >= 3 (at least 1 confounder + 2 tau features)".into(),
            ));
        }
        let mut rng = rand::rngs::StdRng::seed_from_u64(self.seed);
        let normal = Normal::new(0.0, 1.0).unwrap();
        let mut x_rows = vec![vec![0.0_f64; self.p]; self.n];
        let mut y = vec![0.0_f64; self.n];
        let mut w = vec![0.0_f64; self.n];
        let mut true_tau = vec![0.0_f64; self.n];
        for i in 0..self.n {
            for j in 0..self.p {
                x_rows[i][j] = normal.sample(&mut rng);
            }
            // Confounder → treatment probability.
            let conf = x_rows[i][0];
            let prob = 1.0 / (1.0 + (-conf).exp());
            w[i] = if rand::random::<f64>() < prob {
                1.0
            } else {
                0.0
            };
            // CATE function (grf R style).
            let tau = 1.0 + x_rows[i][1] - 0.5 * x_rows[i][2]
                + 0.1 * x_rows[i].get(3).copied().unwrap_or(0.0);
            true_tau[i] = tau;
            y[i] = tau * w[i] + normal.sample(&mut rng);
        }

        let schema = Arc::new(Schema::new(
            vec![
                Field::new("y", DataType::Float64, false),
                Field::new("w", DataType::Float64, false),
                Field::new("true_tau", DataType::Float64, false),
                // x0..x{p-1}
            ]
            .into_iter()
            .chain((0..self.p).map(|j| Field::new(format!("x{}", j), DataType::Float64, false)))
            .collect::<Vec<_>>(),
        ));
        let mut cols: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(y.clone())),
            Arc::new(Float64Array::from(w.clone())),
            Arc::new(Float64Array::from(true_tau.clone())),
        ];
        for j in 0..self.p {
            let col: Vec<f64> = (0..self.n).map(|i| x_rows[i][j]).collect();
            cols.push(Arc::new(Float64Array::from(col)));
        }
        RecordBatch::try_new(schema, cols).map_err(|e| GrfError::Cpp(format!("RecordBatch: {e}")))
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("y", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(GenerateCausalDataSpec)
}
