//! `nested_oof` — leak-free nested cross-validation for one modality's
//! ridge Fine–Gray model, emitting out-of-fold risk scores.
//!
//! The chordoma SAP forbids any evaluation-fold contact during model
//! development: within each **outer** fold, `λ` is chosen by **inner** CV on
//! the outer-training rows only (minimizing the mean inner-OOF IPCW Brier at
//! the horizon — the same metric as the locked endpoint), the model is then
//! refit on the full outer-training set and applied to the untouched outer
//! test rows. Every subject therefore receives exactly one out-of-fold score
//! produced by a model that never saw it.
//!
//! **Port 0** — the complete-case input rows plus:
//!
//! | Column        | Type    | Description                                        |
//! |---------------|---------|----------------------------------------------------|
//! | `oof_fold`    | UInt32  | Outer fold the subject was tested in               |
//! | `oof_lambda`  | Float64 | `λ` selected inside that outer fold                |
//! | `oof_score`   | Float64 | Out-of-fold linear predictor (risk score)          |
//! | `oof_prob`    | Float64 | Out-of-fold predicted CIF at `horizon`             |
//!
//! **Port 1** — one row per outer fold: `fold`, `n_train`, `n_test`,
//! `n_events_train`, `lambda_selected`, `inner_brier_at_lambda`,
//! `n_failed_inner_evals`, `converged`.
//!
//! The design matrix layout is `[clinical…, covariates…]` with the clinical
//! columns exempt from the ridge penalty — the pre-specified clinical model
//! always enters at full strength while the biomarker block shrinks.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, BooleanArray, Float64Array, RecordBatch, UInt32Array, UInt8Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use cmprsk::{CrrInput, CrrRidgeOptions, TimeFunctions, crr_ridge};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::cv::{cif_at, event_by_horizon_strata, select_lambda_inner, stratified_folds};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

/// Node kind string.
pub const NESTED_OOF_NODE_KIND: &str = "nested_oof";

fn d_failcode() -> f64 {
    1.0
}
fn d_cencode() -> f64 {
    0.0
}
fn d_folds() -> usize {
    5
}
fn d_true() -> bool {
    true
}
fn d_gtol() -> f64 {
    1e-6
}
fn d_maxiter() -> usize {
    200
}

/// Configuration for the `nested_oof` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NestedOofSpec {
    /// Failure / censoring time column (numeric, non-negative).
    pub time_column: String,
    /// Failure-type code column (`failcode` = event, `cencode` = censored,
    /// anything else = competing event).
    pub status_column: String,
    /// Penalized biomarker columns (radiomics / pathology scores).
    pub covariates: Vec<String>,
    /// Unpenalized clinical columns entering every fit at full strength.
    #[serde(default)]
    pub clinical: Vec<String>,
    /// Code of `status_column` denoting the event of interest.
    #[serde(default = "d_failcode")]
    pub failcode: f64,
    /// Code of `status_column` denoting censoring.
    #[serde(default = "d_cencode")]
    pub cencode: f64,
    /// Evaluation horizon `t*` (days) for the inner-CV IPCW Brier and the
    /// out-of-fold CIF column.
    pub horizon: f64,
    /// Candidate penalties; the inner CV picks one per outer fold.
    pub lambda_grid: Vec<f64>,
    /// Outer folds producing the out-of-fold scores.
    #[serde(default = "d_folds")]
    pub n_outer: usize,
    /// Inner folds selecting `λ` inside each outer-training set.
    #[serde(default = "d_folds")]
    pub n_inner: usize,
    /// Seed for the deterministic fold assignment.
    #[serde(default)]
    pub seed: u64,
    /// Stratify folds on the cause-1-event-by-horizon indicator. Recommended
    /// for rare events; unstratified folds can leave a fold event-free.
    #[serde(default = "d_true")]
    pub stratify: bool,
    /// Gradient tolerance for each ridge Fine–Gray fit.
    #[serde(default = "d_gtol")]
    pub gtol: f64,
    /// Maximum Newton iterations per fit.
    #[serde(default = "d_maxiter")]
    pub maxiter: usize,
}

impl NestedOofSpec {
    fn validate(&self) -> Result<(), String> {
        if self.covariates.is_empty() {
            return Err("covariates must be non-empty".into());
        }
        let overlap: Vec<&String> = self
            .clinical
            .iter()
            .filter(|c| self.covariates.contains(c))
            .collect();
        if !overlap.is_empty() {
            return Err(format!(
                "columns listed in both clinical and covariates: {}",
                overlap.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        if !self.horizon.is_finite() || self.horizon <= 0.0 {
            return Err("horizon must be finite and positive".into());
        }
        if self.lambda_grid.is_empty() {
            return Err("lambda_grid must be non-empty".into());
        }
        if self.lambda_grid.iter().any(|l| !l.is_finite() || *l < 0.0) {
            return Err("every lambda in lambda_grid must be finite and nonnegative".into());
        }
        if self.n_outer < 2 || self.n_inner < 2 {
            return Err("n_outer and n_inner must both be >= 2".into());
        }
        Ok(())
    }
}

/// One outer fold's bookkeeping.
pub(crate) struct FoldSummary {
    pub fold: u32,
    pub n_train: usize,
    pub n_test: usize,
    pub n_events_train: usize,
    pub lambda_selected: f64,
    pub inner_brier_at_lambda: f64,
    pub n_failed_inner_evals: usize,
    pub converged: bool,
}

/// The per-subject out-of-fold columns plus fold summaries.
pub(crate) struct OofOutcome {
    pub fold: Vec<u32>,
    pub lambda: Vec<f64>,
    pub score: Vec<f64>,
    pub prob: Vec<f64>,
    pub folds: Vec<FoldSummary>,
}

/// Run the nested CV on complete-case data already laid out as
/// `[clinical…, covariates…]`.
pub(crate) fn nested_oof_scores(
    time: &[f64],
    status_code: &[u8],
    cov1: &[Vec<f64>],
    terms: &[String],
    n_clinical: usize,
    spec_grid: &[f64],
    n_outer: usize,
    n_inner: usize,
    horizon: f64,
    seed: u64,
    stratify: bool,
    gtol: f64,
    maxiter: usize,
) -> Result<OofOutcome, String> {
    let n = time.len();
    let strata = if stratify {
        event_by_horizon_strata(time, status_code, horizon)
    } else {
        vec![0u8; n]
    };
    let outer = stratified_folds(&strata, n_outer, seed, 0);

    let mut fold_col = vec![0u32; n];
    let mut lambda_col = vec![0.0_f64; n];
    let mut score_col = vec![0.0_f64; n];
    let mut prob_col = vec![0.0_f64; n];
    let mut summaries = Vec::with_capacity(n_outer);

    for fold in 0..n_outer {
        let test_pos: Vec<usize> = (0..n).filter(|&i| outer[i] as usize == fold).collect();
        let train_pos: Vec<usize> = (0..n).filter(|&i| outer[i] as usize != fold).collect();
        if test_pos.is_empty() || train_pos.is_empty() {
            return Err(format!("outer fold {fold} is degenerate (empty train or test)"));
        }

        // λ selection sees outer-training rows only.
        let selection = select_lambda_inner(
            time,
            status_code,
            cov1,
            terms,
            n_clinical,
            &train_pos,
            spec_grid,
            n_inner,
            horizon,
            seed,
            fold + 1,
            gtol,
            maxiter,
        )?;
        let lambda = selection.best;
        let inner_brier = selection
            .mean_brier
            .iter()
            .find(|(l, _)| *l == lambda)
            .map(|(_, m)| *m)
            .unwrap_or(f64::NAN);

        // Refit on the full outer-training set with the selected λ.
        let t_train: Vec<f64> = train_pos.iter().map(|&i| time[i]).collect();
        let s_train: Vec<u8> = train_pos.iter().map(|&i| status_code[i]).collect();
        let x_train: Vec<Vec<f64>> = train_pos.iter().map(|&i| cov1[i].clone()).collect();
        let fit = crr_ridge(
            &CrrInput {
                ftime: &t_train,
                fstatus: &s_train.iter().map(|&s| f64::from(s)).collect::<Vec<_>>(),
                cov1: &x_train,
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
        .map_err(|e| format!("outer fold {fold}: ridge Fine-Gray refit failed: {e}"))?;

        let n_events_train = s_train.iter().filter(|&&s| s == 1).count();
        summaries.push(FoldSummary {
            fold: fold as u32,
            n_train: train_pos.len(),
            n_test: test_pos.len(),
            n_events_train,
            lambda_selected: lambda,
            inner_brier_at_lambda: inner_brier,
            n_failed_inner_evals: selection.n_failed_evals,
            converged: fit.converged,
        });

        for &i in &test_pos {
            fold_col[i] = fold as u32;
            lambda_col[i] = lambda;
            score_col[i] = fit.linear_predictor(&cov1[i]).map_err(|e| e.to_string())?;
            prob_col[i] = cif_at(&fit, &cov1[i], horizon)?;
        }
    }

    Ok(OofOutcome {
        fold: fold_col,
        lambda: lambda_col,
        score: score_col,
        prob: prob_col,
        folds: summaries,
    })
}

// ── node plumbing ───────────────────────────────────────────────────────────

pub struct NestedOofNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None) // 0: analysis table
        .add_output_port(None) // 0: rows + oof columns
        .add_output_port(None) // 1: outer-fold summary
}

impl NodeFactory for NestedOofNodeFactory {
    fn kind(&self) -> &'static str {
        NESTED_OOF_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Leak-free nested cross-validation producing out-of-fold risk scores for a ridge Fine-Gray model."
    }
    fn doc(&self) -> &'static str {
        "Within each outer fold the ridge penalty lambda is selected by inner \
        cross-validation on the outer-training rows only, minimizing the mean \
        inner out-of-fold IPCW Brier score at the horizon (the same competing-\
        risks metric as the locked endpoint). The model is then refit on the \
        full outer-training set with the selected lambda and applied to the \
        untouched outer test rows, so every subject gets exactly one \
        out-of-fold risk score and cumulative-incidence prediction from a \
        model that never saw it. Clinical columns are exempt from the \
        penalty. Port 0 carries the complete-case input rows plus oof_fold, \
        oof_lambda, oof_score, and oof_prob; port 1 carries one bookkeeping \
        row per outer fold."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(NestedOofSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: NestedOofSpec = serde_json::from_value(spec)?;
        if let Err(reason) = s.validate() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: NESTED_OOF_NODE_KIND.to_string(),
                reason,
                schema_pretty: serde_json::to_string_pretty(&schema_for!(NestedOofSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(NestedOofNode {
            meta: port_layout(),
            spec: s,
        }))
    }
}

#[derive(Clone)]
pub struct NestedOofNode {
    meta: NodePorts,
    spec: NestedOofSpec,
}

fn err(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: NESTED_OOF_NODE_KIND.into(),
        msg: msg.into(),
    }
}

/// Concatenate input batches column-wise, keep complete-case rows, append
/// the out-of-fold columns.
fn build_row_batch(
    batches: &[RecordBatch],
    keep: &[bool],
    outcome: &OofOutcome,
) -> Result<RecordBatch, DagError> {
    let batch0 = batches.first().ok_or_else(|| err("no input rows"))?;
    let schema = batch0.schema();
    let mask = BooleanArray::from(keep.to_vec());
    let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(fields.len() + 4);
    for col_i in 0..fields.len() {
        let chunks: Vec<&dyn Array> = batches.iter().map(|b| b.column(col_i).as_ref()).collect();
        let combined = arrow_select::concat::concat(&chunks)
            .map_err(|e| err(format!("concat input columns: {e}")))?;
        let filtered = arrow_select::filter::filter(&combined, &mask)
            .map_err(|e| err(format!("filter complete-case rows: {e}")))?;
        arrays.push(filtered);
    }
    fields.push(Arc::new(Field::new("oof_fold", DataType::UInt32, false)));
    arrays.push(Arc::new(UInt32Array::from(outcome.fold.clone())));
    fields.push(Arc::new(Field::new("oof_lambda", DataType::Float64, false)));
    arrays.push(Arc::new(Float64Array::from(outcome.lambda.clone())));
    fields.push(Arc::new(Field::new("oof_score", DataType::Float64, false)));
    arrays.push(Arc::new(Float64Array::from(outcome.score.clone())));
    fields.push(Arc::new(Field::new("oof_prob", DataType::Float64, false)));
    arrays.push(Arc::new(Float64Array::from(outcome.prob.clone())));
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|e| err(format!("build row batch: {e}")))
}

fn build_fold_batch(outcome: &OofOutcome) -> Result<RecordBatch, DagError> {
    let rows = &outcome.folds;
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("fold", DataType::UInt32, false),
            Field::new("n_train", DataType::UInt32, false),
            Field::new("n_test", DataType::UInt32, false),
            Field::new("n_events_train", DataType::UInt32, false),
            Field::new("lambda_selected", DataType::Float64, false),
            Field::new("inner_brier_at_lambda", DataType::Float64, false),
            Field::new("n_failed_inner_evals", DataType::UInt32, false),
            Field::new("converged", DataType::Boolean, false),
        ])),
        vec![
            Arc::new(UInt32Array::from(rows.iter().map(|r| r.fold).collect::<Vec<_>>())),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.n_train as u32).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.n_test as u32).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.n_events_train as u32).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.lambda_selected).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.inner_brier_at_lambda).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.n_failed_inner_evals as u32).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.converged).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| err(format!("build fold batch: {e}")))
}

#[async_trait]
impl DagNode for NestedOofNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        NESTED_OOF_NODE_KIND
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
            .ok_or_else(|| err("no input connected"))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| err(format!("collect: {e}")))?;

        let s = &self.spec;
        let time = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.time_column)
            .map_err(|e| err(e.to_string()))?;
        let fstatus = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.status_column)
            .map_err(|e| err(e.to_string()))?;

        // Design matrix in [clinical…, covariates…] layout.
        let layout: Vec<String> = s.clinical.iter().chain(s.covariates.iter()).cloned().collect();
        let mut columns: Vec<Vec<f64>> = Vec::with_capacity(layout.len());
        for name in &layout {
            columns.push(
                dag_core::arrow_util::extract_numeric_lenient(&batches, name)
                    .map_err(|e| err(e.to_string()))?,
            );
        }
        let n = time.len();
        let n_clinical = s.clinical.len();

        // Complete-case filter: every used column finite.
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
        if n - n_dropped < s.n_outer {
            return Err(err(format!(
                "only {} complete cases for {} outer folds",
                n - n_dropped,
                s.n_outer
            )));
        }

        let pos: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
        let time_cc: Vec<f64> = pos.iter().map(|&i| time[i]).collect();
        let status_cc = crate::cv::recode_status(
            &(pos.iter().map(|&i| fstatus[i]).collect::<Vec<_>>()),
            s.failcode,
            s.cencode,
        )
        .map_err(err)?;
        let cov1: Vec<Vec<f64>> = pos
            .iter()
            .map(|&i| columns.iter().map(|c| c[i]).collect())
            .collect();

        let outcome = nested_oof_scores(
            &time_cc,
            &status_cc,
            &cov1,
            &layout,
            n_clinical,
            &s.lambda_grid,
            s.n_outer,
            s.n_inner,
            s.horizon,
            s.seed,
            s.stratify,
            s.gtol,
            s.maxiter,
        )
        .map_err(err)?;

        let row_batch = build_row_batch(&batches, &keep, &outcome)?;
        let fold_batch = build_fold_batch(&outcome)?;

        let ctx = node_ctx.session();
        let mut res = PortOutputs::new();
        res.insert(
            0,
            ctx.read_batch(row_batch).map_err(|e| err(format!("read_batch(0): {e}")))?,
        );
        res.insert(
            1,
            ctx.read_batch(fold_batch).map_err(|e| err(format!("read_batch(1): {e}")))?,
        );
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: serde_json::Value) -> NestedOofSpec {
        serde_json::from_value(json).expect("spec parses")
    }

    /// Deterministic competing-risks sample (same simulator family as the
    /// cmprsk ridge tests): latent exponential cause-1 with signal in the
    /// first covariates, cause-2, independent censoring.
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
    fn defaults_and_validation() {
        let s = spec(serde_json::json!({
            "time_column": "t", "status_column": "s", "covariates": ["x1"],
            "horizon": 1095.0, "lambda_grid": [0.1, 1.0, 10.0]
        }));
        assert_eq!(s.n_outer, 5);
        assert_eq!(s.n_inner, 5);
        assert_eq!(s.seed, 0);
        assert!(s.stratify);
        assert_eq!(s.maxiter, 200);
        assert!(s.validate().is_ok());

        // `covariates` is a required field: an omitted list fails at the
        // schema boundary, before validate() ever runs.
        assert!(
            serde_json::from_value::<NestedOofSpec>(serde_json::json!({
                "time_column": "t", "status_column": "s",
                "horizon": 1095.0, "lambda_grid": [1.0]
            }))
            .is_err(),
            "omitted covariates rejected at deserialization"
        );

        assert!(spec(serde_json::json!({
            "time_column": "t", "status_column": "s", "covariates": ["x"],
            "clinical": ["x"], "horizon": 1095.0, "lambda_grid": [1.0]
        }))
        .validate()
        .is_err(), "clinical/covariates overlap rejected");

        assert!(spec(serde_json::json!({
            "time_column": "t", "status_column": "s", "covariates": ["x"],
            "horizon": 1095.0, "lambda_grid": []
        }))
        .validate()
        .is_err(), "empty grid rejected");
    }

    #[test]
    fn oof_scores_are_deterministic_and_discriminative() {
        let (time, status, cov1) = simulate(240, &[0.9, 0.0, 0.0], 11);
        let terms: Vec<String> = (0..3).map(|j| format!("x{j}")).collect();
        let args = |seed: u64| {
            nested_oof_scores(
                &time, &status, &cov1, &terms, 0, &[0.5, 5.0], 4, 3, 30.0, seed, true, 1e-7, 200,
            )
        };
        let a = args(7).unwrap();
        let b = args(7).unwrap();
        assert_eq!(a.fold, b.fold, "same seed reproduces folds");
        assert_eq!(a.score, b.score, "same seed reproduces scores");
        assert_eq!(a.prob.len(), time.len());
        assert!(a.prob.iter().all(|p| (0.0..=1.0).contains(p)));

        // Every subject appears in exactly one test fold.
        for f in 0..4u32 {
            assert!(a.fold.contains(&f));
        }
        assert_eq!(a.folds.len(), 4);
        assert!(a.folds.iter().all(|f| f.n_test > 0));

        // Strong signal → OOF scores must rank events above non-events more
        // often than not (IPCW-free crude check).
        let mut concordant = 0.0_f64;
        let mut total = 0.0_f64;
        for i in 0..time.len() {
            for j in 0..time.len() {
                if status[i] == 1 && status[j] != 1 && time[i] <= 30.0 {
                    total += 1.0;
                    if a.score[i] > a.score[j] {
                        concordant += 1.0;
                    }
                }
            }
        }
        let frac = concordant / total;
        assert!(frac > 0.6, "OOF scores must discriminate, got {frac}");
    }
}
