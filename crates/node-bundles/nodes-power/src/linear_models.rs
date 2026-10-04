//! `power_anova` and `power_regression` nodes.

use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::common::*;

pub const POWER_ANOVA_NODE_KIND: &str = "power_anova";
pub const POWER_REGRESSION_NODE_KIND: &str = "power_regression";

fn default_alpha() -> f64 {
    0.05
}
fn default_power() -> f64 {
    0.8
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerAnovaSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default)]
    pub n: Option<u64>,
    #[serde(default)]
    pub group_ns: Option<Vec<u64>>,
    #[serde(default)]
    pub allocation_ratios: Option<Vec<f64>>,
    #[serde(default)]
    pub means: Option<Vec<f64>>,
    /// Group count when a standardized effect is supplied without means.
    #[serde(default)]
    pub groups: Option<usize>,
    #[serde(default)]
    pub sd: Option<f64>,
    #[serde(default)]
    pub effect_size: Option<f64>,
}

pub struct PowerAnovaNodeFactory;

impl NodeFactory for PowerAnovaNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_ANOVA_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for one-way ANOVA."
    }
    fn doc(&self) -> &'static str {
        "Uses the noncentral F distribution with a common within-group normal \
        variance. Effects can be supplied as group means and SD or Cohen's f."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerAnovaSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerAnovaSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerAnovaNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerAnovaNode {
    meta: NodePorts,
    spec: PowerAnovaSpec,
}

#[async_trait]
impl DagNode for PowerAnovaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_ANOVA_NODE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        match compute_anova(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute_anova(spec: &PowerAnovaSpec) -> Result<PowerResult, PowerError> {
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    let means = spec.means.clone();
    let k = means
        .as_ref()
        .map_or_else(|| spec.groups.unwrap_or_default(), Vec::len);
    if means.is_none() && spec.effect_size.is_none() {
        return Err(PowerError::Spec(
            "provide group means with sd, or effect_size and group count".into(),
        ));
    }
    if let Some(m) = means.as_ref() {
        if m.len() < 2 || m.iter().any(|x| !x.is_finite()) {
            return Err(PowerError::Input(
                "ANOVA requires at least two finite group means".into(),
            ));
        }
    }
    if let Some(sd) = spec.sd {
        if means.is_none() || !(sd > 0.0) {
            return Err(PowerError::Input(
                "sd must be positive and supplied with group means".into(),
            ));
        }
    } else if means.is_some() {
        return Err(PowerError::Spec("sd is required with means".into()));
    }
    if spec.solve_for == SolveFor::Mde && means.is_none() {
        return Err(PowerError::Spec(
            "MDE requires group means and sd to define the effect direction".into(),
        ));
    }
    if spec.solve_for != SolveFor::Mde && spec.effect_size.is_none() && means.is_none() {
        return Err(PowerError::Spec(
            "provide group means with sd, or groups and effect_size".into(),
        ));
    }
    if let Some(f) = spec.effect_size {
        if !(f > 0.0) || !f.is_finite() {
            return Err(PowerError::Input("effect_size must be positive".into()));
        }
        if spec.solve_for != SolveFor::Mde {
            ensure_solved_input(spec.solve_for, effective_target, Some(f), "effect_size")?;
        }
    }
    let groups = means.clone().unwrap_or_else(|| vec![0.0; k]);
    if k < 2 {
        return Err(PowerError::Input(
            "ANOVA requires at least two groups".into(),
        ));
    }
    let sd = spec.sd.unwrap_or(1.0);

    let ratios = spec
        .allocation_ratios
        .clone()
        .unwrap_or_else(|| vec![1.0; k]);
    if ratios.len() != k || ratios.iter().any(|r| !(*r > 0.0) || !r.is_finite()) {
        return Err(PowerError::Input(
            "allocation_ratios must have one positive value per group".into(),
        ));
    }
    let ratio_sum: f64 = ratios.iter().sum();
    let weights: Vec<f64> = ratios.iter().map(|r| r / ratio_sum).collect();
    let weighted_mean: f64 = groups.iter().zip(&weights).map(|(m, w)| m * w).sum();
    let direct_f = spec.effect_size;
    let base_f = direct_f.unwrap_or_else(|| {
        (groups
            .iter()
            .zip(&weights)
            .map(|(m, w)| w * (m - weighted_mean).powi(2))
            .sum::<f64>()
            / sd.powi(2))
        .sqrt()
    });

    let group_ns = match spec.solve_for {
        SolveFor::Power => spec
            .group_ns
            .clone()
            .ok_or_else(|| PowerError::Spec("group_ns is required for power".into()))?,
        SolveFor::SampleSize => {
            if spec.group_ns.is_some() || spec.n.is_some() {
                return Err(PowerError::Spec(
                    "group_ns or n must not be supplied when solve_for='sample_size'".into(),
                ));
            }
            let base = integer_root_up(2, 1_000_000, effective_target.unwrap(), |base| {
                let group_ns: Vec<u64> = ratios
                    .iter()
                    .map(|r| (base as f64 * r).ceil().max(1.0) as u64)
                    .collect();
                let total: u64 = group_ns.iter().sum();
                anova_f_power(
                    (k - 1) as f64,
                    (total - k as u64) as f64,
                    total as f64 * base_f.powi(2),
                    spec.alpha,
                )
            })?;
            ratios
                .iter()
                .map(|r| (base as f64 * r).ceil() as u64)
                .collect()
        }
        SolveFor::Mde => spec
            .group_ns
            .clone()
            .ok_or_else(|| PowerError::Spec("group_ns is required for mde".into()))?,
    };
    if group_ns.len() != k || group_ns.contains(&0) {
        return Err(PowerError::Input(
            "group_ns must contain one positive count per group".into(),
        ));
    }
    let total_n: u64 = group_ns.iter().sum();
    let df1 = (k - 1) as f64;
    let df2 = (total_n - k as u64) as f64;
    let critical = f_critical(df1, df2, spec.alpha)?;

    let effect_f = match spec.solve_for {
        SolveFor::Mde => find_root_increasing(1e-10, 1e4, effective_target.unwrap(), |scale| {
            anova_f_power(
                df1,
                df2,
                total_n as f64 * (scale * base_f).powi(2),
                spec.alpha,
            )
        })?,
        _ => base_f,
    };
    let power = 1.0 - noncentral_f_cdf(critical, df1, df2, total_n as f64 * effect_f.powi(2))?;

    let effect_description = if means.is_some() {
        let mut means_text = String::new();
        for (i, m) in groups.iter().enumerate() {
            if i > 0 {
                means_text.push_str(", ");
            }
            means_text.push_str(&format!("mu{}={m}", i + 1));
        }
        format!("normal errors with common SD={sd}; direction means [{means_text}]")
    } else {
        "a standardized Cohen's f effect without specific group means".to_string()
    };
    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: effective_target,
        alpha: spec.alpha,
        alternative: Alternative::TwoSided,
        effect_size: effect_f,
        effect_size_scale: "Cohen's f".into(),
        method: "noncentral F distribution".into(),
        method_type: "analytic exact under model assumptions",
        n1: None,
        n2: None,
        total_n: Some(total_n),
        clusters1: None,
        clusters2: None,
        total_clusters: None,
        events: None,
        design_effect: None,
        result_value: match spec.solve_for {
            SolveFor::Power => Some(power),
            SolveFor::SampleSize => Some(total_n as f64),
            SolveFor::Mde => Some(effect_f),
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "total observations".into(),
            SolveFor::Mde => "Cohen's f".into(),
        },
        input_assumptions: format!(
            "one-way ANOVA; {effect_description}; planning parameters are fixed design assumptions"
        ),
        warnings: if df2 < 10.0 {
            "small error degrees of freedom make the normality assumption influential".into()
        } else {
            String::new()
        },
    })
}

fn anova_f_power(df1: f64, df2: f64, lambda: f64, alpha: f64) -> Result<f64, PowerError> {
    if df2 <= 0.0 {
        return Ok(0.0);
    }
    Ok(1.0 - noncentral_f_cdf(f_critical(df1, df2, alpha)?, df1, df2, lambda)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegressionTest {
    Overall,
    Nested,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerRegressionSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default)]
    pub test: Option<RegressionTest>,
    #[serde(default)]
    pub n: Option<u64>,
    pub predictors_total: usize,
    #[serde(default)]
    pub predictors_tested: Option<usize>,
    #[serde(default)]
    pub r_squared: Option<f64>,
    #[serde(default)]
    pub partial_r_squared: Option<f64>,
}

pub struct PowerRegressionNodeFactory;

impl NodeFactory for PowerRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_REGRESSION_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for linear-regression F tests."
    }
    fn doc(&self) -> &'static str {
        "Supports the overall model test based on R-squared and a nested test \
        based on partial R-squared, using the noncentral F distribution."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerRegressionSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerRegressionSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerRegressionNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerRegressionNode {
    meta: NodePorts,
    spec: PowerRegressionSpec,
}

#[async_trait]
impl DagNode for PowerRegressionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_REGRESSION_NODE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        match compute_regression(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute_regression(spec: &PowerRegressionSpec) -> Result<PowerResult, PowerError> {
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    let test = spec.test.unwrap_or(RegressionTest::Overall);
    let (df1, tested, supplied_effect) = match test {
        RegressionTest::Overall => (spec.predictors_total, spec.predictors_total, spec.r_squared),
        RegressionTest::Nested => {
            let tested = spec
                .predictors_tested
                .ok_or_else(|| PowerError::Spec("nested tests require predictors_tested".into()))?;
            (tested, tested, spec.partial_r_squared)
        }
    };
    if spec.predictors_total == 0 || df1 == 0 || tested > spec.predictors_total {
        return Err(PowerError::Input(
            "predictor counts leave no residual degrees of freedom".into(),
        ));
    }
    ensure_solved_input(
        spec.solve_for,
        effective_target,
        supplied_effect,
        match test {
            RegressionTest::Overall => "r_squared",
            RegressionTest::Nested => "partial_r_squared",
        },
    )?;
    if let Some(r2) = supplied_effect {
        if !(0.0..1.0).contains(&r2) {
            return Err(PowerError::Input(
                "the regression effect must be in the open interval (0, 1)".into(),
            ));
        }
    }

    let n = match spec.solve_for {
        SolveFor::Power => spec
            .n
            .ok_or_else(|| PowerError::Spec("n is required for power".into()))?,
        SolveFor::SampleSize => integer_root_up(
            (spec.predictors_total + 2) as u64,
            10_000_000,
            effective_target.unwrap(),
            |n| {
                regression_power(
                    supplied_effect.unwrap(),
                    n as f64,
                    df1 as f64,
                    spec.predictors_total,
                    spec.alpha,
                )
            },
        )?,
        SolveFor::Mde => spec
            .n
            .ok_or_else(|| PowerError::Spec("n is required for mde".into()))?,
    };
    let df2 = (n as usize - spec.predictors_total - 1) as f64;
    if df2 <= 0.0 {
        return Err(PowerError::Input(
            "n is too small for the specified predictor count".into(),
        ));
    }
    let solved_effect = match spec.solve_for {
        SolveFor::Mde => find_root_increasing(1e-10, 0.999999, effective_target.unwrap(), |r2| {
            regression_power(r2, n as f64, df1 as f64, spec.predictors_total, spec.alpha)
        })?,
        _ => supplied_effect.unwrap(),
    };
    let ncp = n as f64 * solved_effect / (1.0 - solved_effect);
    let critical = f_critical(df1 as f64, df2, spec.alpha)?;
    let power = 1.0 - noncentral_f_cdf(critical, df1 as f64, df2, ncp)?;
    let (method, scale) = match test {
        RegressionTest::Overall => (
            "overall linear-model F test via noncentral F".to_string(),
            "population R-squared".to_string(),
        ),
        RegressionTest::Nested => (
            "nested linear-model F test via noncentral F".to_string(),
            "population partial R-squared".to_string(),
        ),
    };

    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: effective_target,
        alpha: spec.alpha,
        alternative: Alternative::TwoSided,
        effect_size: solved_effect,
        effect_size_scale: scale.clone(),
        method,
        method_type: "analytic approximation",
        n1: Some(n),
        n2: None,
        total_n: Some(n),
        clusters1: None,
        clusters2: None,
        total_clusters: None,
        events: None,
        design_effect: None,
        result_value: match spec.solve_for {
            SolveFor::Power => Some(power),
            SolveFor::SampleSize => Some(n as f64),
            SolveFor::Mde => Some(solved_effect),
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "observations".into(),
            SolveFor::Mde => scale,
        },
        input_assumptions: format!(
            "linear model with normal homoscedastic errors; total predictors={}; tested predictors={}; \
             the effect is a population quantity used for design",
            spec.predictors_total, tested
        ),
        warnings: if df2 < 10.0 {
            "few residual degrees of freedom make model assumptions influential".into()
        } else {
            String::new()
        },
    })
}

fn regression_power(
    effect: f64,
    n: f64,
    df1: f64,
    predictors_total: usize,
    alpha: f64,
) -> Result<f64, PowerError> {
    let df2 = n - predictors_total as f64 - 1.0;
    if df2 <= 0.0 || !(0.0..1.0).contains(&effect) {
        return Ok(0.0);
    }
    let ncp = n * effect / (1.0 - effect);
    let critical = f_critical(df1, df2, alpha)?;
    Ok(1.0 - noncentral_f_cdf(critical, df1, df2, ncp)?)
}
