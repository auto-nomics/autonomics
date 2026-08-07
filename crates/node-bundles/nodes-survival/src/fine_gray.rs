//! Fine–Gray proportional subdistribution hazards regression node.
//!
//! Wraps [`cmprsk::crr`] — the Rust port of R `cmprsk::crr()` (Fine & Gray,
//! JASA 1999). Unlike a cause-specific Cox model (see
//! [`cox_regression`](super::cox_regression)), this models the *subdistribution*
//! hazard, so its coefficients map directly onto the cumulative incidence of
//! the event of interest in the presence of competing risks.
//!
//! Two output ports:
//!
//! **Port 0** — one row per coefficient:
//!
//! | Column             | Type    | Description                                |
//! |--------------------|---------|--------------------------------------------|
//! | `term`             | Utf8    | Covariate name                              |
//! | `coefficient`      | Float64 | β̂                                          |
//! | `subhazard_ratio`  | Float64 | exp(β̂) — the SHR                           |
//! | `std_error`        | Float64 | SE(β̂) from the robust sandwich             |
//! | `z_stat`           | Float64 | β̂ / SE                                     |
//! | `p_value`          | Float64 | Two-sided Wald p-value                      |
//! | `shr_ci_lower`     | Float64 | Lower confidence limit for the SHR          |
//! | `shr_ci_upper`     | Float64 | Upper confidence limit for the SHR          |
//! | `log_likelihood`   | Float64 | Log pseudo-likelihood at β̂                 |
//! | `loglik_null`      | Float64 | Log pseudo-likelihood at β = 0              |
//! | `lr_stat`          | Float64 | −2 · (loglik_null − loglik)                 |
//! | `lr_df`            | Int32   | Degrees of freedom                          |
//! | `lr_p_value`       | Float64 | χ² p-value for the LR test                  |
//! | `n_obs`            | Int32   | Observations used                           |
//! | `n_missing`        | Int32   | Observations dropped for missing values     |
//! | `n_events`         | Int32   | Failures of the cause of interest           |
//! | `converged`        | Boolean | Whether Newton–Raphson converged            |
//!
//! **Port 1** — one row per unique failure time:
//!
//! | Column         | Type    | Description                                     |
//! |----------------|---------|-------------------------------------------------|
//! | `uftime`       | Float64 | Unique failure time                              |
//! | `bfitj`        | Float64 | Jump in the baseline cumulative subdist. hazard  |
//! | `baseline_cif` | Float64 | `1 − exp(−cumsum(bfitj))`                        |

use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use cmprsk::{CrrFit, CrrInput, CrrOptions, TimeFn, TimeFunctions};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::arrow_util::ColumnError;
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

/// Node kind string.
pub const FINE_GRAY_NODE_KIND: &str = "fine_gray";

#[derive(Debug, Error)]
pub enum FineGrayError {
    #[error("{0}")]
    Column(String),
    #[error("Fine-Gray fit failed: {0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
    #[error("invalid spec: {0}")]
    Spec(String),
}

impl From<ColumnError> for FineGrayError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for FineGrayError {
    fn node_type(&self) -> &str { FINE_GRAY_NODE_KIND }
}

// ── spec ────────────────────────────────────────────────────────────────────

fn default_failcode() -> f64 {
    1.0
}
fn default_cencode() -> f64 {
    0.0
}
fn default_gtol() -> f64 {
    1e-6
}
fn default_maxiter() -> usize {
    10
}
fn default_true() -> bool {
    true
}
fn default_conf_level() -> f64 {
    0.95
}

/// Configuration for the `fine_gray` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FineGrayNodeSpec {
    /// Failure / censoring time column (numeric, non-negative).
    pub time_column: String,
    /// Failure-type code column. One value marks the event of interest
    /// (`failcode`), one marks censoring (`cencode`), any other value is a
    /// competing event.
    pub status_column: String,
    /// Fixed covariate columns (`cov1` in `crr`). May be empty if
    /// `tv_covariates` is given.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Covariates interacted with a function of time (`cov2` in `crr`). Often
    /// these also appear in `covariates`, giving a proportional effect plus a
    /// time interaction.
    #[serde(default)]
    pub tv_covariates: Vec<String>,
    /// One time function per entry of `tv_covariates`.
    #[serde(default)]
    pub time_functions: Vec<TimeFn>,
    /// Column defining strata with distinct censoring distributions. The
    /// censoring survivor function is estimated separately within each.
    #[serde(default)]
    pub cengroup_column: Option<String>,
    /// Code of `status_column` denoting the failure type of interest.
    #[serde(default = "default_failcode")]
    pub failcode: f64,
    /// Code of `status_column` denoting a censored observation.
    #[serde(default = "default_cencode")]
    pub cencode: f64,
    /// Gradient tolerance for convergence.
    #[serde(default = "default_gtol")]
    pub gtol: f64,
    /// Maximum Newton iterations; `0` evaluates scores at `init` only.
    #[serde(default = "default_maxiter")]
    pub maxiter: usize,
    /// Starting values for the coefficients (default: all zeros).
    #[serde(default)]
    pub init: Option<Vec<f64>>,
    /// Whether to compute the robust variance (required for standard errors).
    #[serde(default = "default_true")]
    pub variance: bool,
    /// Confidence level for the subhazard-ratio interval.
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
}

impl FineGrayNodeSpec {
    fn validate(&self) -> Result<(), String> {
        if self.covariates.is_empty() && self.tv_covariates.is_empty() {
            return Err("at least one of covariates / tv_covariates must be non-empty".into());
        }
        if self.time_functions.len() != self.tv_covariates.len() {
            return Err(format!(
                "time_functions has {} entries but tv_covariates has {}",
                self.time_functions.len(),
                self.tv_covariates.len()
            ));
        }
        if !(0.0..1.0).contains(&(1.0 - self.conf_level)) {
            return Err("conf_level must be in (0, 1]".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct FineGrayNode {
    meta: NodePorts,
    spec: FineGrayNodeSpec,
}

pub struct FineGrayNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None) // 0: coefficient table
        .add_output_port(None) // 1: baseline cumulative incidence
        .add_input_port(None)
}

// ── R codegen helpers ───────────────────────────────────────────────────────

/// `as.matrix(df[, c("a", "b"), drop = FALSE])` — the shape `crr` wants for
/// `cov1` / `cov2`, preserving column names so R's term labels match ours.
fn r_cov_matrix(input: &str, cols: &[String]) -> String {
    let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
    format!(
        "as.matrix({input}[, c({}), drop = FALSE])",
        quoted.join(", ")
    )
}

/// Build the `tf` closure R needs: `function(uft) cbind(uft, uft^2)`.
fn r_tf_closure(fns: &[TimeFn]) -> String {
    let cols: Vec<String> = fns.iter().map(|f| f.r_expr("uft")).collect();
    format!("function(uft) cbind({})", cols.join(", "))
}

impl NodeFactory for FineGrayNodeFactory {
    fn kind(&self) -> &'static str {
        FINE_GRAY_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Fine-Gray proportional subdistribution hazards regression (competing risks)."
    }
    fn doc(&self) -> &'static str {
        "Fits the Fine & Gray (1999) proportional subdistribution hazards model, \
        the standard regression method for competing-risks data. Unlike a \
        cause-specific Cox model, its coefficients translate directly into \
        effects on the cumulative incidence of the event of interest, so \
        exp(coef) is a subdistribution hazard ratio (SHR). Risk-set \
        contributions from competing events are reweighted by an estimate of \
        the censoring survivor function, which may be stratified via \
        `cengroup_column` when censoring differs across groups. Optional \
        time-interacted covariates (`tv_covariates` × `time_functions`) relax \
        the proportionality assumption. Standard errors come from the robust \
        sandwich estimator. Output port 0 is the coefficient table; port 1 is \
        the baseline cumulative incidence curve."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FineGrayNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: FineGrayNodeSpec = serde_json::from_value(spec)?;
        if let Err(reason) = s.validate() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: FINE_GRAY_NODE_KIND.to_string(),
                reason,
                schema_pretty: serde_json::to_string_pretty(&schema_for!(FineGrayNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(FineGrayNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<FineGrayNodeSpec>(spec, FINE_GRAY_NODE_KIND)?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let base_var = ctx.fresh_var("fg_baseline");
        let fit = ctx.fresh_var("fg_fit");
        let se = ctx.fresh_var("fg_se");

        let mut code = vec![
            "# Fine-Gray proportional subdistribution hazards regression".to_string(),
            format!("{fit} <- cmprsk::crr("),
            format!("  ftime = {},", r_col(&input, &s.time_column)),
            format!("  fstatus = {},", r_col(&input, &s.status_column)),
        ];
        if !s.covariates.is_empty() {
            code.push(format!("  cov1 = {},", r_cov_matrix(&input, &s.covariates)));
        }
        if !s.tv_covariates.is_empty() {
            code.push(format!(
                "  cov2 = {},",
                r_cov_matrix(&input, &s.tv_covariates)
            ));
            code.push(format!("  tf = {},", r_tf_closure(&s.time_functions)));
        }
        if let Some(cg) = &s.cengroup_column {
            code.push(format!("  cengroup = {},", r_col(&input, cg)));
        }
        code.extend([
            format!("  failcode = {},", s.failcode),
            format!("  cencode = {},", s.cencode),
            format!("  gtol = {},", s.gtol),
            format!("  maxiter = {},", s.maxiter),
        ]);
        if let Some(init) = &s.init {
            let vals: Vec<String> = init.iter().map(|v| v.to_string()).collect();
            code.push(format!("  init = c({}),", vals.join(", ")));
        }
        code.push(format!(
            "  variance = {}",
            if s.variance { "TRUE" } else { "FALSE" }
        ));
        code.push(")".to_string());

        // ── port 0: coefficient table ───────────────────────────────────────
        let a = (1.0 - s.conf_level) / 2.0;
        code.extend([
            format!("{se} <- sqrt(diag(as.matrix({fit}$var)))"),
            format!("{out} <- data.frame("),
            format!("  term = names({fit}$coef),"),
            format!("  coefficient = as.numeric({fit}$coef),"),
            format!("  subhazard_ratio = as.numeric(exp({fit}$coef)),"),
            format!("  std_error = as.numeric({se}),"),
            format!("  z_stat = as.numeric({fit}$coef / {se}),"),
            format!("  p_value = as.numeric(2 * (1 - pnorm(abs({fit}$coef / {se})))),"),
            format!("  shr_ci_lower = as.numeric(exp({fit}$coef + qnorm({a}) * {se})),"),
            format!(
                "  shr_ci_upper = as.numeric(exp({fit}$coef + qnorm({}) * {se})),",
                1.0 - a
            ),
            "  stringsAsFactors = FALSE".to_string(),
            ")".to_string(),
            format!("{out}$log_likelihood <- {fit}$loglik"),
            format!("{out}$loglik_null <- {fit}$loglik.null"),
            format!("{out}$lr_stat <- -2 * ({fit}$loglik.null - {fit}$loglik)"),
            format!("{out}$lr_df <- length({fit}$coef)"),
            format!("{out}$lr_p_value <- 1 - pchisq({out}$lr_stat[1], length({fit}$coef))"),
            format!("{out}$n_obs <- {fit}$n"),
            format!("{out}$n_missing <- {fit}$n.missing"),
            format!(
                "{out}$n_events <- sum({} == {})",
                r_col(&input, &s.status_column),
                s.failcode
            ),
            format!("{out}$converged <- {fit}$converged"),
            format!("print({out})"),
        ]);

        // ── port 1: baseline cumulative incidence ───────────────────────────
        code.extend([
            format!("{base_var} <- data.frame("),
            format!("  uftime = as.numeric({fit}$uftime),"),
            format!("  bfitj = as.numeric({fit}$bfitj),"),
            format!("  baseline_cif = as.numeric(1 - exp(-cumsum({fit}$bfitj)))"),
            ")".to_string(),
        ]);

        Ok(dag_core::codegen::NodeCodegen {
            code,
            output_vars: vec![out, base_var],
            extra_packages: vec![],
        })
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["cmprsk".into()]
    }
}

// ── execution ───────────────────────────────────────────────────────────────

/// Column layout of output port 0.
fn coef_schema() -> Schema {
    Schema::new(vec![
        Field::new("term", DataType::Utf8, false),
        Field::new("coefficient", DataType::Float64, false),
        Field::new("subhazard_ratio", DataType::Float64, false),
        Field::new("std_error", DataType::Float64, false),
        Field::new("z_stat", DataType::Float64, false),
        Field::new("p_value", DataType::Float64, false),
        Field::new("shr_ci_lower", DataType::Float64, false),
        Field::new("shr_ci_upper", DataType::Float64, false),
        Field::new("log_likelihood", DataType::Float64, false),
        Field::new("loglik_null", DataType::Float64, false),
        Field::new("lr_stat", DataType::Float64, false),
        Field::new("lr_df", DataType::Int32, false),
        Field::new("lr_p_value", DataType::Float64, false),
        Field::new("n_obs", DataType::Int32, false),
        Field::new("n_missing", DataType::Int32, false),
        Field::new("n_events", DataType::Int32, false),
        Field::new("converged", DataType::Boolean, false),
    ])
}

/// Column layout of output port 1.
fn baseline_schema() -> Schema {
    Schema::new(vec![
        Field::new("uftime", DataType::Float64, false),
        Field::new("bfitj", DataType::Float64, false),
        Field::new("baseline_cif", DataType::Float64, false),
    ])
}

/// Build both output batches from a fitted model.
pub(crate) fn build_batches(
    fit: &CrrFit,
    conf_level: f64,
) -> Result<(RecordBatch, RecordBatch), FineGrayError> {
    let smry = cmprsk::summary_crr(fit, conf_level);
    let np = fit.coef.len();

    let terms: Vec<String> = smry.coefficients.iter().map(|c| c.term.clone()).collect();
    let take =
        |f: fn(&cmprsk::CoefRow) -> f64| -> Vec<f64> { smry.coefficients.iter().map(f).collect() };

    let coef_batch = RecordBatch::try_new(
        Arc::new(coef_schema()),
        vec![
            Arc::new(StringArray::from(terms)),
            Arc::new(Float64Array::from(take(|c| c.coef))),
            Arc::new(Float64Array::from(take(|c| c.exp_coef))),
            Arc::new(Float64Array::from(take(|c| c.se))),
            Arc::new(Float64Array::from(take(|c| c.z))),
            Arc::new(Float64Array::from(take(|c| c.p_value))),
            Arc::new(Float64Array::from(take(|c| c.ci_lower))),
            Arc::new(Float64Array::from(take(|c| c.ci_upper))),
            Arc::new(Float64Array::from(vec![fit.loglik; np])),
            Arc::new(Float64Array::from(vec![fit.loglik_null; np])),
            Arc::new(Float64Array::from(vec![smry.logtest; np])),
            Arc::new(Int32Array::from(vec![smry.df as i32; np])),
            Arc::new(Float64Array::from(vec![smry.logtest_p; np])),
            Arc::new(Int32Array::from(vec![fit.n as i32; np])),
            Arc::new(Int32Array::from(vec![fit.n_missing as i32; np])),
            Arc::new(Int32Array::from(vec![fit.n_events as i32; np])),
            Arc::new(BooleanArray::from(vec![fit.converged; np])),
        ],
    )
    .map_err(|e| FineGrayError::Fit(format!("coefficient batch: {e}")))?;

    let base_batch = RecordBatch::try_new(
        Arc::new(baseline_schema()),
        vec![
            Arc::new(Float64Array::from(fit.uftime.clone())),
            Arc::new(Float64Array::from(fit.bfitj.clone())),
            Arc::new(Float64Array::from(cmprsk::baseline_cif(fit))),
        ],
    )
    .map_err(|e| FineGrayError::Fit(format!("baseline batch: {e}")))?;

    Ok((coef_batch, base_batch))
}

#[async_trait]
impl DagNode for FineGrayNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        FINE_GRAY_NODE_KIND
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
            .ok_or(FineGrayError::Column("no input connected".to_string()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| FineGrayError::Collect(e.to_string()))?;

        let s = &self.spec;
        let ftime = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.time_column)?;
        let fstatus = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.status_column)?;

        // Covariates as row-major matrices, as `cmprsk::crr` expects.
        let read_block = |cols: &[String]| -> Result<Vec<Vec<f64>>, ColumnError> {
            if cols.is_empty() {
                return Ok(Vec::new());
            }
            let mut columns = Vec::with_capacity(cols.len());
            for name in cols {
                columns.push(dag_core::arrow_util::extract_numeric_lenient(
                    &batches, name,
                )?);
            }
            let n = columns[0].len();
            Ok((0..n)
                .map(|i| columns.iter().map(|c| c[i]).collect())
                .collect())
        };
        let cov1 = read_block(&s.covariates)?;
        let cov2 = read_block(&s.tv_covariates)?;

        let cengroup = match &s.cengroup_column {
            None => None,
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
        };

        let tf = if s.time_functions.is_empty() {
            TimeFunctions::None
        } else {
            TimeFunctions::Fns(&s.time_functions)
        };

        // `crr` performs its own complete-case filter (na.omit semantics), so
        // NaNs are passed through rather than pre-filtered here — that keeps
        // `n_missing` consistent with R.
        let fit = cmprsk::crr(
            &CrrInput {
                ftime: &ftime,
                fstatus: &fstatus,
                cov1: &cov1,
                cov1_names: &s.covariates,
                cov2: &cov2,
                cov2_names: &s.tv_covariates,
                tf,
                cengroup: cengroup.as_deref(),
            },
            &CrrOptions {
                failcode: s.failcode,
                cencode: s.cencode,
                gtol: s.gtol,
                maxiter: s.maxiter,
                init: s.init.clone(),
                variance: s.variance,
            },
        )
        .map_err(|e| FineGrayError::Fit(e.to_string()))?;

        let (coef_batch, base_batch) = build_batches(&fit, s.conf_level)?;

        let ctx = node_ctx.session();
        let mut res = PortOutputs::new();
        res.insert(
            0,
            ctx.read_batch(coef_batch)
                .map_err(|e| FineGrayError::ReadBatch(e.to_string()))?,
        );
        res.insert(
            1,
            ctx.read_batch(base_batch)
                .map_err(|e| FineGrayError::ReadBatch(e.to_string()))?,
        );
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: serde_json::Value) -> FineGrayNodeSpec {
        serde_json::from_value(json).expect("spec parses")
    }

    #[test]
    fn defaults_match_r() {
        let s = spec(serde_json::json!({
            "time_column": "t",
            "status_column": "s",
            "covariates": ["x1"]
        }));
        assert_eq!(s.failcode, 1.0);
        assert_eq!(s.cencode, 0.0);
        assert_eq!(s.gtol, 1e-6);
        assert_eq!(s.maxiter, 10);
        assert!(s.variance);
        assert_eq!(s.conf_level, 0.95);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn rejects_missing_covariates() {
        let s = spec(serde_json::json!({"time_column": "t", "status_column": "s"}));
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_tf_length_mismatch() {
        let s = spec(serde_json::json!({
            "time_column": "t",
            "status_column": "s",
            "tv_covariates": ["a", "b"],
            "time_functions": ["identity"]
        }));
        assert!(s.validate().is_err());
    }

    #[test]
    fn tf_closure_matches_the_documented_example() {
        // crr.Rd: crr(..., cbind(cov[,1], cov[,1]), function(Uft) cbind(Uft, Uft^2))
        assert_eq!(
            r_tf_closure(&[TimeFn::Identity, TimeFn::Square]),
            "function(uft) cbind(uft, uft^2)"
        );
        assert_eq!(
            r_tf_closure(&[TimeFn::Log]),
            "function(uft) cbind(log(uft))"
        );
    }

    #[test]
    fn cov_matrix_preserves_column_names() {
        assert_eq!(
            r_cov_matrix("df", &["x1".into(), "x2".into()]),
            "as.matrix(df[, c(\"x1\", \"x2\"), drop = FALSE])"
        );
    }
}
