//! `svy_rcs` — design-based restricted cubic splines.
//!
//! The `epi_rcs` node fits an RCS with sampling weights but model-based
//! (pseudo-likelihood) inference; this node plugs the same Harrell basis
//! (Stone–Koo tail constraints, [`epi::rcs`]) into a full survey design:
//! the knot placement uses weighted percentiles of the design weights, the
//! fit is `survey::svyglm` (design-based sandwich covariance, design df),
//! and the nonlinearity / overall tests are Wald tests on the design
//! covariance — the R `svyglm(y ~ rcs(x, k) + C, design, family=...)`
//! workflow with `regTermTest` inference.
//!
//! Ports: `0` in — input table; `0` out — single-row summary
//! (nonlinear/overall tests, knots, peak); `1` out — dose-response curve
//! grid with design-based SEs.

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use statrs::distribution::{ContinuousCDF, StudentsT};

use crate::survey_common::{
    SurveyDesignSpec, SurveyDomainSpec, build_survey_design, one_in_two_out, survey_domain_mask,
};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

fn default_n_knots() -> usize {
    4
}
fn default_rcs_family() -> String {
    "gaussian".to_string()
}
fn default_grid() -> usize {
    100
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyRcsSpec {
    pub design: SurveyDesignSpec,
    /// Restrict the analysis (and the design) to rows where this column holds
    /// one of the listed values — R `subset=` domain semantics.
    #[serde(default)]
    pub survey_domain: Option<SurveyDomainSpec>,
    /// Continuous predictor to expand as a restricted cubic spline.
    pub x_column: String,
    /// Outcome column (continuous for gaussian; 0/1 for binomial families).
    pub outcome_column: String,
    /// Adjust-for covariates (linear terms), held at their weighted means
    /// on the curve.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Number of knots (3–7; Hmisc supports 3/4/5/7). Ignored when explicit
    /// `knots` are given.
    #[serde(default = "default_n_knots")]
    pub n_knots: usize,
    /// Explicit knot positions (overrides `n_knots`; must be ≥ 3, ascending).
    #[serde(default)]
    pub knots: Option<Vec<f64>>,
    /// GLM family: `"gaussian"` (default), `"binomial"`, `"quasibinomial"`.
    #[serde(default = "default_rcs_family")]
    pub family: String,
    /// Optional non-canonical link.
    #[serde(default)]
    pub link: Option<String>,
    /// Curve grid resolution (default 100 → 101 rows).
    #[serde(default = "default_grid")]
    pub n_grid_points: usize,
}

#[derive(Clone)]
pub struct SvyRcsNode {
    meta: NodePorts,
    spec: SvyRcsSpec,
}

impl SvyRcsNode {
    pub fn new(spec: SvyRcsSpec) -> Self {
        Self {
            meta: one_in_two_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyRcsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svy_rcs"
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
        let ne = |msg: String| DagError::NodeError {
            node_type: "svy_rcs".into(),
            msg,
        };

        let family_spec = survey::FamilySpec::new(&self.spec.family, self.spec.link.as_deref())
            .ok_or_else(|| {
                ne(format!(
                    "unsupported family='{}' link='{:?}'",
                    self.spec.family, self.spec.link
                ))
            })?;

        let input = inputs.first().ok_or_else(|| ne("no input data".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| ne(format!("collect failed: {e}")))?;

        let domain_mask = match &self.spec.survey_domain {
            Some(spec) => survey_domain_mask(&batches, spec).map_err(|e| ne(e.to_string()))?,
            None => vec![true; batches.iter().map(|batch| batch.num_rows()).sum()],
        };
        if !domain_mask.iter().any(|&keep| keep) {
            return Err(ne("survey domain selected no rows".into()));
        }

        let design =
            build_survey_design(&self.spec.design, &batches).map_err(|e| ne(e.to_string()))?;
        let mut y_all =
            crate::survey_common::extract_variables(&batches, &[self.spec.outcome_column.clone()])
                .map_err(|e| ne(e.to_string()))?;
        let x_all =
            crate::survey_common::extract_variables(&batches, &[self.spec.x_column.clone()])
                .map_err(|e| ne(e.to_string()))?;
        let cov_all = crate::survey_common::extract_variables(&batches, &self.spec.covariates)
            .map_err(|e| ne(e.to_string()))?;

        // Validate the in-domain outcome for the family.
        let domain_y: Vec<f64> = y_all[0]
            .iter()
            .zip(&domain_mask)
            .filter(|(_, keep)| **keep)
            .map(|(value, _)| *value)
            .collect();
        if let Err(msg) = family_spec.validate_y(&domain_y) {
            return Err(ne(msg));
        }

        // NaN the response outside the domain / where x or any covariate is
        // missing, so svyglm's row filter implements the complete-case set.
        let n_rows = y_all[0].len();
        for row in 0..n_rows {
            if !domain_mask[row]
                || x_all[0][row].is_nan()
                || cov_all.iter().any(|c| c[row].is_nan())
            {
                y_all[0][row] = f64::NAN;
            }
        }

        // Complete-case rows drive knot placement and the curve's weighted
        // covariate means.
        let keep: Vec<usize> = (0..n_rows)
            .filter(|&row| {
                domain_mask[row]
                    && y_all[0][row].is_finite()
                    && x_all[0][row].is_finite()
                    && cov_all.iter().all(|c| c[row].is_finite())
            })
            .collect();
        if keep.len() < 5 {
            return Err(ne(format!("too few complete cases ({})", keep.len())));
        }

        let weights = design.weights();
        let x_kept: Vec<f64> = keep.iter().map(|&i| x_all[0][i]).collect();
        let w_kept: Vec<f64> = keep.iter().map(|&i| weights[i]).collect();

        // Knots: explicit, or Harrell's weighted percentile placement.
        let knots = match &self.spec.knots {
            Some(k) => {
                if k.len() < 3 {
                    return Err(ne("explicit knots must have ≥ 3 positions".into()));
                }
                if !k.windows(2).all(|w| w[0] < w[1]) {
                    return Err(ne("explicit knots must be strictly ascending".into()));
                }
                k.clone()
            }
            None => epi::rcs::knot_positions_weighted(&x_kept, Some(&w_kept), self.spec.n_knots)
                .map_err(|e| ne(e.to_string()))?,
        };
        let k = knots.len();

        // Basis on the full column (rows with NaN x produce NaN basis and are
        // dropped by svyglm together with the NaN-ed response).
        let basis = epi::rcs::rcs_basis(&x_all[0], &knots); // k − 2 columns

        // Design columns: [x, basis…, covariates…].
        let mut x_cols: Vec<Vec<f64>> = Vec::with_capacity(1 + basis.len() + cov_all.len());
        x_cols.push(x_all[0].clone());
        for col in &basis {
            x_cols.push(col.clone());
        }
        for col in &cov_all {
            x_cols.push(col.clone());
        }

        let fit = survey::svyglm(&y_all[0], &x_cols, &design, true, None, &family_spec, None)
            .map_err(|e| ne(e.to_string()))?;

        // Nonlinearity: joint Wald test on the k−2 basis columns
        // (coefficients 2..2+(k−2), 0-based after the intercept and linear x).
        let nonlinear_indices: Vec<usize> = (2..2 + basis.len()).collect();
        let nonlinear =
            survey::reg_term_test_with(&fit, &nonlinear_indices, survey::RegTermTestMethod::Wald)
                .map_err(|e| ne(e.to_string()))?;
        // Overall association: every non-intercept coefficient.
        let overall_indices: Vec<usize> = (1..fit.coefficients.len()).collect();
        let overall =
            survey::reg_term_test_with(&fit, &overall_indices, survey::RegTermTestMethod::Wald)
                .map_err(|e| ne(e.to_string()))?;

        // ── Curve: η = X₀β with design-based SE, covariates at weighted means.
        let w_sum: f64 = w_kept.iter().sum();
        let cov_wmeans: Vec<f64> = cov_all
            .iter()
            .map(|c| {
                keep.iter()
                    .zip(&w_kept)
                    .map(|(&i, &w)| w * c[i])
                    .sum::<f64>()
                    / w_sum
            })
            .collect();

        let x_min = x_kept.iter().cloned().fold(f64::INFINITY, f64::min);
        let x_max = x_kept.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let steps = self.spec.n_grid_points.max(1);
        let t_crit = StudentsT::new(0.0, 1.0, fit.df as f64)
            .map(|d| d.inverse_cdf(0.975))
            .unwrap_or(1.96);
        let is_binomial = matches!(
            family_spec.family,
            survey::family::Family::Binomial | survey::family::Family::QuasiBinomial
        );

        let mut grid_x: Vec<f64> = Vec::with_capacity(steps + 1);
        let mut grid_eta: Vec<f64> = Vec::with_capacity(steps + 1);
        let mut grid_se: Vec<f64> = Vec::with_capacity(steps + 1);
        for step in 0..=steps {
            let g = x_min + (x_max - x_min) * step as f64 / steps as f64;
            let g_basis = epi::rcs::rcs_basis(&[g], &knots);
            // X₀ = [1, g, basis(g)…, covariate weighted means…]
            let mut x0: Vec<f64> = Vec::with_capacity(fit.coefficients.len());
            x0.push(1.0);
            x0.push(g);
            for col in &g_basis {
                x0.push(col[0]);
            }
            for &m in &cov_wmeans {
                x0.push(m);
            }
            x0.truncate(fit.coefficients.len());
            let eta: f64 = x0.iter().zip(&fit.coefficients).map(|(a, b)| a * b).sum();
            let mut var = 0.0_f64;
            for (i, &ai) in x0.iter().enumerate() {
                for (j, &aj) in x0.iter().enumerate() {
                    var += ai * fit.design_cov[i][j] * aj;
                }
            }
            grid_x.push(g);
            grid_eta.push(eta);
            grid_se.push(var.max(0.0).sqrt());
        }

        // Peak of the dose-response on the grid (max η).
        let peak_x = grid_x
            .iter()
            .zip(&grid_eta)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(g, _)| *g)
            .unwrap_or(f64::NAN);

        // ── Port 0: single-row summary.
        let n_obs = fit.n;
        let mut fields = vec![
            Field::new("nonlinear_stat", DataType::Float64, false),
            Field::new("nonlinear_ndf", DataType::Float64, false),
            Field::new("nonlinear_ddf", DataType::Float64, false),
            Field::new("nonlinear_p", DataType::Float64, false),
            Field::new("overall_stat", DataType::Float64, false),
            Field::new("overall_ndf", DataType::Float64, false),
            Field::new("overall_ddf", DataType::Float64, false),
            Field::new("overall_p", DataType::Float64, false),
            Field::new("n_knots", DataType::Int32, false),
            Field::new("n_obs", DataType::Int32, false),
            Field::new("peak_x", DataType::Float64, false),
        ];
        // Fitted coefficients: intercept, linear x, basis columns, covariates.
        let mut coef_names: Vec<String> = Vec::with_capacity(fit.coefficients.len());
        coef_names.push("coef_intercept".to_string());
        coef_names.push("coef_x".to_string());
        for b in 0..basis.len() {
            coef_names.push(format!("coef_basis_{b}"));
        }
        for cov in &self.spec.covariates {
            coef_names.push(format!("coef_{cov}"));
        }
        for name in &coef_names {
            fields.push(Field::new(name.clone(), DataType::Float64, false));
        }
        for i in 0..knots.len() {
            fields.push(Field::new(format!("knot_{i}"), DataType::Float64, false));
        }
        let mut cols: Vec<Arc<dyn arrow_array::Array>> = vec![
            Arc::new(Float64Array::from(vec![nonlinear.statistic])),
            Arc::new(Float64Array::from(vec![nonlinear.ndf as f64])),
            Arc::new(Float64Array::from(vec![nonlinear.ddf as f64])),
            Arc::new(Float64Array::from(vec![nonlinear.p_value])),
            Arc::new(Float64Array::from(vec![overall.statistic])),
            Arc::new(Float64Array::from(vec![overall.ndf as f64])),
            Arc::new(Float64Array::from(vec![overall.ddf as f64])),
            Arc::new(Float64Array::from(vec![overall.p_value])),
            Arc::new(Int32Array::from(vec![k as i32])),
            Arc::new(Int32Array::from(vec![n_obs as i32])),
            Arc::new(Float64Array::from(vec![peak_x])),
        ];
        for &c in &fit.coefficients {
            cols.push(Arc::new(Float64Array::from(vec![c])));
        }
        for knot in &knots {
            cols.push(Arc::new(Float64Array::from(vec![*knot])));
        }
        let summary = RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
            .map_err(|e| ne(format!("summary batch: {e}")))?;

        // ── Port 1: curve grid.
        let curve_rows = steps + 1;
        let lo: Vec<f64> = grid_eta
            .iter()
            .zip(&grid_se)
            .map(|(e, s)| e - t_crit * s)
            .collect();
        let hi: Vec<f64> = grid_eta
            .iter()
            .zip(&grid_se)
            .map(|(e, s)| e + t_crit * s)
            .collect();
        let mut curve_fields = vec![
            Field::new("x", DataType::Float64, false),
            Field::new("eta", DataType::Float64, false),
            Field::new("se", DataType::Float64, false),
            Field::new("eta_lower", DataType::Float64, false),
            Field::new("eta_upper", DataType::Float64, false),
        ];
        let mut curve_cols: Vec<Arc<dyn arrow_array::Array>> = vec![
            Arc::new(Float64Array::from(grid_x.clone())),
            Arc::new(Float64Array::from(grid_eta.clone())),
            Arc::new(Float64Array::from(grid_se.clone())),
            Arc::new(Float64Array::from(lo)),
            Arc::new(Float64Array::from(hi)),
        ];
        if is_binomial {
            let exp_v = |v: &Vec<f64>| -> Vec<f64> { v.iter().map(|e| e.exp()).collect() };
            let or_lo: Vec<f64> = grid_eta
                .iter()
                .zip(&grid_se)
                .map(|(e, s)| (e - t_crit * s).exp())
                .collect();
            let or_hi: Vec<f64> = grid_eta
                .iter()
                .zip(&grid_se)
                .map(|(e, s)| (e + t_crit * s).exp())
                .collect();
            curve_fields.push(Field::new("or", DataType::Float64, false));
            curve_fields.push(Field::new("or_lower", DataType::Float64, false));
            curve_fields.push(Field::new("or_upper", DataType::Float64, false));
            curve_cols.push(Arc::new(Float64Array::from(exp_v(&grid_eta))));
            curve_cols.push(Arc::new(Float64Array::from(or_lo)));
            curve_cols.push(Arc::new(Float64Array::from(or_hi)));
        }
        let curve = RecordBatch::try_new(Arc::new(Schema::new(curve_fields)), curve_cols)
            .map_err(|e| ne(format!("curve batch: {e}")))?;
        let _ = curve_rows;

        let ctx = node_ctx.session();
        let df_summary = ctx
            .read_batch(summary)
            .map_err(|e| ne(format!("read_batch failed: {e}")))?;
        let df_curve = ctx
            .read_batch(curve)
            .map_err(|e| ne(format!("read_batch failed: {e}")))?;
        let mut res = PortOutputs::new();
        res.insert(0, df_summary);
        res.insert(1, df_curve);
        Ok(res)
    }
}

pub struct SvyRcsFactory;

impl NodeFactory for SvyRcsFactory {
    fn kind(&self) -> &'static str {
        "svy_rcs"
    }
    fn desc(&self) -> &'static str {
        "Design-based restricted cubic splines (svyglm + rcs basis)."
    }
    fn doc(&self) -> &'static str {
        "Expands a continuous predictor into a Harrell restricted cubic \
         spline basis (weighted-percentile knots by default), fits it with \
         survey::svyglm under the full survey design (design-based sandwich \
         covariance and df), and reports Wald nonlinearity / overall tests \
         plus a dose-response curve grid with design-based standard errors. \
         Families: gaussian (continuous outcome), binomial / quasibinomial \
         (0-1 outcome; curve reports odds ratios)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyRcsSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_two_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: SvyRcsSpec = serde_json::from_value(spec)?;
        let reject = |reason: String| dag_core::registry::error::Error::SpecRejection {
            kind: "svy_rcs".to_string(),
            reason,
            schema_pretty: serde_json::to_string_pretty(&self.spec_schema()).unwrap_or_default(),
        };
        if survey::FamilySpec::new(&node_spec.family, node_spec.link.as_deref()).is_none() {
            return Err(reject(format!(
                "unsupported family='{}' link='{:?}'",
                node_spec.family, node_spec.link
            )));
        }
        if node_spec.knots.is_none() && node_spec.n_knots < 3 {
            return Err(reject("n_knots must be ≥ 3".into()));
        }
        Ok(Box::new(SvyRcsNode::new(node_spec)))
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

    /// U-shaped gaussian system with strata/PSU/weights + a covariate.
    fn make_batch(binomial: bool) -> RecordBatch {
        let n = 300;
        let strata: Vec<i64> = (0..n as i64).map(|i| i / 50).collect();
        let psu: Vec<i64> = (0..n as i64).map(|i| i / 10).collect();
        let x: Vec<f64> = (0..n)
            .map(|i| 3.0 + 7.0 * (i as f64 * 0.6180339887498949).fract())
            .collect();
        let cc: Vec<f64> = (0..n)
            .map(|i| (i as f64 * 0.4142135623730951).fract() - 0.5)
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| {
                let u = (i as f64 * 0.7548776662466927).fract();
                if binomial {
                    let eta = -1.5 + 0.15 * (x[i] - 6.5) * (x[i] - 6.5) + 0.8 * cc[i];
                    if u < 1.0 / (1.0 + (-eta).exp()) {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    8.0 - 0.4 * (x[i] - 6.5) * (x[i] - 6.5) + 0.8 * cc[i] + (u - 0.5) * 2.0
                }
            })
            .collect();
        let wt: Vec<f64> = (0..n)
            .map(|i| 1000.0 + 500.0 * (i as f64 * 0.6180339887498949).fract())
            .collect();
        let to_arr = |v: Vec<f64>| Arc::new(Float64Array::from(v)) as Arc<dyn arrow_array::Array>;
        let to_int =
            |v: Vec<i64>| Arc::new(arrow_array::Int64Array::from(v)) as Arc<dyn arrow_array::Array>;
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("strata", DataType::Int64, false),
                Field::new("psu", DataType::Int64, false),
                Field::new("x", DataType::Float64, false),
                Field::new("cc", DataType::Float64, false),
                Field::new("y", DataType::Float64, false),
                Field::new("wt", DataType::Float64, false),
            ])),
            vec![
                to_int(strata),
                to_int(psu),
                to_arr(x),
                to_arr(cc),
                to_arr(y),
                to_arr(wt),
            ],
        )
        .unwrap()
    }

    async fn run(spec: serde_json::Value, binomial: bool) -> (Vec<RecordBatch>, Vec<RecordBatch>) {
        let mut node = SvyRcsFactory.build(spec, node_ctx()).unwrap();
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(make_batch(binomial))
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
        let summary = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
        let curve = outs.dataframe(1).unwrap().clone().collect().await.unwrap();
        (summary, curve)
    }

    fn cell(rows: &[RecordBatch], name: &str) -> f64 {
        let batch = &rows[0];
        let col = batch.column(batch.schema().index_of(name).unwrap());
        if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
            a.value(0)
        } else {
            col.as_any().downcast_ref::<Int32Array>().unwrap().value(0) as f64
        }
    }

    #[tokio::test]
    async fn gaussian_u_shape_detected() {
        let (summary, curve) = run(
            serde_json::json!({
                "design": {"ids": ["psu"], "strata": ["strata"], "weights": "wt"},
                "x_column": "x",
                "outcome_column": "y",
                "covariates": ["cc"],
                "n_knots": 4,
            }),
            false,
        )
        .await;
        // Strong U-shape → nonlinearity significant and tight.
        assert!(cell(&summary, "nonlinear_p") < 0.01);
        assert!(cell(&summary, "overall_p") < 0.01);
        assert_eq!(cell(&summary, "n_knots") as usize, 4);
        assert_eq!(cell(&summary, "n_obs") as usize, 300);
        assert!(summary[0].schema().index_of("knot_3").is_ok());
        // Inverted-U: the eta peak sits mid-range (~6.5).
        let peak = cell(&summary, "peak_x");
        assert!((5.5..7.5).contains(&peak), "peak {peak}");
        // Curve: 101 rows, monotone x, CIs bracket the estimate.
        assert_eq!(curve.iter().map(|b| b.num_rows()).sum::<usize>(), 101);
        let x_col = curve[0]
            .column(curve[0].schema().index_of("x").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(x_col.value(0) < x_col.value(100));
        let eta = curve[0]
            .column(curve[0].schema().index_of("eta").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let lo = curve[0]
            .column(curve[0].schema().index_of("eta_lower").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let hi = curve[0]
            .column(curve[0].schema().index_of("eta_upper").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in [0, 25, 50, 75, 100] {
            assert!(lo.value(i) <= eta.value(i) + 1e-9);
            assert!(eta.value(i) <= hi.value(i) + 1e-9);
        }
        // Gaussian curve has no OR columns.
        assert!(curve[0].schema().index_of("or").is_err());
    }

    #[tokio::test]
    async fn binomial_curve_has_odds_ratios() {
        let (summary, curve) = run(
            serde_json::json!({
                "design": {"ids": ["psu"], "strata": ["strata"], "weights": "wt"},
                "x_column": "x",
                "outcome_column": "y",
                "covariates": ["cc"],
                "family": "quasibinomial",
                "knots": [4.0, 5.5, 8.0, 9.5],
                "n_grid_points": 20,
            }),
            true,
        )
        .await;
        assert!(cell(&summary, "nonlinear_p") < 0.05);
        assert_eq!(curve.iter().map(|b| b.num_rows()).sum::<usize>(), 21);
        for name in ["or", "or_lower", "or_upper"] {
            assert!(curve[0].schema().index_of(name).is_ok(), "{name} missing");
        }
        let or = curve[0]
            .column(curve[0].schema().index_of("or").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let or_lo = curve[0]
            .column(curve[0].schema().index_of("or_lower").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(or.value(0) > or_lo.value(0));
        // OR = exp(eta) exactly.
        let eta = curve[0]
            .column(curve[0].schema().index_of("eta").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((or.value(7) - eta.value(7).exp()).abs() < 1e-9);
    }

    #[tokio::test]
    async fn domain_subset_and_explicit_knots_respected() {
        // Noisy binary domain column.
        let spec = serde_json::json!({
            "design": {"ids": ["psu"], "strata": ["strata"], "weights": "wt"},
            "x_column": "x",
            "outcome_column": "y",
            "survey_domain": {"column": "strata", "values": ["0", "1", "2", "3"]},
            "knots": [4.0, 6.5, 9.0],
        });
        let (summary, _curve) = run(spec, false).await;
        // 4 strata × 50 rows kept.
        assert_eq!(cell(&summary, "n_obs") as usize, 200);
        assert_eq!(cell(&summary, "n_knots") as usize, 3);
        assert!((cell(&summary, "knot_0") - 4.0).abs() < 1e-12);
    }

    #[tokio::test]
    async fn bad_family_rejected_at_build() {
        let err = SvyRcsFactory.build(
            serde_json::json!({
                "design": {"ids": ["psu"]},
                "x_column": "x",
                "outcome_column": "y",
                "family": "weibull",
            }),
            node_ctx(),
        );
        assert!(err.is_err());
    }
}
