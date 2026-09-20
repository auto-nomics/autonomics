//! Survey-weighted moderated mediation node.
//!
//! Wraps [`epi::mediation_moderated`]. Conditional indirect effects
//! ω(w) = a(w)·b(w)·Δx and the Hayes (2015) index of moderated mediation
//! — the Figure 6C estimator of Li et al. 2026 — with design-aware
//! bootstrap CIs.
//!
//! Output (single row; the schema depends on the moderator grid, default
//! weighted mean ± weighted SD, so it is built per execute):
//!
//! | Column                       | Type    | Description                       |
//! |------------------------------|---------|-----------------------------------|
//! | `imm_first` / `_ci_*`        | Float64 | First-stage index a_xw·b_m + CI   |
//! | `imm_second` / `_ci_*`       | Float64 | Second-stage index a_x·b_mw + CI  |
//! | `w_{i}`                      | Float64 | i-th moderator level              |
//! | `cond_indirect_{i}` / `_ci_*`| Float64 | ω(w_i) + 95% CI                   |
//! | `direct_{i}` / `_ci_*`       | Float64 | c′(w_i) + 95% CI                  |
//! | `a_x`..`c_xw`                | Float64 | Path coefficients                 |
//! | `n_bootstrap`                | Int32   | Successful bootstrap replicates   |
//! | `n_obs`                      | Int32   | Complete-case observations         |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::arrow_util::ColumnError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};
use epi::bootstrap::{BootstrapDesign, CiMethod};
use epi::mediation_moderated::ModerationStage;

use crate::mediation_weighted::{parse_ci_method, to_u64_codes};

#[derive(Debug, Error)]
pub enum MediationModeratedError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for MediationModeratedError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for MediationModeratedError {
    fn node_type(&self) -> &str {
        "mediation_moderated"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MediationModeratedNodeSpec {
    /// Exposure column name.
    pub exposure_column: String,
    /// Mediator column name.
    pub mediator_column: String,
    /// Moderator column name (the W in X×W / M×W).
    pub moderator_column: String,
    /// Outcome column name.
    pub outcome_column: String,
    /// Optional confounder column names.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Sampling-weight column name (required, e.g. NHANES WTMEC2YR).
    pub weight_column: String,
    /// Stratum column name (e.g. SDMVSTRA); enables stratified bootstrap.
    #[serde(default)]
    pub strata_column: Option<String>,
    /// PSU column name (e.g. SDMVPSU); requires strata.
    #[serde(default)]
    pub psu_column: Option<String>,
    /// Moderation stage: "first" (X×W → M, default), "second" (M×W → Y),
    /// or "both".
    #[serde(default = "default_stage")]
    pub moderation_stage: String,
    /// Include X×M interaction in the outcome model (default false).
    #[serde(default)]
    pub include_xm_interaction: bool,
    /// Explicit moderator evaluation points; `null` ⇒ weighted mean ±
    /// weighted SD of the moderator.
    #[serde(default)]
    pub w_grid: Option<Vec<f64>>,
    /// CI construction: "bc" (bias-corrected, default) or "percentile".
    #[serde(default = "default_ci_method")]
    pub ci_method: String,
    /// Bootstrap iterations (default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_stage() -> String {
    "first".to_string()
}
fn default_ci_method() -> String {
    "bc".to_string()
}
fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

fn parse_stage(
    s: &str,
) -> Result<ModerationStage, MediationModeratedError> {
    match s {
        "first" => Ok(ModerationStage::First),
        "second" => Ok(ModerationStage::Second),
        "both" => Ok(ModerationStage::Both),
        other => Err(MediationModeratedError::Column(format!(
            "moderation_stage must be \"first\", \"second\" or \"both\", got \"{other}\""
        ))),
    }
}

#[derive(Clone)]
pub struct MediationModeratedNode {
    meta: NodePorts,
    exposure_column: String,
    mediator_column: String,
    moderator_column: String,
    outcome_column: String,
    covariates: Vec<String>,
    weight_column: String,
    strata_column: Option<String>,
    psu_column: Option<String>,
    stage: ModerationStage,
    include_xm_interaction: bool,
    w_grid: Option<Vec<f64>>,
    ci_method: CiMethod,
    n_bootstrap: usize,
    seed: u64,
}

pub struct MediationModeratedNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for MediationModeratedNodeFactory {
    fn kind(&self) -> &'static str {
        "mediation_moderated"
    }
    fn desc(&self) -> &'static str {
        "Moderated mediation: conditional indirect effects + Hayes index (weighted)."
    }
    fn doc(&self) -> &'static str {
        "Moderated mediation under sampling weights: the indirect effect \
        of X on Y through M is evaluated across levels of a moderator W \
        (first stage X×W, second stage M×W, or both). Reports the index \
        of moderated mediation (Hayes 2015) and conditional indirect / \
        direct effects at the moderator grid (weighted mean ± weighted \
        SD by default) with bias-corrected bootstrap CIs."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MediationModeratedNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MediationModeratedNodeSpec = serde_json::from_value(spec)?;
        let stage = parse_stage(&s.moderation_stage)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        let ci_method = parse_ci_method(&s.ci_method)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(Box::new(MediationModeratedNode {
            meta: port_layout(),
            exposure_column: s.exposure_column,
            mediator_column: s.mediator_column,
            moderator_column: s.moderator_column,
            outcome_column: s.outcome_column,
            covariates: s.covariates,
            weight_column: s.weight_column,
            strata_column: s.strata_column,
            psu_column: s.psu_column,
            stage,
            include_xm_interaction: s.include_xm_interaction,
            w_grid: s.w_grid,
            ci_method,
            n_bootstrap: s.n_bootstrap,
            seed: s.seed,
        }))
    }
}

#[async_trait]
impl DagNode for MediationModeratedNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "mediation_moderated"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(MediationModeratedError::Column("no input connected".to_string()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| MediationModeratedError::Collect(e.to_string()))?;

        let x_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.exposure_column)?;
        let m_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.mediator_column)?;
        let wm_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.moderator_column)?;
        let y_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.outcome_column)?;
        let w_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.weight_column)?;
        let s_raw = match &self.strata_column {
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
            None => None,
        };
        let p_raw = match &self.psu_column {
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
            None => None,
        };
        let cov_raw: Vec<Vec<f64>> = self
            .covariates
            .iter()
            .map(|c| dag_core::arrow_util::extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;

        // Complete-case filter across every column in play.
        let n = y_raw.len();
        let mut x = Vec::with_capacity(n);
        let mut m = Vec::with_capacity(n);
        let mut wm = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut w = Vec::with_capacity(n);
        let mut s_kept: Vec<f64> = Vec::with_capacity(n);
        let mut p_kept: Vec<f64> = Vec::with_capacity(n);
        let mut cov_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.covariates.len()];
        for i in 0..n {
            if x_raw[i].is_nan()
                || m_raw[i].is_nan()
                || wm_raw[i].is_nan()
                || y_raw[i].is_nan()
                || w_raw[i].is_nan()
                || s_raw.as_ref().is_some_and(|s| s[i].is_nan())
                || p_raw.as_ref().is_some_and(|p| p[i].is_nan())
                || cov_raw.iter().any(|c| c[i].is_nan())
            {
                continue;
            }
            x.push(x_raw[i]);
            m.push(m_raw[i]);
            wm.push(wm_raw[i]);
            y.push(y_raw[i]);
            w.push(w_raw[i]);
            if let Some(s) = &s_raw {
                s_kept.push(s[i]);
            }
            if let Some(p) = &p_raw {
                p_kept.push(p[i]);
            }
            for (j, c) in cov_raw.iter().enumerate() {
                cov_filtered[j].push(c[i]);
            }
        }
        if x.is_empty() {
            return Err(MediationModeratedError::Column("no complete-case rows".to_string()).into());
        }

        let strata = if s_raw.is_some() {
            Some(to_u64_codes(&s_kept, self.strata_column.as_deref().unwrap_or(""))?)
        } else {
            None
        };
        let psu = if p_raw.is_some() {
            Some(to_u64_codes(&p_kept, self.psu_column.as_deref().unwrap_or(""))?)
        } else {
            None
        };

        let cov_slices: Vec<&[f64]> = cov_filtered.iter().map(|v| v.as_slice()).collect();
        let design = BootstrapDesign::with_clusters(&w, strata.as_deref(), psu.as_deref())
            .map_err(|e| MediationModeratedError::Fit(e.to_string()))?;
        let opts = epi::mediation_moderated::ModeratedMediationOptions {
            stage: self.stage,
            include_xm_interaction: self.include_xm_interaction,
            w_grid: self.w_grid.clone(),
            ci_method: self.ci_method,
            n_bootstrap: self.n_bootstrap,
            seed: self.seed,
            ..Default::default()
        };
        let result = epi::mediation_moderated::mediation_moderated(
            &x, &m, &wm, &y, &cov_slices, &design, &opts,
        )
        .map_err(|e| MediationModeratedError::Fit(e.to_string()))?;

        // Grid-dependent output schema, built explicitly per execute.
        let mut fields: Vec<Field> = vec![
            Field::new("imm_first", DataType::Float64, false),
            Field::new("imm_first_ci_lower", DataType::Float64, false),
            Field::new("imm_first_ci_upper", DataType::Float64, false),
            Field::new("imm_second", DataType::Float64, false),
            Field::new("imm_second_ci_lower", DataType::Float64, false),
            Field::new("imm_second_ci_upper", DataType::Float64, false),
        ];
        for i in 0..result.w_points.len() {
            fields.push(Field::new(format!("w_{i}"), DataType::Float64, false));
            fields.push(Field::new(format!("cond_indirect_{i}"), DataType::Float64, false));
            fields.push(Field::new(format!("cond_indirect_{i}_ci_lower"), DataType::Float64, false));
            fields.push(Field::new(format!("cond_indirect_{i}_ci_upper"), DataType::Float64, false));
            fields.push(Field::new(format!("direct_{i}"), DataType::Float64, false));
            fields.push(Field::new(format!("direct_{i}_ci_lower"), DataType::Float64, false));
            fields.push(Field::new(format!("direct_{i}_ci_upper"), DataType::Float64, false));
        }
        fields.extend([
            Field::new("a_x", DataType::Float64, false),
            Field::new("a_xw", DataType::Float64, false),
            Field::new("b_m", DataType::Float64, false),
            Field::new("b_mw", DataType::Float64, false),
            Field::new("c_x", DataType::Float64, false),
            Field::new("c_xw", DataType::Float64, false),
            Field::new("n_bootstrap", DataType::Int32, false),
            Field::new("n_obs", DataType::Int32, false),
        ]);

        let mut cols: Vec<Arc<dyn arrow_array::Array>> = vec![
            Arc::new(Float64Array::from(vec![result.index_first_stage])),
            Arc::new(Float64Array::from(vec![result.index_first_stage_ci.0])),
            Arc::new(Float64Array::from(vec![result.index_first_stage_ci.1])),
            Arc::new(Float64Array::from(vec![result.index_second_stage])),
            Arc::new(Float64Array::from(vec![result.index_second_stage_ci.0])),
            Arc::new(Float64Array::from(vec![result.index_second_stage_ci.1])),
        ];
        for (i, c) in result.conditional.iter().enumerate() {
            let (_, direct, d_ci) = &result.conditional_direct[i];
            cols.push(Arc::new(Float64Array::from(vec![c.w])));
            cols.push(Arc::new(Float64Array::from(vec![c.indirect])));
            cols.push(Arc::new(Float64Array::from(vec![c.ci.0])));
            cols.push(Arc::new(Float64Array::from(vec![c.ci.1])));
            cols.push(Arc::new(Float64Array::from(vec![*direct])));
            cols.push(Arc::new(Float64Array::from(vec![d_ci.0])));
            cols.push(Arc::new(Float64Array::from(vec![d_ci.1])));
        }
        let tail: Vec<Arc<dyn arrow_array::Array>> = vec![
            Arc::new(Float64Array::from(vec![result.a_x])),
            Arc::new(Float64Array::from(vec![result.a_xw])),
            Arc::new(Float64Array::from(vec![result.b_m])),
            Arc::new(Float64Array::from(vec![result.b_mw])),
            Arc::new(Float64Array::from(vec![result.c_x])),
            Arc::new(Float64Array::from(vec![result.c_xw])),
            Arc::new(Int32Array::from(vec![result.n_bootstrap as i32])),
            Arc::new(Int32Array::from(vec![result.n_obs as i32])),
        ];
        cols.extend(tail);

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
            .expect("mediation_moderated schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| MediationModeratedError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn make_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect();
        let arrays: Vec<Arc<dyn arrow_array::Array>> = columns
            .iter()
            .map(|(_, vals)| {
                Arc::new(Float64Array::from(vals.clone())) as Arc<dyn arrow_array::Array>
            })
            .collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    /// First-stage moderation fixture: a_xw = 0.4, b_m = 0.7, c_x = 0.2.
    fn input_batch() -> RecordBatch {
        let n = 80;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let wm: Vec<f64> = (0..n).map(|i| ((i % 7) as f64 - 3.0) / 3.0).collect();
        let xw: Vec<f64> = x.iter().zip(&wm).map(|(&a, &b)| a * b).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.5 * x[i] + 0.3 * wm[i] + 0.4 * xw[i] + 0.2 * (i % 5) as f64 / 5.0)
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 2.0 + 0.2 * x[i] + 0.7 * m[i] + 0.1 * wm[i] + (i % 3) as f64 / 3.0)
            .collect();
        let wt: Vec<f64> = (0..n).map(|i| 5000.0 + 100.0 * (i % 5) as f64).collect();
        make_batch(vec![
            ("x", x),
            ("m", m),
            ("wm", wm),
            ("y", y),
            ("wt", wt),
        ])
    }

    async fn run_node(spec: serde_json::Value) -> Vec<RecordBatch> {
        let mut node =
            MediationModeratedNodeFactory {}.build(spec, node_ctx()).unwrap();
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(input_batch())
                .unwrap(),
        );
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        outs.dataframe(0).unwrap().clone().collect().await.unwrap()
    }

    fn cell(rows: &[RecordBatch], name: &str) -> f64 {
        let batch = &rows[0];
        let idx = batch.schema().index_of(name).unwrap();
        let col = batch.column(idx);
        if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
            a.value(0)
        } else {
            col.as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .value(0) as f64
        }
    }

    #[tokio::test]
    async fn default_grid_smoke() {
        let rows = run_node(serde_json::json!({
            "exposure_column": "x",
            "mediator_column": "m",
            "moderator_column": "wm",
            "outcome_column": "y",
            "weight_column": "wt",
            "n_bootstrap": 50,
        }))
        .await;
        assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
        assert_eq!(cell(&rows, "n_obs") as usize, 80);
        assert_eq!(cell(&rows, "n_bootstrap") as usize, 50);
        // Default grid = weighted mean ± weighted SD → 3 points, ordered.
        assert!(cell(&rows, "w_0") < cell(&rows, "w_1"));
        assert!(cell(&rows, "w_1") < cell(&rows, "w_2"));
        // Real first-stage moderation ⇒ the conditional indirect varies.
        assert!(cell(&rows, "cond_indirect_0") < cell(&rows, "cond_indirect_2"));
        assert!(cell(&rows, "imm_first") > 0.0);
        assert!(cell(&rows, "imm_first_ci_lower") < cell(&rows, "imm_first_ci_upper"));
        assert!((0.0..0.5).contains(&cell(&rows, "direct_1")));
    }

    #[tokio::test]
    async fn bad_stage_is_rejected_at_build() {
        let err = MediationModeratedNodeFactory {}.build(
            serde_json::json!({
                "exposure_column": "x",
                "mediator_column": "m",
                "moderator_column": "wm",
                "outcome_column": "y",
                "weight_column": "wt",
                "moderation_stage": "everywhere",
            }),
            node_ctx(),
        );
        assert!(err.is_err());
    }
}
