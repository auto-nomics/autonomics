//! DAG node: `grf_get_scores`
//!
//! Extract doubly-robust scores from a trained causal forest. Mirrors
//! `grf R`'s `get_scores(forest, ...)`.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::forest::ForestKind;
use crate::nodes::causal_forest::CausalForestOutput;
use crate::nodes::dr_scores::dr_scores_binary;
use crate::{GrfError, Result};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetScoresSpec {
    /// If true (default), train an auxiliary regression forest to estimate
    /// Var[W|X] when the treatment is continuous. For binary treatment
    /// grf uses the IPW formula directly.
    #[serde(default)]
    pub estimate_variance: bool,
    /// Number of trees in the auxiliary forest for Var[W|X].
    /// Only used for continuous treatments.
    #[serde(default = "default_num_trees")]
    pub num_trees_for_weights: u32,
}

fn default_num_trees() -> u32 {
    500
}

#[derive(Debug, Clone, Serialize)]
pub struct GetScoresOutput {
    pub dr_scores: Vec<f64>,
    pub n: usize,
}

pub struct GetScoresFactory;

impl GetScoresFactory {
    pub fn kind() -> &'static str {
        "grf_get_scores"
    }
}

impl GetScoresSpec {
    pub fn compute(&self, causal: &CausalForestOutput) -> Result<GetScoresOutput> {
        if causal.forest.kind() != ForestKind::Causal {
            return Err(GrfError::KindMismatch {
                trained: causal.forest.kind(),
                requested: ForestKind::Causal,
            });
        }
        let (y_orig, w_orig) = causal
            .original_outcomes()
            .ok_or_else(|| GrfError::Missing("missing Y/W on causal forest".into()))?;
        let y_hat = &causal.y_hat;
        let w_hat = &causal.w_hat;
        let tau_hat = causal
            .oob_predictions
            .as_ref()
            .ok_or_else(|| GrfError::Missing("missing OOB tau".into()))?
            .values
            .clone();

        let n = y_orig.len();
        // grf R's get_scores: for binary W, AIPW with IPW weights; for
        // continuous W, estimate Var[W|X] via a regression forest and use
        // CAPE-style debiasing. We only implement the binary branch.
        let binary_w = w_orig.iter().all(|&w| w == 0.0 || w == 1.0);
        if !binary_w {
            return Err(GrfError::Missing(
                "continuous treatment DR scores require Var[W|X] estimation; \
                 not implemented in this version"
                    .into(),
            ));
        }
        let dr = dr_scores_binary(y_orig, w_orig, y_hat, w_hat, &tau_hat);
        Ok(GetScoresOutput { dr_scores: dr, n })
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        "dr_score",
        DataType::Float64,
        false,
    )]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(GetScoresSpec)
}
