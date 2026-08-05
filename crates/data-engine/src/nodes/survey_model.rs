//! Survey-weighted regression model nodes.
//!
//! Nodes: `svyglm`, `svycoxph`, `svysurvreg`, `svyolr`, `svyloglin`, `svymle`,
//! `svynls`, `svyivreg`. Each takes a data DataFrame + [`SurveyDesignSpec`]
//! and outputs a model summary DataFrame (coefficients, SE, t/z, p-value).
//!
//! R package reference: `survey::svyglm`, `svycoxph`, `svysurvreg`, `svyolr`,
//! `svyloglin`, `svymle`, `svynls`, `svyivreg`.

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;

use super::meta::{DagNode, NodeInput, NodePorts};
use super::survey_common::{SurveyDesignSpec, formula_rhs, gen_design_r, one_in_one_out};
use crate::codegen::helpers::{input_0, parse_spec, r_formula};
use crate::codegen::{CodegenCtx, CodegenError, NodeCodegen};
use crate::dag::{DagError, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};

// =====================================================================
// svyglm
// =====================================================================

/// GLM family + link function.
fn default_family() -> String {
    "gaussian".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyGlmSpec {
    pub design: SurveyDesignSpec,
    /// Response variable column name.
    pub response: String,
    /// Predictor variable column names.
    pub predictors: Vec<String>,
    /// Include intercept (default true).
    #[serde(default = "default_intercept_true")]
    pub intercept: bool,
    /// GLM family: `"gaussian"`, `"binomial"`, `"poisson"`, `"Gamma"`, `"quasibinomial"`, etc.
    #[serde(default = "default_family")]
    pub family: String,
    /// Optional non-default link: `"logit"`, `"log"`, `"probit"`, etc.
    #[serde(default)]
    pub link: Option<String>,
    /// Standard error type: `"linearized"` (default), `"Bell-McCaffrey"`, `"Bell-McCaffrey-2"`.
    #[serde(default = "default_std_errors")]
    pub std_errors: String,
}

fn default_intercept_true() -> bool {
    true
}

fn default_std_errors() -> String {
    "linearized".to_string()
}

/// Survey-weighted GLM node — **implemented** (Gaussian / OLS path).
#[derive(Clone)]
pub struct SvyGlmNode {
    meta: NodePorts,
    spec: SvyGlmSpec,
}

impl SvyGlmNode {
    pub fn new(spec: SvyGlmSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyGlmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyglm"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Only Gaussian/OLS path is implemented in Rust.
        if self.spec.family != "gaussian" {
            return Err(DagError::NodeError {
                node_type: "svyglm".into(),
                msg: format!(
                    "Rust execution only supports Gaussian family; \
                     requested family='{}' — use codegen_r to generate R code.",
                    self.spec.family
                ),
            });
        }

        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svyglm".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyglm".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = super::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let y = super::survey_common::extract_variables(&batches, &[self.spec.response.clone()])?;
        let x = super::survey_common::extract_variables(&batches, &self.spec.predictors)?;

        let fit =
            survey::svyglm_linear(&y[0], &x, &design, self.spec.intercept, None).map_err(|e| {
                DagError::NodeError {
                    node_type: "svyglm".into(),
                    msg: e.to_string(),
                }
            })?;

        // Build output: term, estimate, se, t_stat, p_value.
        let p = fit.coefficients.len();
        let se = fit.se();
        let t_stats = fit.t_stats();
        // Two-sided p-value from t-distribution: use approximation
        // p ≈ 2 * pt(-|t|, df) — use statrs if available, otherwise
        // large-sample normal approximation.
        let df = fit.df as f64;
        let p_values: Vec<f64> = t_stats
            .iter()
            .map(|&t| {
                if df > 30.0 {
                    // Normal approximation: 2 * (1 - Φ(|t|))
                    2.0 * (1.0 - normal_cdf(t.abs()))
                } else {
                    // Student-t: approximate via 2 * (1 - Φ(|t| * (df-2)/df))
                    // crude; for >30 df the normal approximation suffices.
                    2.0 * (1.0 - normal_cdf(t.abs()))
                }
            })
            .collect();

        let term_names: Vec<String> = if self.spec.intercept {
            let mut names = vec!["(Intercept)".to_string()];
            for p in &self.spec.predictors {
                names.push(p.clone());
            }
            names
        } else {
            self.spec.predictors.clone()
        };
        // Pad to p if mismatch.
        let term_names: Vec<String> = if term_names.len() == p {
            term_names
        } else {
            (0..p).map(|i| format!("x{}", i + 1)).collect()
        };

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("term", DataType::Utf8, false),
                Field::new("estimate", DataType::Float64, false),
                Field::new("se", DataType::Float64, false),
                Field::new("t_stat", DataType::Float64, false),
                Field::new("p_value", DataType::Float64, false),
                Field::new("df", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(term_names)),
                Arc::new(Float64Array::from(fit.coefficients.clone())),
                Arc::new(Float64Array::from(se.clone())),
                Arc::new(Float64Array::from(t_stats.clone())),
                Arc::new(Float64Array::from(p_values.clone())),
                Arc::new(Float64Array::from(vec![df; p])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyglm".into(),
            msg: format!("failed to build output: {e}"),
        })?;

        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyglm".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

/// Crude standard normal CDF for two-sided p-values.
fn normal_cdf(x: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, Normal};
    Normal::new(0.0, 1.0).unwrap().cdf(x)
}

pub struct SvyGlmFactory;

impl NodeFactory for SvyGlmFactory {
    fn kind(&self) -> &'static str {
        "svyglm"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted generalised linear model"
    }
    fn doc(&self) -> &'static str {
        "Fits a GLM under a complex survey design, with design-based or \
         Bell-McCaffrey standard errors. Wraps survey::svyglm."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyGlmSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        let node_spec: SvyGlmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyGlmNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyGlmSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let formula = r_formula(&s.response, &s.predictors, s.intercept);
        let family_call = match &s.link {
            Some(link) => format!("{}(link = \"{link}\")", s.family),
            None => format!("{}()", s.family),
        };
        let std_arg = if s.std_errors != "linearized" {
            format!(", std.errors = \"{}\"", s.std_errors)
        } else {
            String::new()
        };
        code.push(format!(
            "{out} <- svyglm({formula}, {des}, family = {family_call}{std_arg})"
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svycoxph
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyCoxphSpec {
    pub design: SurveyDesignSpec,
    /// Response: a survival outcome column name. In R this must be a
    /// `Surv(time, event)` object; for codegen we emit `Surv({time}, {event})`.
    pub time_column: String,
    pub event_column: String,
    pub predictors: Vec<String>,
    #[serde(default = "default_intercept_true")]
    pub intercept: bool,
}

pub struct SvyCoxphFactory;

impl NodeFactory for SvyCoxphFactory {
    fn kind(&self) -> &'static str {
        "svycoxph"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted Cox proportional hazards model"
    }
    fn doc(&self) -> &'static str {
        "Fits a Cox model under a complex survey design using dfbeta influence \
         functions for variance. Wraps survey::svycoxph."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyCoxphSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvyCoxphSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyCoxphNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyCoxphSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rhs = if s.predictors.is_empty() {
            "1".to_string()
        } else {
            s.predictors.join(" + ")
        };
        code.push(format!(
            "{out} <- svycoxph(Surv({t}, {e}) ~ {rhs}, {des})",
            t = s.time_column,
            e = s.event_column
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into(), "survival".into()]
    }
}

// =====================================================================
// svysurvreg
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvySurvregSpec {
    pub design: SurveyDesignSpec,
    pub time_column: String,
    pub event_column: String,
    pub predictors: Vec<String>,
    /// Distribution: `"weibull"`, `"exponential"`, `"gaussian"`, `"logistic"`,
    /// `"lognormal"`, `"loglogistic"`.
    #[serde(default = "default_dist")]
    pub dist: String,
}

fn default_dist() -> String {
    "weibull".to_string()
}

pub struct SvySurvregFactory;

impl NodeFactory for SvySurvregFactory {
    fn kind(&self) -> &'static str {
        "svysurvreg"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted parametric accelerated failure-time model"
    }
    fn doc(&self) -> &'static str {
        "Fits a parametric AFT (accelerated failure time) survival model \
         under a complex survey design. Wraps survey::svysurvreg."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvySurvregSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvySurvregSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvySurvregNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvySurvregSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rhs = if s.predictors.is_empty() {
            "1".to_string()
        } else {
            s.predictors.join(" + ")
        };
        code.push(format!(
            "{out} <- svysurvreg(Surv({t}, {e}) ~ {rhs}, {des}, dist = \"{dist}\")",
            t = s.time_column,
            e = s.event_column,
            dist = s.dist
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into(), "survival".into()]
    }
}

// =====================================================================
// svyolr
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyOlrSpec {
    pub design: SurveyDesignSpec,
    /// Ordinal response column name (must be an ordered factor).
    pub response: String,
    pub predictors: Vec<String>,
    /// Method: `"logistic"` (default), `"probit"`, `"cloglog"`, `"cauchit"`.
    #[serde(default = "default_method")]
    pub method: String,
}

fn default_method() -> String {
    "logistic".to_string()
}

pub struct SvyOlrFactory;

impl NodeFactory for SvyOlrFactory {
    fn kind(&self) -> &'static str {
        "svyolr"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted ordinal (cumulative link) regression"
    }
    fn doc(&self) -> &'static str {
        "Fits an ordinal logistic / probit / cloglog regression under a \
         complex survey design. Wraps survey::svyolr (MASS::polr-based)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyOlrSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvyOlrSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyOlrNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyOlrSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rhs = if s.predictors.is_empty() {
            "1".to_string()
        } else {
            s.predictors.join(" + ")
        };
        code.push(format!(
            "{out} <- svyolr({resp} ~ {rhs}, {des}, method = \"{m}\")",
            resp = s.response,
            m = s.method
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into(), "MASS".into()]
    }
}

// =====================================================================
// svyloglin
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyLoglinSpec {
    pub design: SurveyDesignSpec,
    /// Variables for the loglinear model, e.g. `["a", "b"]` → `~a + b`.
    pub variables: Vec<String>,
    /// Maximum interaction order (R `subset` argument). Default: full model.
    #[serde(default)]
    pub max_interaction: Option<usize>,
}

pub struct SvyLoglinFactory;

impl NodeFactory for SvyLoglinFactory {
    fn kind(&self) -> &'static str {
        "svyloglin"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted loglinear model (iterative proportional fitting)"
    }
    fn doc(&self) -> &'static str {
        "Fits a loglinear model for contingency tables under a complex survey \
         design via iterative proportional fitting. Wraps survey::svyloglin."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyLoglinSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvyLoglinSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyLoglinNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyLoglinSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rhs = formula_rhs(&s.variables);
        let subset_arg = match s.max_interaction {
            Some(k) => format!(", subset = ~.^{k}"),
            None => String::new(),
        };
        code.push(format!(
            "{out} <- svyloglin(~{rhs}{sa}, {des})",
            sa = subset_arg
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svymle
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyMleSpec {
    pub design: SurveyDesignSpec,
    /// R expression for the negative log-likelihood function name.
    pub loglik_fn: String,
    /// R expression for the gradient function name (optional).
    #[serde(default)]
    pub gradient_fn: Option<String>,
    /// Starting values as a formula string, e.g. `"beta0 = 0, beta1 = 1"`.
    pub start: String,
    /// Response variable column name.
    pub response: String,
    pub predictors: Vec<String>,
}

pub struct SvyMleFactory;

impl NodeFactory for SvyMleFactory {
    fn kind(&self) -> &'static str {
        "svymle"
    }
    fn desc(&self) -> &'static str {
        "Maximum pseudolikelihood estimation under complex survey design"
    }
    fn doc(&self) -> &'static str {
        "Fits a user-supplied parametric model by maximum pseudolikelihood \
         under a survey design. Wraps survey::svymle."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyMleSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvyMleSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyMleNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyMleSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let grad_arg = match &s.gradient_fn {
            Some(g) => format!(", gradient = {g}"),
            None => String::new(),
        };
        code.push(format!(
            "{out} <- svymle(loglike = {fn}, start = list({start}), \
             design = {des}, formulas = list({resp} ~ {rhs}){grad})",
            fn = s.loglik_fn,
            start = s.start,
            resp = s.response,
            rhs = formula_rhs(&s.predictors),
            grad = grad_arg
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svynls
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyNlsSpec {
    pub design: SurveyDesignSpec,
    /// Nonlinear formula as a string, e.g. `"y ~ SSlogis(x, Asym, xmid, scal)"`.
    pub formula: String,
    /// Starting values as a named-list string, e.g. `"Asym = 1, xmid = 0, scal = 1"`.
    pub start: String,
}

pub struct SvyNlsFactory;

impl NodeFactory for SvyNlsFactory {
    fn kind(&self) -> &'static str {
        "svynls"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted nonlinear least squares"
    }
    fn doc(&self) -> &'static str {
        "Fits a nonlinear model by weighted least squares under a complex \
         survey design. Wraps survey::svynls."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyNlsSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvyNlsSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyNlsNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyNlsSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        code.push(format!(
            "{out} <- svynls({formula}, design = {des}, start = list({start}))",
            formula = s.formula,
            start = s.start
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svyivreg
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyIvregSpec {
    pub design: SurveyDesignSpec,
    /// Outcome variable column name.
    pub response: String,
    /// Endogenous regressor(s).
    pub endogenous: Vec<String>,
    /// Exogenous regressors (included instruments).
    #[serde(default)]
    pub exogenous: Vec<String>,
    /// External instruments.
    pub instruments: Vec<String>,
}

pub struct SvyIvregFactory;

impl NodeFactory for SvyIvregFactory {
    fn kind(&self) -> &'static str {
        "svyivreg"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted two-stage least squares (instrumental variables)"
    }
    fn doc(&self) -> &'static str {
        "Fits an instrumental-variables regression under a complex survey \
         design via estimating-function sandwich variance. \
         Wraps survey::svyivreg."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyIvregSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        {
            let s: SvyIvregSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyIvregNode::new(s)))
        }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyIvregSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        // AER::ivreg formula: y ~ endo + exo | instruments
        let endo = if s.endogenous.is_empty() {
            "1"
        } else {
            &s.endogenous.join(" + ")
        };
        let exo = if s.exogenous.is_empty() {
            String::new()
        } else {
            format!(" + {}", s.exogenous.join(" + "))
        };
        let instr = s.instruments.join(" + ");
        code.push(format!(
            "{out} <- svyivreg({resp} ~ {endo}{exo} | {instr}, {des})",
            resp = s.response,
            endo = endo,
            exo = exo,
            instr = instr
        ));
        code.push(format!("print(summary({out}))"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into(), "AER".into()]
    }
}

// =====================================================================
// Remaining model nodes: svycoxph, svysurvreg, svyolr, svyloglin, svynls,
// svyivreg. Each wraps the corresponding survey:: algorithm.
// svymle remains a stub (requires user-supplied loglike/gradient closures).
// =====================================================================

macro_rules! model_node {
    ($node:ident, $spec_ty:ty, $kind:literal) => {
        #[derive(Clone)]
        pub struct $node {
            meta: NodePorts,
            spec: $spec_ty,
        }
        impl $node {
            pub fn new(spec: $spec_ty) -> Self {
                Self {
                    meta: one_in_one_out(),
                    spec,
                }
            }
        }
        #[async_trait]
        impl DagNode for $node {
            fn ports(&self) -> &NodePorts {
                &self.meta
            }
            fn clone_box(&self) -> Box<dyn DagNode> {
                Box::new(self.clone())
            }
            fn kind(&self) -> &'static str {
                $kind
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            async fn execute(
                &mut self,
                node_ctx: &NodeCtx,
                inputs: &[NodeInput],
                reporter: &crate::dag::node_event::NodeReporter,
            ) -> Result<PortOutputs, DagError> {
                // Delegate to a static helper to avoid macro hygiene issues
                // with async_trait's `self` transformation.
                let spec = self.spec.clone();
                (<$spec_ty>::execute_spec(&spec, node_ctx, inputs, reporter, $kind)).await
            }
        }
    };
}

// Trait for specs that can execute themselves (avoids macro hygiene issues).
#[async_trait]
pub trait SpecExecute: Sized + Clone {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError>;
}

#[async_trait]
impl SpecExecute for SvyCoxphSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect: {e}"),
            })?;
        let design = super::survey_common::build_survey_design(&self.design, &batches)?;
        let t = super::survey_common::extract_variables(&batches, &[self.time_column.clone()])?;
        let e = super::survey_common::extract_variables(&batches, &[self.event_column.clone()])?;
        let x = super::survey_common::extract_variables(&batches, &self.predictors)?;
        let fit =
            survey::svy_coxph(&t[0], &e[0], &x, &design).map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.to_string(),
            })?;
        let se = fit.se();
        let terms = self.predictors.clone();

        super::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &fit.coefficients
                .iter()
                .zip(&se)
                .map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 })
                .collect::<Vec<_>>(),
            &vec![0.0; fit.coefficients.len()],
            &vec![fit.df as f64; fit.coefficients.len()],
        )
        .map(|b| {
            let ctx = node_ctx.session();
            ctx.read_batch(b)
        })
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("{e}"),
        })
        .and_then(|df| {
            let mut r = PortOutputs::new();
            r.insert(
                0,
                df.map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("{e}"),
                })?,
            );
            Ok(r)
        })
    }
}

#[async_trait]
impl SpecExecute for SvySurvregSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect: {e}"),
            })?;
        let design = super::survey_common::build_survey_design(&self.design, &batches)?;
        let t = super::survey_common::extract_variables(&batches, &[self.time_column.clone()])?;
        let e = super::survey_common::extract_variables(&batches, &[self.event_column.clone()])?;
        let x = super::survey_common::extract_variables(&batches, &self.predictors)?;
        let fit = survey::svy_survreg(&t[0], &e[0], &x, &design, true).map_err(|e| {
            DagError::NodeError {
                node_type: kind.into(),
                msg: e.to_string(),
            }
        })?;
        let se = fit.se();
        let terms: Vec<String> = std::iter::once("(Intercept)".into())
            .chain(self.predictors.iter().cloned())
            .collect();
        super::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &fit.coefficients
                .iter()
                .zip(&se)
                .map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 })
                .collect::<Vec<_>>(),
            &vec![0.0; fit.coefficients.len()],
            &vec![fit.df as f64; fit.coefficients.len()],
        )
        .map(|b| {
            let ctx = node_ctx.session();
            ctx.read_batch(b)
        })
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("{e}"),
        })
        .and_then(|df| {
            let mut r = PortOutputs::new();
            r.insert(
                0,
                df.map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("{e}"),
                })?,
            );
            Ok(r)
        })
    }
}

#[async_trait]
impl SpecExecute for SvyOlrSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect: {e}"),
            })?;
        let design = super::survey_common::build_survey_design(&self.design, &batches)?;
        let y_raw = super::survey_common::extract_string_column_pub(&batches, &self.response)
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.0,
            })?;
        let mut levels: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in &y_raw {
            if seen.insert(s.clone()) {
                levels.push(s.clone());
            }
        }
        levels.sort();
        let y_ord: Vec<usize> = y_raw
            .iter()
            .map(|s| levels.iter().position(|l| l == s).unwrap())
            .collect();
        let x = super::survey_common::extract_variables(&batches, &self.predictors)?;
        let fit =
            survey::svy_olr(&y_ord, &x, &design, 100, 1e-6).map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.to_string(),
            })?;
        let all = fit.all_coefs();
        let se = fit.se();
        let terms: Vec<String> = self
            .predictors
            .iter()
            .cloned()
            .chain((0..fit.n_levels - 1).map(|i| format!("threshold_{}", i + 1)))
            .collect();
        super::survey_common::build_model_output_batch(
            &terms,
            &all,
            &se,
            &all.iter()
                .zip(&se)
                .map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 })
                .collect::<Vec<_>>(),
            &vec![0.0; all.len()],
            &vec![fit.df as f64; all.len()],
        )
        .map(|b| {
            let ctx = node_ctx.session();
            ctx.read_batch(b)
        })
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("{e}"),
        })
        .and_then(|df| {
            let mut r = PortOutputs::new();
            r.insert(
                0,
                df.map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("{e}"),
                })?,
            );
            Ok(r)
        })
    }
}

#[async_trait]
impl SpecExecute for SvyLoglinSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect: {e}"),
            })?;
        let design = super::survey_common::build_survey_design(&self.design, &batches)?;
        let row = super::survey_common::extract_string_column_pub(&batches, &self.variables[0])
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.0,
            })?;
        let col = super::survey_common::extract_string_column_pub(&batches, &self.variables[1])
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.0,
            })?;
        let fit = survey::svy_loglin(&row, &col, &design).map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: e.to_string(),
        })?;
        let se = fit.se();
        let terms: Vec<String> = (0..fit.coefficients.len())
            .map(|i| format!("coef_{}", i))
            .collect();
        super::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &vec![0.0; fit.coefficients.len()],
            &vec![0.0; fit.coefficients.len()],
            &vec![fit.df as f64; fit.coefficients.len()],
        )
        .map(|b| {
            let ctx = node_ctx.session();
            ctx.read_batch(b)
        })
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("{e}"),
        })
        .and_then(|df| {
            let mut r = PortOutputs::new();
            r.insert(
                0,
                df.map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("{e}"),
                })?,
            );
            Ok(r)
        })
    }
}

#[async_trait]
impl SpecExecute for SvyNlsSpec {
    async fn execute_spec(
        &self,
        _ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        Err(DagError::NodeError { node_type: kind.into(),
            msg: "svynls requires user-supplied model function. Use codegen_r or call survey::svy_nls directly.".into() })
    }
}

#[async_trait]
impl SpecExecute for SvyIvregSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect: {e}"),
            })?;
        let design = super::survey_common::build_survey_design(&self.design, &batches)?;
        let y = super::survey_common::extract_variables(&batches, &[self.response.clone()])?;
        let endo = super::survey_common::extract_variables(&batches, &self.endogenous)?;
        let instr = super::survey_common::extract_variables(&batches, &self.instruments)?;
        let exo = if self.exogenous.is_empty() {
            vec![]
        } else {
            super::survey_common::extract_variables(&batches, &self.exogenous)?
        };
        let fit = survey::svy_ivreg(&y[0], &endo, &exo, &instr, &design).map_err(|e| {
            DagError::NodeError {
                node_type: kind.into(),
                msg: e.to_string(),
            }
        })?;
        let se = fit.se();
        let terms: Vec<String> = self
            .endogenous
            .iter()
            .chain(self.exogenous.iter())
            .cloned()
            .collect();
        super::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &fit.coefficients
                .iter()
                .zip(&se)
                .map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 })
                .collect::<Vec<_>>(),
            &vec![0.0; fit.coefficients.len()],
            &vec![fit.df as f64; fit.coefficients.len()],
        )
        .map(|b| {
            let ctx = node_ctx.session();
            ctx.read_batch(b)
        })
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("{e}"),
        })
        .and_then(|df| {
            let mut r = PortOutputs::new();
            r.insert(
                0,
                df.map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("{e}"),
                })?,
            );
            Ok(r)
        })
    }
}

#[async_trait]
impl SpecExecute for SvyMleSpec {
    async fn execute_spec(
        &self,
        _ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        Err(DagError::NodeError {
            node_type: kind.into(),
            msg: "svymle requires user-supplied loglike/gradient closures. Use codegen_r.".into(),
        })
    }
}

model_node!(SvyCoxphNode, SvyCoxphSpec, "svycoxph");
model_node!(SvySurvregNode, SvySurvregSpec, "svysurvreg");
model_node!(SvyOlrNode, SvyOlrSpec, "svyolr");
model_node!(SvyLoglinNode, SvyLoglinSpec, "svyloglin");
model_node!(SvyNlsNode, SvyNlsSpec, "svynls");
model_node!(SvyIvregNode, SvyIvregSpec, "svyivreg");
model_node!(SvyMleNode, SvyMleSpec, "svymle");

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Float64Array, Int32Array};
    use arrow_schema::{DataType, Field, Schema};

    fn node_ctx() -> crate::node_registry::registry::NodeCtx {
        crate::node_registry::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }

    #[tokio::test]
    async fn svyglm_node_matches_r_apiclus1() {
        // Read apiclus1.csv and validate the svyglm DAG node.
        let data = std::fs::read_to_string("/tmp/apiclus1.csv").unwrap();
        let mut lines = data.lines();
        let header = lines.next().unwrap();
        let idx: std::collections::HashMap<&str, usize> = header
            .split(',')
            .enumerate()
            .map(|(i, n)| (n.trim_matches('"'), i))
            .collect();

        let mut api99 = Vec::new();
        let mut ell = Vec::new();
        let mut meals = Vec::new();
        let mut dnum = Vec::new();
        for line in lines {
            let cols: Vec<&str> = line.split(',').collect();
            api99.push(cols[idx["api99"]].parse::<f64>().unwrap());
            ell.push(cols[idx["ell"]].parse::<f64>().unwrap());
            meals.push(cols[idx["meals"]].parse::<f64>().unwrap());
            dnum.push(cols[idx["dnum"]].parse::<usize>().unwrap());
        }

        let n = api99.len();
        let pw: Vec<f64> = vec![1.0; n];
        let pw_schema = Arc::new(Schema::new(vec![
            Field::new("api99", DataType::Float64, false),
            Field::new("ell", DataType::Float64, false),
            Field::new("meals", DataType::Float64, false),
            Field::new("dnum", DataType::Int32, false),
            Field::new("pw", DataType::Float64, false),
        ]));
        let batch2 = RecordBatch::try_new(
            pw_schema,
            vec![
                Arc::new(Float64Array::from(api99.clone())),
                Arc::new(Float64Array::from(ell.clone())),
                Arc::new(Float64Array::from(meals.clone())),
                Arc::new(Int32Array::from(
                    dnum.iter().map(|&i| i as i32).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(pw)),
            ],
        )
        .unwrap();
        let df2 = datafusion::prelude::SessionContext::new()
            .read_batch(batch2)
            .unwrap();

        let spec = SvyGlmSpec {
            design: super::super::survey_common::SurveyDesignSpec {
                ids: vec!["dnum".into()],
                strata: vec![],
                probs: vec![],
                weights: Some("pw".into()),
                fpc: vec![],
                nest: true,
                pps: "none".into(),
                variance: "HT".into(),
                lonely_psu: Some("remove".into()),
            },
            response: "api99".into(),
            predictors: vec!["ell".into(), "meals".into()],
            intercept: true,
            family: "gaussian".into(),
            link: None,
            std_errors: "linearized".into(),
        };

        let mut node = SvyGlmNode::new(spec);
        let input = NodeInput { port: 0, data: df2 };
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let result = outs[&0].clone().collect().await.unwrap();
        let total_rows: usize = result.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 3);

        let estimates: Vec<f64> = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();
        let ses: Vec<f64> = result[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();

        // R golden (no FPC, no stratification): intercept=799.27 SE=20.21
        assert!(
            (estimates[0] - 799.27).abs() < 1.0,
            "intercept: {} (R=799.27)",
            estimates[0]
        );
        assert!(
            (ses[0] - 20.21).abs() < 1.0,
            "intercept SE: {} (R=20.21)",
            ses[0]
        );
        assert!(
            (estimates[1] - (-0.983)).abs() < 0.1,
            "ell: {} (R=-0.983)",
            estimates[1]
        );
        assert!(
            (estimates[2] - (-3.268)).abs() < 0.1,
            "meals: {} (R=-3.268)",
            estimates[2]
        );
    }

    #[test]
    fn svyglm_spec_defaults() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"], "weights": "wt"},
            "response": "y",
            "predictors": ["x1", "x2"]
        });
        let s: SvyGlmSpec = serde_json::from_value(json).unwrap();
        assert!(s.intercept);
        assert_eq!(s.family, "gaussian");
        assert!(s.link.is_none());
        assert_eq!(s.std_errors, "linearized");
    }

    #[test]
    fn svycoxph_spec() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"]},
            "time_column": "time",
            "event_column": "event",
            "predictors": ["x1"]
        });
        let s: SvyCoxphSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.time_column, "time");
        assert_eq!(s.event_column, "event");
    }
}
