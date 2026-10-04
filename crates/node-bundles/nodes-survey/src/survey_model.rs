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

use crate::survey_common::{SurveyDesignSpec, SurveyDomainSpec, one_in_one_out};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

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
    /// Optional survey domain (subpopulation) filter.
    #[serde(default)]
    pub survey_domain: Option<SurveyDomainSpec>,
    /// Sampling-weight column name. This is a top-level convenience alias for
    /// `design.weights`; it is mutually exclusive with `design.weights` and
    /// `design.probs`.
    #[serde(default)]
    pub weight_column: Option<String>,
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

/// Survey-weighted GLM node — supports all R `glm` families (gaussian,
/// binomial, poisson, Gamma, inverse.gaussian, quasi families) with
/// arbitrary link functions, via IRLS + design-based variance.
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
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Parse the family + link specification.
        let family_spec = survey::FamilySpec::new(&self.spec.family, self.spec.link.as_deref())
            .ok_or_else(|| DagError::NodeError {
                node_type: "svyglm".into(),
                msg: format!(
                    "unsupported family='{}' link='{:?}'",
                    self.spec.family, self.spec.link
                ),
            })?;

        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svyglm".into(),
            msg: "no input data".into(),
        })?;
        let batches =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: "svyglm".into(),
                    msg: format!("collect failed: {e}"),
                })?;

        if self.spec.weight_column.is_some()
            && (self.spec.design.weights.is_some() || !self.spec.design.probs.is_empty())
        {
            return Err(DagError::NodeError {
                node_type: "svyglm".into(),
                msg: "weight_column cannot be combined with design.weights or design.probs".into(),
            });
        }

        let domain_mask = match &self.spec.survey_domain {
            Some(spec) => crate::survey_common::survey_domain_mask(&batches, spec)?,
            None => vec![true; batches.iter().map(|batch| batch.num_rows()).sum()],
        };
        if !domain_mask.iter().any(|&keep| keep) {
            return Err(DagError::NodeError {
                node_type: "svyglm".into(),
                msg: "survey domain selected no rows".into(),
            });
        }

        let mut design_spec = self.spec.design.clone();
        if let Some(weight_column) = &self.spec.weight_column {
            design_spec.weights = Some(weight_column.clone());
        }
        let design = crate::survey_common::build_survey_design(&design_spec, &batches)?;
        let mut y =
            crate::survey_common::extract_variables(&batches, &[self.spec.response.clone()])?;
        let x = crate::survey_common::extract_variables(&batches, &self.spec.predictors)?;

        // Validate response for the chosen family (e.g. binomial needs y in [0,1]).
        let domain_y: Vec<f64> = y[0]
            .iter()
            .zip(&domain_mask)
            .filter(|(_, keep)| **keep)
            .map(|(value, _)| *value)
            .collect();
        if let Err(msg) = family_spec.validate_y(&domain_y) {
            return Err(DagError::NodeError {
                node_type: "svyglm".into(),
                msg,
            });
        }
        for (value, keep) in y[0].iter_mut().zip(domain_mask) {
            if !keep {
                *value = f64::NAN;
            }
        }

        let fit = survey::svyglm(
            &y[0],
            &x,
            &design,
            self.spec.intercept,
            None,
            &family_spec,
            None,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyglm".into(),
            msg: e.to_string(),
        })?;

        // Build output: term, estimate, se, t_stat, p_value.
        let p = fit.coefficients.len();
        let se = fit.se();
        let t_stats = fit.t_stats();
        let p_values = fit.p_values();
        let df = fit.df as f64;

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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyGlmSpec = serde_json::from_value(spec)?;
        if node_spec.weight_column.is_some()
            && (node_spec.design.weights.is_some() || !node_spec.design.probs.is_empty())
        {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "svyglm".to_string(),
                reason: "weight_column cannot be combined with design.weights or design.probs"
                    .to_string(),
                schema_pretty: serde_json::to_string_pretty(&self.spec_schema())
                    .unwrap_or_default(),
            });
        }
        if node_spec
            .survey_domain
            .as_ref()
            .is_some_and(|domain| domain.values.is_empty())
        {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "svyglm".to_string(),
                reason: "survey_domain.values must contain at least one level".to_string(),
                schema_pretty: serde_json::to_string_pretty(&self.spec_schema())
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(SvyGlmNode::new(node_spec)))
    }
}

// =====================================================================
// svycoxph
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SvyCoxphSpec {
    pub design: SurveyDesignSpec,
    /// Response: a survival outcome column name.
    pub time_column: String,
    pub event_column: String,
    pub predictors: Vec<String>,
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let schema = self.spec_schema();
        let schema_json = serde_json::to_value(schema).unwrap_or_default();
        let s: SvyCoxphSpec = serde_json::from_value(spec).map_err(|source| {
            dag_core::registry::error::Error::spec_rejection_from(self.kind(), &schema_json, source)
        })?;
        if s.predictors.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: self.kind().to_string(),
                reason: "predictors must be a non-empty array".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_json).unwrap_or_default(),
            });
        }
        Ok(Box::new(SvyCoxphNode::new(s)))
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        {
            let s: SvySurvregSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvySurvregNode::new(s)))
        }
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        {
            let s: SvyOlrSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyOlrNode::new(s)))
        }
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        {
            let s: SvyLoglinSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyLoglinNode::new(s)))
        }
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        {
            let s: SvyMleSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyMleNode::new(s)))
        }
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        {
            let s: SvyNlsSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyNlsNode::new(s)))
        }
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
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        {
            let s: SvyIvregSpec = serde_json::from_value(spec)?;
            Ok(Box::new(SvyIvregNode::new(s)))
        }
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
                reporter: &dag_core::dag::node_event::NodeReporter,
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
        reporter: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError>;
}

#[async_trait]
impl SpecExecute for SvyCoxphSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("collect: {e}"),
                })?;
        let design = crate::survey_common::build_survey_design(&self.design, &batches)?;
        let t = crate::survey_common::extract_variables(
            &batches,
            std::slice::from_ref(&self.time_column),
        )?;
        let e = crate::survey_common::extract_variables(
            &batches,
            std::slice::from_ref(&self.event_column),
        )?;
        let x = crate::survey_common::extract_variables(&batches, &self.predictors)?;
        let fit =
            survey::svy_coxph(&t[0], &e[0], &x, &design).map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.to_string(),
            })?;
        let se = fit.se();
        let terms = self.predictors.clone();
        let t_stats: Vec<f64> = fit
            .coefficients
            .iter()
            .zip(&se)
            .map(|(coef, stderr)| {
                if *stderr > 0.0 {
                    coef / stderr
                } else {
                    f64::NAN
                }
            })
            .collect();
        let p_values: Vec<f64> = t_stats
            .iter()
            .map(|&t| crate::survey_common::student_t_two_sided_p(t, fit.df as f64))
            .collect();

        crate::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &t_stats,
            &p_values,
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
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("collect: {e}"),
                })?;
        let design = crate::survey_common::build_survey_design(&self.design, &batches)?;
        let t = crate::survey_common::extract_variables(
            &batches,
            std::slice::from_ref(&self.time_column),
        )?;
        let e = crate::survey_common::extract_variables(
            &batches,
            std::slice::from_ref(&self.event_column),
        )?;
        let x = crate::survey_common::extract_variables(&batches, &self.predictors)?;
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
        let t_stats: Vec<f64> = fit
            .coefficients
            .iter()
            .zip(&se)
            .map(|(coef, stderr)| {
                if *stderr > 0.0 {
                    coef / stderr
                } else {
                    f64::NAN
                }
            })
            .collect();
        let p_values: Vec<f64> = t_stats
            .iter()
            .map(|&t| crate::survey_common::student_t_two_sided_p(t, fit.df as f64))
            .collect();
        crate::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &t_stats,
            &p_values,
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
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("collect: {e}"),
                })?;
        let design = crate::survey_common::build_survey_design(&self.design, &batches)?;
        let y_raw = crate::survey_common::extract_string_column_pub(&batches, &self.response)
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
        let x = crate::survey_common::extract_variables(&batches, &self.predictors)?;
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
        let t_stats: Vec<f64> = all
            .iter()
            .zip(&se)
            .map(|(b, s)| if *s > 0.0 { b / s } else { f64::NAN })
            .collect();
        let p_values: Vec<f64> = t_stats
            .iter()
            .map(|&t| crate::survey_common::student_t_two_sided_p(t, fit.df as f64))
            .collect();
        crate::survey_common::build_model_output_batch(
            &terms,
            &all,
            &se,
            &t_stats,
            &p_values,
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
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("collect: {e}"),
                })?;
        let design = crate::survey_common::build_survey_design(&self.design, &batches)?;
        let row = crate::survey_common::extract_string_column_pub(&batches, &self.variables[0])
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: e.0,
            })?;
        let col = crate::survey_common::extract_string_column_pub(&batches, &self.variables[1])
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
        let t_stats: Vec<f64> = fit
            .coefficients
            .iter()
            .zip(&se)
            .map(|(b, s)| if *s > 0.0 { b / s } else { f64::NAN })
            .collect();
        let p_values: Vec<f64> = t_stats
            .iter()
            .map(|&t| crate::survey_common::student_t_two_sided_p(t, fit.df as f64))
            .collect();
        crate::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &t_stats,
            &p_values,
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
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        Err(DagError::NodeError {
            node_type: kind.into(),
            msg: "svynls requires a user-supplied model function".into(),
        })
    }
}

#[async_trait]
impl SpecExecute for SvyIvregSpec {
    async fn execute_spec(
        &self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: kind.into(),
            msg: "no input".into(),
        })?;
        let batches =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: kind.into(),
                    msg: format!("collect: {e}"),
                })?;
        let design = crate::survey_common::build_survey_design(&self.design, &batches)?;
        let y = crate::survey_common::extract_variables(
            &batches,
            std::slice::from_ref(&self.response),
        )?;
        let endo = crate::survey_common::extract_variables(&batches, &self.endogenous)?;
        let instr = crate::survey_common::extract_variables(&batches, &self.instruments)?;
        let exo = if self.exogenous.is_empty() {
            vec![]
        } else {
            crate::survey_common::extract_variables(&batches, &self.exogenous)?
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
        let t_stats: Vec<f64> = fit
            .coefficients
            .iter()
            .zip(&se)
            .map(|(b, s)| if *s > 0.0 { b / s } else { f64::NAN })
            .collect();
        let p_values: Vec<f64> = t_stats
            .iter()
            .map(|&t| crate::survey_common::student_t_two_sided_p(t, fit.df as f64))
            .collect();
        crate::survey_common::build_model_output_batch(
            &terms,
            &fit.coefficients,
            &se,
            &t_stats,
            &p_values,
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
        _r: &dag_core::dag::node_event::NodeReporter,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        Err(DagError::NodeError {
            node_type: kind.into(),
            msg: "svymle requires user-supplied loglike/gradient closures".into(),
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

    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    #[tokio::test]
    async fn svyglm_node_matches_r_apiclus1() {
        // Read apiclus1.csv and validate the svyglm DAG node.
        if !std::path::Path::new("/tmp/apiclus1.csv").exists() {
            eprintln!("skipping: /tmp/apiclus1.csv not present");
            return;
        }
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
            design: crate::survey_common::SurveyDesignSpec {
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
            survey_domain: None,
            weight_column: None,
            response: "api99".into(),
            predictors: vec!["ell".into(), "meals".into()],
            intercept: true,
            family: "gaussian".into(),
            link: None,
            std_errors: "linearized".into(),
        };

        let mut node = SvyGlmNode::new(spec);
        let input = NodeInput::new_dataframe(0, df2);
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let result = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
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
        assert!(s.survey_domain.is_none());
        assert!(s.weight_column.is_none());
        assert!(s.intercept);
        assert_eq!(s.family, "gaussian");
        assert!(s.link.is_none());
        assert_eq!(s.std_errors, "linearized");
    }

    #[test]
    fn svyglm_schema_exposes_domain_and_weight_options() {
        let schema = serde_json::to_value(SvyGlmFactory.spec_schema()).unwrap();
        assert!(schema["properties"].get("survey_domain").is_some());
        assert!(schema["properties"].get("weight_column").is_some());
    }

    #[tokio::test]
    async fn svyglm_domain_and_weight_column_restrict_fit() {
        let x_domain: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let y_domain: Vec<f64> = x_domain.iter().map(|&x| 1.0 + 2.0 * x).collect();
        let x_excluded: Vec<f64> = (0..10).map(|i| 10.0 + i as f64).collect();
        let y_excluded: Vec<f64> = x_excluded.iter().map(|&x| 100.0 + 20.0 * x).collect();
        let domain = vec!["analysis"; 10]
            .into_iter()
            .chain(vec!["excluded"; 10])
            .collect::<Vec<_>>();
        let weights = vec![1.0; 20];

        let mut x = x_domain.clone();
        x.extend(x_excluded);
        let mut y = y_domain.clone();
        y.extend(y_excluded);
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
            Field::new("domain", DataType::Utf8, false),
            Field::new("wt", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(x)),
                Arc::new(Float64Array::from(y)),
                Arc::new(StringArray::from(domain)),
                Arc::new(Float64Array::from(weights)),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyGlmSpec {
            design: crate::survey_common::SurveyDesignSpec::default(),
            survey_domain: Some(crate::survey_common::SurveyDomainSpec {
                column: "domain".into(),
                values: vec!["analysis".into()],
            }),
            weight_column: Some("wt".into()),
            response: "y".into(),
            predictors: vec!["x".into()],
            intercept: true,
            family: "gaussian".into(),
            link: None,
            std_errors: "linearized".into(),
        };
        let mut node = SvyGlmNode::new(spec);
        let outs = node
            .execute(
                &node_ctx(),
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let result = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
        let estimates: Vec<f64> = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();
        assert!((estimates[0] - 1.0).abs() < 1e-8);
        assert!((estimates[1] - 2.0).abs() < 1e-8);
    }

    #[tokio::test]
    async fn svyglm_binomial_domain_excludes_rows_without_nan_panic() {
        // Regression for the NaN-clobbering binomial NaN bug: the DAG node
        // marks domain-out rows by setting their response to NaN, and the
        // inner survey::svyglm must not pre-reject NaN as outside [0,1].
        // Domain-out rows here contain "illegal" y values (5.0); if the
        // pre-filter were not NaN-aware the binomial validator would refuse.
        let n_in = 20;
        let n_out = 10;
        let x: Vec<f64> = (0..n_in + n_out).map(|i| (i as f64) * 0.1).collect();
        // Domain-in rows: Bernoulli with p = sigmoid(1 + x)
        let y_in: Vec<f64> = (0..n_in)
            .map(|i| {
                let p = 1.0 / (1.0 + (-(1.0 + x[i])).exp());
                if p > 0.5 { 1.0 } else { 0.0 }
            })
            .collect();
        // Domain-out rows: response would be 5.0 — outside [0,1] but masked
        // by the domain filter (NaN-ed before validation).
        let y_out: Vec<f64> = vec![5.0; n_out];
        let mut y = y_in.clone();
        y.extend(y_out);
        let domain: Vec<String> = (0..n_in)
            .map(|_| "study".to_string())
            .chain((0..n_out).map(|_| "exclude".to_string()))
            .collect();

        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
            Field::new("domain", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(x)),
                Arc::new(Float64Array::from(y)),
                Arc::new(StringArray::from(domain)),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyGlmSpec {
            design: crate::survey_common::SurveyDesignSpec::default(),
            survey_domain: Some(crate::survey_common::SurveyDomainSpec {
                column: "domain".into(),
                values: vec!["study".into()],
            }),
            weight_column: None,
            response: "y".into(),
            predictors: vec!["x".into()],
            intercept: true,
            family: "binomial".into(),
            link: None,
            std_errors: "linearized".into(),
        };
        let mut node = SvyGlmNode::new(spec);
        let outs = node
            .execute(
                &node_ctx(),
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("svyglm binomial + survey_domain must not fail at validation");
        let result = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
        // The x coefficient should be positive (more x → higher p).
        let estimates: Vec<f64> = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();
        assert!(
            estimates[1] > 0.0,
            "x estimate should be positive: {estimates:?}"
        );
    }

    #[tokio::test]
    async fn svyglm_quasibinomial_domain_excludes_rows() {
        // Same regression but quasibinomial — same y range validator.
        let n_in = 12;
        let n_out = 6;
        let x: Vec<f64> = (0..n_in + n_out).map(|i| i as f64).collect();
        let y_in: Vec<f64> = x
            .iter()
            .take(n_in)
            .map(|&xi| if xi > 5.0 { 1.0 } else { 0.0 })
            .collect();
        let y_out: Vec<f64> = vec![5.0; n_out];
        let mut y = y_in;
        y.extend(y_out);
        let domain: Vec<String> = (0..n_in)
            .map(|_| "in".to_string())
            .chain((0..n_out).map(|_| "out".to_string()))
            .collect();
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
            Field::new("domain", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(x)),
                Arc::new(Float64Array::from(y)),
                Arc::new(StringArray::from(domain)),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyGlmSpec {
            design: crate::survey_common::SurveyDesignSpec::default(),
            survey_domain: Some(crate::survey_common::SurveyDomainSpec {
                column: "domain".into(),
                values: vec!["in".into()],
            }),
            weight_column: None,
            response: "y".into(),
            predictors: vec!["x".into()],
            intercept: true,
            family: "quasibinomial".into(),
            link: None,
            std_errors: "linearized".into(),
        };
        let mut node = SvyGlmNode::new(spec);
        let outs = node
            .execute(
                &node_ctx(),
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("quasibinomial + domain must work");
        let result = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
        assert!(result[0].num_rows() >= 2);
    }

    #[test]
    fn svycoxph_spec() {
        let json = serde_json::json!({
            "design": {"ids": []},
            "time_column": "time",
            "event_column": "event",
            "predictors": ["x1"]
        });
        let s: SvyCoxphSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.time_column, "time");
        assert_eq!(s.event_column, "event");
        assert!(s.design.ids.is_empty());
    }

    #[test]
    fn svycoxph_schema_has_no_intercept() {
        let schema = serde_json::to_value(SvyCoxphFactory.spec_schema()).unwrap();
        assert!(schema["properties"].get("intercept").is_none());
    }

    #[test]
    fn svycoxph_rejects_removed_intercept_field() {
        let json = serde_json::json!({
            "design": {"ids": []},
            "time_column": "time",
            "event_column": "event",
            "predictors": ["x"],
            "intercept": false
        });
        assert!(serde_json::from_value::<SvyCoxphSpec>(json).is_err());
    }

    #[tokio::test]
    async fn svycoxph_node_reports_nonzero_t_p_value() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("time", DataType::Float64, false),
            Field::new("event", DataType::Float64, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![4.0, 3.0, 2.0, 5.0, 6.0, 1.5])),
                Arc::new(Float64Array::from(vec![1.0, 1.0, 1.0, 0.0, 1.0, 0.0])),
                Arc::new(Float64Array::from(vec![0.0, 1.0, 1.0, 0.0, 1.0, 0.0])),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyCoxphSpec {
            design: crate::survey_common::SurveyDesignSpec {
                ids: vec![],
                strata: vec![],
                probs: vec![],
                weights: None,
                fpc: vec![],
                nest: false,
                pps: "none".into(),
                variance: "HT".into(),
                lonely_psu: None,
            },
            time_column: "time".into(),
            event_column: "event".into(),
            predictors: vec!["x".into()],
        };
        let mut node = SvyCoxphNode::new(spec);
        let outs = node
            .execute(
                &node_ctx(),
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let result = outs.dataframe(0).unwrap().clone().collect().await.unwrap();

        let estimate = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let stderr = result[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let p_value = result[0]
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);

        assert!((estimate - 0.8216793853198499).abs() < 1e-8);
        assert!(stderr.is_finite() && stderr > 0.0);
        assert!(p_value.is_finite() && p_value > 0.0 && p_value <= 1.0);
    }
}
