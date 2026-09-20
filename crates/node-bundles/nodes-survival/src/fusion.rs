//! `fusion_fit` and `lock_predict` — the locked fusion-model pair for the
//! chordoma SAP.
//!
//! `fusion_fit` trains the final ridge Fine–Gray fusion model on the full
//! development cohort: clinical covariates enter unpenalized (the
//! pre-specified clinical model keeps full strength) while the modality
//! score / biomarker block shrinks under the ridge penalty. `λ` is either
//! fixed or selected by inner cross-validation minimizing the mean inner-OOF
//! IPCW Brier at the horizon — never on external data.
//!
//! `lock_predict` re-fits the **identical** specification on the frozen
//! training table (`lambda` is a required literal transcribed from
//! `fusion_fit`'s summary — no re-selection) and applies it to a target
//! cohort: the frozen external-validation step. Standardization statistics
//! come from the training table only, so the lock freezes preprocessing and
//! coefficients together, not just the coefficients.

use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use cmprsk::{CrrInput, CrrRidgeFit, CrrRidgeOptions, TimeFunctions, crr_ridge};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::cv::{LambdaSelection, cif_at, select_lambda_inner};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

pub const FUSION_FIT_NODE_KIND: &str = "fusion_fit";
pub const LOCK_PREDICT_NODE_KIND: &str = "lock_predict";

fn d_failcode() -> f64 {
    1.0
}
fn d_cencode() -> f64 {
    0.0
}
fn d_folds() -> usize {
    5
}
fn d_gtol() -> f64 {
    1e-6
}
fn d_maxiter() -> usize {
    200
}

// ── shared computation ──────────────────────────────────────────────────────

/// Fit the fusion model on complete-case data laid out as
/// `[clinical…, covariates…]`, optionally selecting `λ` by inner CV.
pub(crate) fn fit_fusion(
    time: &[f64],
    status_code: &[u8],
    cov1: &[Vec<f64>],
    terms: &[String],
    n_clinical: usize,
    lambda: Option<f64>,
    grid: &[f64],
    n_inner: usize,
    horizon: Option<f64>,
    seed: u64,
    gtol: f64,
    maxiter: usize,
) -> Result<(CrrRidgeFit, Option<LambdaSelection>), String> {
    let (lambda, selection) = match lambda {
        Some(l) => (l, None),
        None => {
            let horizon = horizon.ok_or_else(|| {
                "horizon is required when lambda is not fixed (it drives the inner-CV \
                 IPCW Brier used to select lambda)"
                    .to_string()
            })?;
            let idx: Vec<usize> = (0..time.len()).collect();
            let sel = select_lambda_inner(
                time,
                status_code,
                cov1,
                terms,
                n_clinical,
                &idx,
                grid,
                n_inner,
                horizon,
                seed,
                1,
                gtol,
                maxiter,
            )?;
            (sel.best, Some(sel))
        }
    };
    let fit = crr_ridge(
        &CrrInput {
            ftime: time,
            fstatus: &status_code
                .iter()
                .map(|&s| f64::from(s))
                .collect::<Vec<_>>(),
            cov1,
            cov1_names: terms,
            cov2: &[],
            cov2_names: &[],
            tf: TimeFunctions::None,
            cengroup: None,
        },
        &CrrRidgeOptions {
            lambda,
            unpenalized: (0..n_clinical).collect(),
            standardize: true,
            gtol,
            maxiter,
            failcode: 1.0,
            cencode: 0.0,
        },
    )
    .map_err(|e| format!("ridge Fine-Gray fit failed: {e}"))?;
    Ok((fit, selection))
}

/// Standard-normal CDF via the Abramowitz–Stegun 7.1.26 erf approximation
/// evaluated at `x/√2` (`Φ(x) = ½(1 + erf(x/√2))`; erf error < 1.5e-7 —
/// ample for Wald p-values).
fn normal_cdf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let y = x.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * y);
    let erf = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-y * y).exp();
    0.5 * (1.0 + sign * erf)
}

/// Coefficient table of `fusion_fit` port 0.
fn build_coef_batch(
    kind: &str,
    fit: &CrrRidgeFit,
    n_clinical: usize,
) -> Result<RecordBatch, DagError> {
    let np = fit.ncov1;
    let se: Vec<f64> = (0..np).map(|j| fit.var[j][j].max(0.0).sqrt()).collect();
    let z: Vec<f64> = fit
        .coef
        .iter()
        .zip(se.iter())
        .map(|(&b, &s)| b / s)
        .collect();
    let p: Vec<f64> = z
        .iter()
        .map(|&v| 2.0 * (1.0 - normal_cdf(v.abs())))
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("term", DataType::Utf8, false),
            Field::new("coefficient", DataType::Float64, false),
            Field::new("subhazard_ratio", DataType::Float64, false),
            Field::new("std_error", DataType::Float64, false),
            Field::new("z_stat", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("penalized", DataType::Boolean, false),
        ])),
        vec![
            Arc::new(StringArray::from(fit.terms.clone())),
            Arc::new(Float64Array::from(fit.coef.clone())),
            Arc::new(Float64Array::from(
                fit.coef.iter().map(|b| b.exp()).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(se)),
            Arc::new(Float64Array::from(z)),
            Arc::new(Float64Array::from(p)),
            Arc::new(BooleanArray::from(
                (0..np).map(|j| j >= n_clinical).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| DagError::NodeError {
        node_type: kind.into(),
        msg: format!("coefficient batch: {e}"),
    })
}

/// Baseline cumulative-incidence table of `fusion_fit` port 1.
fn build_baseline_batch(kind: &str, fit: &CrrRidgeFit) -> Result<RecordBatch, DagError> {
    let mut acc = 0.0_f64;
    let cif: Vec<f64> = fit
        .bfitj
        .iter()
        .map(|&j| {
            acc += j;
            1.0 - (-acc).exp()
        })
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("uftime", DataType::Float64, false),
            Field::new("bfitj", DataType::Float64, false),
            Field::new("baseline_cif", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Float64Array::from(fit.uftime.clone())),
            Arc::new(Float64Array::from(fit.bfitj.clone())),
            Arc::new(Float64Array::from(cif)),
        ],
    )
    .map_err(|e| DagError::NodeError {
        node_type: kind.into(),
        msg: format!("baseline batch: {e}"),
    })
}

// ── shared design-matrix extraction ────────────────────────────────────────

/// Complete-case design in `[clinical…, covariates…]` layout with recoded
/// status in the crrkit `{0, 1, 2}` contract.
pub(crate) struct PreparedDesign {
    pub layout: Vec<String>,
    pub n_clinical: usize,
    pub time_cc: Vec<f64>,
    pub status_cc: Vec<u8>,
    pub cov1: Vec<Vec<f64>>,
    pub n_dropped: usize,
}

fn prepare_design(
    batches: &[RecordBatch],
    time_column: &str,
    status_column: &str,
    clinical: &[String],
    covariates: &[String],
    failcode: f64,
    cencode: f64,
) -> Result<PreparedDesign, String> {
    let time = dag_core::arrow_util::extract_numeric_lenient(batches, time_column)
        .map_err(|e| e.to_string())?;
    let fstatus = dag_core::arrow_util::extract_numeric_lenient(batches, status_column)
        .map_err(|e| e.to_string())?;
    let layout: Vec<String> = clinical.iter().chain(covariates.iter()).cloned().collect();
    let mut columns: Vec<Vec<f64>> = Vec::with_capacity(layout.len());
    for name in &layout {
        columns.push(
            dag_core::arrow_util::extract_numeric_lenient(batches, name)
                .map_err(|e| e.to_string())?,
        );
    }
    let n = time.len();
    let mut keep = vec![true; n];
    let mut n_dropped = 0usize;
    for i in 0..n {
        let bad = !time[i].is_finite()
            || !fstatus[i].is_finite()
            || columns.iter().any(|c| !c[i].is_finite());
        if bad {
            keep[i] = false;
            n_dropped += 1;
        }
    }
    let pos: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
    let time_cc: Vec<f64> = pos.iter().map(|&i| time[i]).collect();
    let status_cc = crate::cv::recode_status(
        &(pos.iter().map(|&i| fstatus[i]).collect::<Vec<_>>()),
        failcode,
        cencode,
    )?;
    let cov1: Vec<Vec<f64>> = pos
        .iter()
        .map(|&i| columns.iter().map(|c| c[i]).collect())
        .collect();
    Ok(PreparedDesign {
        layout,
        n_clinical: clinical.len(),
        time_cc,
        status_cc,
        cov1,
        n_dropped,
    })
}

fn validate_covariates(clinical: &[String], covariates: &[String]) -> Result<(), String> {
    if covariates.is_empty() {
        return Err("covariates must be non-empty".into());
    }
    let overlap: Vec<&String> = clinical.iter().filter(|c| covariates.contains(c)).collect();
    if !overlap.is_empty() {
        return Err(format!(
            "columns listed in both clinical and covariates: {}",
            overlap
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

fn validate_lambda_grid(grid: &[f64]) -> Result<(), String> {
    if grid.is_empty() {
        return Err("lambda_grid must be non-empty".into());
    }
    if grid.iter().any(|l| !l.is_finite() || *l < 0.0) {
        return Err("every lambda in lambda_grid must be finite and nonnegative".into());
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// fusion_fit
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for the `fusion_fit` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FusionFitSpec {
    /// Failure / censoring time column.
    pub time_column: String,
    /// Failure-type code column (`failcode` = event, `cencode` = censored,
    /// anything else = competing event).
    pub status_column: String,
    /// Penalized biomarker / modality-score columns.
    pub covariates: Vec<String>,
    /// Unpenalized clinical columns.
    #[serde(default)]
    pub clinical: Vec<String>,
    /// Fixed ridge `λ`. When omitted, `lambda_grid` and `horizon` drive the
    /// inner-CV selection.
    #[serde(default)]
    pub lambda: Option<f64>,
    /// Candidate penalties for inner-CV selection (required when `lambda`
    /// is omitted).
    #[serde(default)]
    pub lambda_grid: Vec<f64>,
    /// Evaluation horizon for the inner-CV IPCW Brier (required when
    /// `lambda` is omitted).
    #[serde(default)]
    pub horizon: Option<f64>,
    /// Inner folds for `λ` selection.
    #[serde(default = "d_folds")]
    pub n_inner: usize,
    /// Seed for the deterministic fold assignment.
    #[serde(default)]
    pub seed: u64,
    /// Code of `status_column` denoting the event of interest.
    #[serde(default = "d_failcode")]
    pub failcode: f64,
    /// Code of `status_column` denoting censoring.
    #[serde(default = "d_cencode")]
    pub cencode: f64,
    /// Gradient tolerance.
    #[serde(default = "d_gtol")]
    pub gtol: f64,
    /// Maximum Newton iterations.
    #[serde(default = "d_maxiter")]
    pub maxiter: usize,
}

impl FusionFitSpec {
    fn validate(&self) -> Result<(), String> {
        validate_covariates(&self.clinical, &self.covariates)?;
        if let Some(l) = self.lambda {
            if !l.is_finite() || l < 0.0 {
                return Err("lambda must be finite and nonnegative".into());
            }
        } else {
            validate_lambda_grid(&self.lambda_grid)?;
            if self.horizon.is_none_or(|h| !h.is_finite() || h <= 0.0) {
                return Err(
                    "a finite positive horizon is required when lambda is not fixed".into(),
                );
            }
            if self.n_inner < 2 {
                return Err("n_inner must be >= 2".into());
            }
        }
        Ok(())
    }
}

pub struct FusionFitNodeFactory {}

fn fusion_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(None) // 0: development-cohort table
        .add_output_port(None) // 0: coefficient table
        .add_output_port(None) // 1: baseline cumulative incidence
        .add_output_port(None) // 2: fit summary (locked export bookkeeping)
}

impl NodeFactory for FusionFitNodeFactory {
    fn kind(&self) -> &'static str {
        FUSION_FIT_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Locked ridge Fine-Gray fusion model: clinical unpenalized, modality scores penalized."
    }
    fn doc(&self) -> &'static str {
        "Trains the final fusion model for the frozen SAP: a ridge-penalized \
        Fine-Gray regression whose clinical columns are exempt from the \
        penalty and whose biomarker / modality-score columns shrink. lambda \
        is either fixed or selected by inner cross-validation minimizing the \
        mean inner out-of-fold IPCW Brier at the horizon — development data \
        only, never external. Port 0 is the coefficient table (Wald columns \
        are conditional on lambda: use them for prediction calibration, not \
        confirmatory inference); port 1 the baseline cumulative incidence; \
        port 2 the fit summary recording the selected lambda — transcribe \
        that lambda into lock_predict to freeze the model for external \
        validation."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FusionFitSpec)
    }
    fn ports(&self) -> NodePorts {
        fusion_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: FusionFitSpec = serde_json::from_value(spec)?;
        if let Err(reason) = s.validate() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: FUSION_FIT_NODE_KIND.to_string(),
                reason,
                schema_pretty: serde_json::to_string_pretty(&schema_for!(FusionFitSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(FusionFitNode {
            meta: fusion_ports(),
            spec: s,
        }))
    }
}

#[derive(Clone)]
pub struct FusionFitNode {
    meta: NodePorts,
    spec: FusionFitSpec,
}

fn ferr(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: FUSION_FIT_NODE_KIND.into(),
        msg: msg.into(),
    }
}

#[async_trait]
impl DagNode for FusionFitNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        FUSION_FIT_NODE_KIND
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
        let input = inputs.first().ok_or_else(|| ferr("no input connected"))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| ferr(format!("collect: {e}")))?;
        let s = &self.spec;
        let prepared = prepare_design(
            &batches,
            &s.time_column,
            &s.status_column,
            &s.clinical,
            &s.covariates,
            s.failcode,
            s.cencode,
        )
        .map_err(ferr)?;

        let (fit, selection) = fit_fusion(
            &prepared.time_cc,
            &prepared.status_cc,
            &prepared.cov1,
            &prepared.layout,
            prepared.n_clinical,
            s.lambda,
            &s.lambda_grid,
            s.n_inner,
            s.horizon,
            s.seed,
            s.gtol,
            s.maxiter,
        )
        .map_err(ferr)?;

        let coef_batch = build_coef_batch(FUSION_FIT_NODE_KIND, &fit, prepared.n_clinical)?;
        let baseline_batch = build_baseline_batch(FUSION_FIT_NODE_KIND, &fit)?;
        let inner_brier = selection.as_ref().and_then(|sel| {
            sel.mean_brier
                .iter()
                .find(|(l, _)| *l == sel.best)
                .map(|(_, m)| *m)
        });
        let summary_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("lambda", DataType::Float64, false),
                Field::new("lambda_selected_by_inner_cv", DataType::Boolean, false),
                Field::new("inner_brier_at_lambda", DataType::Float64, true),
                Field::new("n_failed_inner_evals", DataType::UInt32, false),
                Field::new("horizon", DataType::Float64, true),
                Field::new("intercept", DataType::Float64, false),
                Field::new("loglik", DataType::Float64, false),
                Field::new("objective_penalized", DataType::Float64, false),
                Field::new("n_obs", DataType::UInt32, false),
                Field::new("n_dropped_missing", DataType::UInt32, false),
                Field::new("n_events", DataType::UInt32, false),
                Field::new("n_iter", DataType::UInt32, false),
                Field::new("converged", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![fit.lambda])),
                Arc::new(BooleanArray::from(vec![selection.is_some()])),
                Arc::new(Float64Array::from(vec![inner_brier])),
                Arc::new(UInt32Array::from(vec![
                    selection.as_ref().map_or(0, |sel| sel.n_failed_evals) as u32,
                ])),
                Arc::new(Float64Array::from(vec![s.horizon])),
                Arc::new(Float64Array::from(vec![fit.intercept])),
                Arc::new(Float64Array::from(vec![fit.loglik])),
                Arc::new(Float64Array::from(vec![fit.objective_penalized])),
                Arc::new(UInt32Array::from(vec![fit.n as u32])),
                Arc::new(UInt32Array::from(vec![prepared.n_dropped as u32])),
                Arc::new(UInt32Array::from(vec![fit.n_events as u32])),
                Arc::new(UInt32Array::from(vec![fit.n_iter as u32])),
                Arc::new(BooleanArray::from(vec![fit.converged])),
            ],
        )
        .map_err(|e| ferr(format!("summary batch: {e}")))?;

        let ctx = node_ctx.session();
        let mut res = PortOutputs::new();
        res.insert(
            0,
            ctx.read_batch(coef_batch)
                .map_err(|e| ferr(format!("read_batch(0): {e}")))?,
        );
        res.insert(
            1,
            ctx.read_batch(baseline_batch)
                .map_err(|e| ferr(format!("read_batch(1): {e}")))?,
        );
        res.insert(
            2,
            ctx.read_batch(summary_batch)
                .map_err(|e| ferr(format!("read_batch(2): {e}")))?,
        );
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// lock_predict
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for the `lock_predict` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LockPredictSpec {
    /// Failure / censoring time column of the **training** table.
    pub time_column: String,
    /// Failure-type code column of the **training** table.
    pub status_column: String,
    /// Penalized biomarker / modality-score columns (same list and order as
    /// `fusion_fit`).
    pub covariates: Vec<String>,
    /// Unpenalized clinical columns (same list and order as `fusion_fit`).
    #[serde(default)]
    pub clinical: Vec<String>,
    /// The locked ridge `λ`, transcribed from `fusion_fit`'s summary. No
    /// re-selection happens here.
    pub lambda: f64,
    /// Horizon `t*` (days) at which the frozen CIF is evaluated.
    pub horizon: f64,
    /// Code of `status_column` denoting the event of interest.
    #[serde(default = "d_failcode")]
    pub failcode: f64,
    /// Code of `status_column` denoting censoring.
    #[serde(default = "d_cencode")]
    pub cencode: f64,
    /// Gradient tolerance.
    #[serde(default = "d_gtol")]
    pub gtol: f64,
    /// Maximum Newton iterations.
    #[serde(default = "d_maxiter")]
    pub maxiter: usize,
}

impl LockPredictSpec {
    fn validate(&self) -> Result<(), String> {
        validate_covariates(&self.clinical, &self.covariates)?;
        if !self.lambda.is_finite() || self.lambda < 0.0 {
            return Err("lambda must be finite and nonnegative".into());
        }
        if !self.horizon.is_finite() || self.horizon <= 0.0 {
            return Err("horizon must be finite and positive".into());
        }
        Ok(())
    }
}

pub struct LockPredictNodeFactory {}

fn lock_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(None) // 0: frozen training table
        .add_input_port(None) // 1: target cohort
        .add_output_port(None) // 0: target rows + risk_score + predicted_cif
}

impl NodeFactory for LockPredictNodeFactory {
    fn kind(&self) -> &'static str {
        LOCK_PREDICT_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Apply the frozen fusion model to a target cohort (frozen external validation)."
    }
    fn doc(&self) -> &'static str {
        "Re-fits the exact locked specification — same column lists, same \
        fixed lambda transcribed from fusion_fit's summary, no re-selection \
        — on the frozen training table (port 0), then scores every row of \
        the target cohort (port 1). Standardization statistics derive from \
        the training table only. The target table must carry every clinical \
        and covariate column with no missing values: a locked model cannot \
        impute. Output port 0 is the target rows plus risk_score (linear \
        predictor) and predicted_cif (cumulative incidence at the horizon)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LockPredictSpec)
    }
    fn ports(&self) -> NodePorts {
        lock_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: LockPredictSpec = serde_json::from_value(spec)?;
        if let Err(reason) = s.validate() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: LOCK_PREDICT_NODE_KIND.to_string(),
                reason,
                schema_pretty: serde_json::to_string_pretty(&schema_for!(LockPredictSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(LockPredictNode {
            meta: lock_ports(),
            spec: s,
        }))
    }
}

#[derive(Clone)]
pub struct LockPredictNode {
    meta: NodePorts,
    spec: LockPredictSpec,
}

fn lerr(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: LOCK_PREDICT_NODE_KIND.into(),
        msg: msg.into(),
    }
}

#[async_trait]
impl DagNode for LockPredictNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        LOCK_PREDICT_NODE_KIND
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
        let train_input = inputs
            .iter()
            .find(|i| i.port == 0)
            .ok_or_else(|| lerr("training table (port 0) not connected"))?;
        let target_input = inputs
            .iter()
            .find(|i| i.port == 1)
            .ok_or_else(|| lerr("target table (port 1) not connected"))?;
        let train_batches = train_input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| lerr(format!("collect train: {e}")))?;
        let target_batches = target_input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| lerr(format!("collect target: {e}")))?;

        let s = &self.spec;
        let prepared = prepare_design(
            &train_batches,
            &s.time_column,
            &s.status_column,
            &s.clinical,
            &s.covariates,
            s.failcode,
            s.cencode,
        )
        .map_err(lerr)?;

        // Target rows: every used column present and finite — a locked model
        // cannot impute.
        let mut target_columns: Vec<Vec<f64>> = Vec::with_capacity(prepared.layout.len());
        for name in &prepared.layout {
            target_columns.push(
                dag_core::arrow_util::extract_numeric_lenient(&target_batches, name)
                    .map_err(|e| lerr(format!("target table: {e}")))?,
            );
        }
        let n_target = target_batches.iter().map(|b| b.num_rows()).sum::<usize>();
        for (j, col) in target_columns.iter().enumerate() {
            if col.len() != n_target || col.iter().any(|v| !v.is_finite()) {
                return Err(lerr(format!(
                    "target column '{}' has missing or non-finite values; a locked model \
                     cannot impute — complete the column upstream",
                    prepared.layout[j]
                )));
            }
        }

        let (fit, _) = fit_fusion(
            &prepared.time_cc,
            &prepared.status_cc,
            &prepared.cov1,
            &prepared.layout,
            prepared.n_clinical,
            Some(s.lambda),
            &[],
            d_folds(),
            Some(s.horizon),
            0,
            s.gtol,
            s.maxiter,
        )
        .map_err(lerr)?;

        let mut risk = Vec::with_capacity(n_target);
        let mut cif = Vec::with_capacity(n_target);
        for i in 0..n_target {
            let row: Vec<f64> = target_columns.iter().map(|c| c[i]).collect();
            risk.push(
                fit.linear_predictor(&row)
                    .map_err(|e| lerr(e.to_string()))?,
            );
            cif.push(cif_at(&fit, &row, s.horizon).map_err(lerr)?);
        }

        // Target rows + the two prediction columns.
        let batch0 = target_batches
            .first()
            .ok_or_else(|| lerr("target has no rows"))?;
        let schema = batch0.schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<ArrayRef> = Vec::with_capacity(fields.len() + 2);
        for col_i in 0..fields.len() {
            let chunks: Vec<&dyn Array> = target_batches
                .iter()
                .map(|b| b.column(col_i).as_ref())
                .collect();
            arrays.push(
                arrow_select::concat::concat(&chunks)
                    .map_err(|e| lerr(format!("concat target columns: {e}")))?,
            );
        }
        fields.push(Arc::new(Field::new("risk_score", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(risk)));
        fields.push(Arc::new(Field::new(
            "predicted_cif",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(cif)));
        let out_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| lerr(format!("build output batch: {e}")))?;

        let ctx = node_ctx.session();
        let mut res = PortOutputs::new();
        res.insert(
            0,
            ctx.read_batch(out_batch)
                .map_err(|e| lerr(format!("read_batch: {e}")))?,
        );
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn uniform(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn exponential(&mut self, rate: f64) -> f64 {
            -(self.uniform().max(1e-12)).ln() / rate
        }
    }

    fn simulate(n: usize, beta: &[f64], seed: u64) -> (Vec<f64>, Vec<u8>, Vec<Vec<f64>>) {
        let p = beta.len();
        let mut rng = Rng(seed);
        let mut time = Vec::with_capacity(n);
        let mut status = Vec::with_capacity(n);
        let mut cov1 = Vec::with_capacity(n);
        for _ in 0..n {
            let row: Vec<f64> = (0..p).map(|_| rng.uniform() * 2.0 - 1.0).collect();
            let eta: f64 = row.iter().zip(beta.iter()).map(|(x, b)| x * b).sum();
            let t1 = rng.exponential(0.05 * eta.exp());
            let t2 = rng.exponential(0.06);
            let c = rng.exponential(0.04);
            if t1 <= t2 && t1 <= c {
                time.push(t1);
                status.push(1u8);
            } else if t2 < t1 && t2 <= c {
                time.push(t2);
                status.push(2u8);
            } else {
                time.push(c);
                status.push(0u8);
            }
            cov1.push(row);
        }
        (time, status, cov1)
    }

    #[test]
    fn normal_cdf_matches_reference_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-9);
        assert!((normal_cdf(1.959964) - 0.975).abs() < 1e-4);
        assert!((normal_cdf(-1.959964) - 0.025).abs() < 1e-4);
        assert!((normal_cdf(3.0) - 0.99865).abs() < 1e-5);
    }

    #[test]
    fn fusion_fit_recovers_signal_with_tiny_lambda() {
        let (time, status, cov1) = simulate(300, &[0.8, -0.6], 17);
        let terms = vec!["x0".to_string(), "x1".to_string()];
        let (fit, selection) = fit_fusion(
            &time,
            &status,
            &cov1,
            &terms,
            0,
            Some(1e-8),
            &[],
            5,
            None,
            0,
            1e-8,
            300,
        )
        .unwrap();
        assert!(selection.is_none());
        assert!(fit.converged);
        assert!(
            fit.coef[0] > 0.3,
            "positive signal recovered: {}",
            fit.coef[0]
        );
        assert!(
            fit.coef[1] < -0.3,
            "negative signal recovered: {}",
            fit.coef[1]
        );
    }

    #[test]
    fn lambda_selection_picks_from_the_grid() {
        let (time, status, cov1) = simulate(300, &[0.7], 29);
        let terms = vec!["x0".to_string()];
        let (_, selection) = fit_fusion(
            &time,
            &status,
            &cov1,
            &terms,
            0,
            None,
            &[0.1, 1.0, 10.0],
            4,
            Some(30.0),
            3,
            1e-7,
            300,
        )
        .unwrap();
        let sel = selection.expect("selection performed");
        assert!([0.1, 1.0, 10.0].contains(&sel.best));
        assert_eq!(sel.mean_brier.len(), 3);
        assert!(sel.mean_brier.iter().all(|(_, m)| m.is_finite()));
    }

    #[test]
    fn spec_validation_rejects_bad_configs() {
        let base = serde_json::json!({
            "time_column": "t", "status_column": "s",
            "covariates": ["score_a", "score_b"],
            "clinical": ["age"],
            "lambda": 2.0
        });
        assert!(
            serde_json::from_value::<FusionFitSpec>(base.clone())
                .unwrap()
                .validate()
                .is_ok()
        );

        // No fixed lambda and no grid → rejected.
        let mut no_lambda = base.clone();
        no_lambda["lambda"] = serde_json::Value::Null;
        let spec: FusionFitSpec = serde_json::from_value(no_lambda).unwrap();
        assert!(spec.validate().is_err());

        // Selection without horizon → rejected.
        let mut sel_no_horizon = base.clone();
        sel_no_horizon["lambda"] = serde_json::Value::Null;
        sel_no_horizon["lambda_grid"] = serde_json::json!([0.5, 5.0]);
        let spec: FusionFitSpec = serde_json::from_value(sel_no_horizon).unwrap();
        assert!(spec.validate().is_err());

        // lock_predict: negative lambda / non-positive horizon rejected.
        let lock: LockPredictSpec = serde_json::from_value(serde_json::json!({
            "time_column": "t", "status_column": "s",
            "covariates": ["score_a"], "lambda": -1.0, "horizon": 1095.0
        }))
        .unwrap();
        assert!(lock.validate().is_err());
        let lock: LockPredictSpec = serde_json::from_value(serde_json::json!({
            "time_column": "t", "status_column": "s",
            "covariates": ["score_a"], "lambda": 2.0, "horizon": 0.0
        }))
        .unwrap();
        assert!(lock.validate().is_err());
    }
}
