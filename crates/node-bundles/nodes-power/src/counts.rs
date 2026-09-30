//! `power_prop_test` and `power_chisq_test` nodes.

use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::common::*;

pub const POWER_PROP_TEST_NODE_KIND: &str = "power_prop_test";
pub const POWER_CHISQ_TEST_NODE_KIND: &str = "power_chisq_test";

fn default_alpha() -> f64 {
    0.05
}
fn default_power() -> f64 {
    0.8
}
fn default_alternative() -> String {
    "two_sided".into()
}
fn default_allocation() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerPropTestSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_alternative")]
    pub alternative: String,
    #[serde(default)]
    pub p0: Option<f64>,
    #[serde(default)]
    pub p1: Option<f64>,
    #[serde(default)]
    pub p2: Option<f64>,
    #[serde(default)]
    pub n: Option<u64>,
    #[serde(default)]
    pub n1: Option<u64>,
    #[serde(default)]
    pub n2: Option<u64>,
    #[serde(default = "default_allocation")]
    pub allocation_ratio: f64,
}

pub struct PowerPropTestNodeFactory;

impl NodeFactory for PowerPropTestNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_PROP_TEST_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for one- or two-proportion tests."
    }
    fn doc(&self) -> &'static str {
        "Uses the arcsine normal approximation for prospective power. The result \
        is approximate and is not an exact binomial calculation."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerPropTestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerPropTestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerPropTestNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerPropTestNode {
    meta: NodePorts,
    spec: PowerPropTestSpec,
}

#[async_trait]
impl DagNode for PowerPropTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_PROP_TEST_NODE_KIND
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
        match compute_prop(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute_prop(spec: &PowerPropTestSpec) -> Result<PowerResult, PowerError> {
    let alternative = Alternative::parse(&spec.alternative)?;
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    if !(spec.allocation_ratio > 0.0) || !spec.allocation_ratio.is_finite() {
        return Err(PowerError::Input(
            "allocation_ratio must be positive".into(),
        ));
    }

    let one_sample = spec.p0.is_some();
    for p in [&spec.p0, &spec.p1, &spec.p2].into_iter().flatten() {
        if !(0.0..=1.0).contains(p) {
            return Err(PowerError::Input("proportions must be in [0, 1]".into()));
        }
    }

    if one_sample {
        let p0 = spec.p0.unwrap();
        let supplied_p1 = spec.p1;
        ensure_solved_input(spec.solve_for, effective_target, supplied_p1, "p1")?;
        let (n, p1) = match spec.solve_for {
            SolveFor::Power => {
                let p1 = supplied_p1.ok_or_else(|| PowerError::Spec("p1 is required".into()))?;
                let n = spec
                    .n
                    .ok_or_else(|| PowerError::Spec("n is required for power".into()))?;
                (n, p1)
            }
            SolveFor::SampleSize => {
                let p1 = supplied_p1.ok_or_else(|| PowerError::Spec("p1 is required".into()))?;
                let h = arcsine_effect(p0, p1)?;
                let n = integer_root_up(2, 1_000_000, effective_target.unwrap(), |n| {
                    prop_power(h, n as f64, 0.0, alternative, spec.alpha)
                })?;
                (n, p1)
            }
            SolveFor::Mde => {
                let n = spec
                    .n
                    .ok_or_else(|| PowerError::Spec("n is required for mde".into()))?;
                let solved = find_root_increasing(
                    1e-8,
                    std::f64::consts::PI,
                    effective_target.unwrap(),
                    |h| prop_power(h, n as f64, 0.0, Alternative::Greater, spec.alpha),
                )?;
                let direction = if alternative == Alternative::Less {
                    -1.0
                } else {
                    1.0
                };
                let p1 = arcsine_to_proportion(p0, solved * direction)?;
                (n, p1)
            }
        };
        let h = arcsine_effect(p0, p1)?;
        let power = prop_power(h, n as f64, 0.0, alternative, spec.alpha)?;
        let expected_small = n as f64 * p0.min(p1) < 5.0;
        return prop_result(
            spec,
            effective_target,
            alternative,
            power,
            h,
            p1,
            Some(n),
            None,
            n,
            "one-sample z test on the arcsine-transformed proportion",
            expected_small,
            format!("H0: p={p0}; design p={p1}; binomial outcomes"),
        );
    }

    let supplied_p1 = spec.p1;
    ensure_solved_input(spec.solve_for, effective_target, supplied_p1, "p1")?;
    let supplied_p2 = spec.p2;
    ensure_solved_input(spec.solve_for, effective_target, supplied_p2, "p2")?;
    let (n1, n2, p1, p2) = match spec.solve_for {
        SolveFor::Power => {
            let p1 = supplied_p1.ok_or_else(|| PowerError::Spec("p1 is required".into()))?;
            let p2 = supplied_p2.ok_or_else(|| PowerError::Spec("p2 is required".into()))?;
            let n1 = spec
                .n1
                .or(spec.n)
                .ok_or_else(|| PowerError::Spec("n1 (or n) is required for power".into()))?;
            let n2 = spec
                .n2
                .ok_or_else(|| PowerError::Spec("n2 is required for power".into()))?;
            (n1, n2, p1, p2)
        }
        SolveFor::SampleSize => {
            let p1 = supplied_p1.ok_or_else(|| PowerError::Spec("p1 is required".into()))?;
            let p2 = supplied_p2.ok_or_else(|| PowerError::Spec("p2 is required".into()))?;
            let h = arcsine_effect(p1, p2)?;
            let ratio = spec.allocation_ratio;
            let n1 = integer_root_up(2, 1_000_000, effective_target.unwrap(), |n1| {
                prop_power(h, n1 as f64, ratio, alternative, spec.alpha)
            })?;
            let n2 = (n1 as f64 * ratio).ceil() as u64;
            (n1, n2, p1, p2)
        }
        SolveFor::Mde => {
            let n1 = spec
                .n1
                .or(spec.n)
                .ok_or_else(|| PowerError::Spec("n1 (or n) is required for mde".into()))?;
            let n2 = spec
                .n2
                .ok_or_else(|| PowerError::Spec("n2 is required for mde".into()))?;
            let p1 = supplied_p1.ok_or_else(|| PowerError::Spec("p1 is required".into()))?;
            let solved_h =
                find_root_increasing(1e-8, std::f64::consts::PI, effective_target.unwrap(), |h| {
                    prop_power(h, n1 as f64, n2 as f64 / n1 as f64, alternative, spec.alpha)
                })?;
            let direction = if alternative == Alternative::Less {
                -1.0
            } else {
                1.0
            };
            let p2 = arcsine_to_proportion(p1, solved_h * direction)?;
            (n1, n2, p1, p2)
        }
    };
    let h = arcsine_effect(p1, p2)?;
    let power = prop_power(h, n1 as f64, n2 as f64 / n1 as f64, alternative, spec.alpha)?;
    let expected_small = n1 as f64 * p1.min(1.0 - p1) < 5.0 || n2 as f64 * p2.min(1.0 - p2) < 5.0;
    prop_result(
        spec,
        effective_target,
        alternative,
        power,
        h,
        p2,
        Some(n1),
        Some(n2),
        n1 + n2,
        "two-sample z test on the arcsine-transformed proportions",
        expected_small,
        format!("design proportions p1={p1}, p2={p2}; independent binary outcomes"),
    )
}

#[allow(clippy::too_many_arguments)]
fn prop_result(
    spec: &PowerPropTestSpec,
    target: Option<f64>,
    alternative: Alternative,
    power: f64,
    h: f64,
    solved_p: f64,
    n1: Option<u64>,
    n2: Option<u64>,
    total_n: u64,
    method: &str,
    expected_small: bool,
    assumptions: String,
) -> Result<PowerResult, PowerError> {
    let mut warnings = Vec::new();
    if expected_small {
        warnings.push(
            "expected counts below 5; the normal approximation may be unreliable".to_string(),
        );
    }
    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: target,
        alpha: spec.alpha,
        alternative,
        effect_size: h,
        effect_size_scale: "Cohen's h (arcsine effect)".into(),
        method: method.into(),
        method_type: "normal approximation",
        n1,
        n2,
        total_n: Some(total_n),
        clusters1: None,
        clusters2: None,
        total_clusters: None,
        events: None,
        design_effect: None,
        result_value: match spec.solve_for {
            SolveFor::Power => Some(power),
            SolveFor::SampleSize => Some(total_n as f64),
            SolveFor::Mde => Some(solved_p),
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "total observations".into(),
            SolveFor::Mde => "solved alternative proportion".into(),
        },
        input_assumptions: assumptions,
        warnings: warnings.join("; "),
    })
}

fn arcsine_effect(p0: f64, p1: f64) -> Result<f64, PowerError> {
    if !(0.0..=1.0).contains(&p0) || !(0.0..=1.0).contains(&p1) {
        return Err(PowerError::Input("proportions must be in [0,1]".into()));
    }
    Ok(2.0 * (p1.sqrt().asin() - p0.sqrt().asin()))
}

fn arcsine_to_proportion(reference: f64, signed_h: f64) -> Result<f64, PowerError> {
    let transformed = 2.0 * reference.sqrt().asin() + signed_h;
    if !transformed.is_finite() || transformed.abs() > std::f64::consts::PI + 1e-12 {
        return Err(PowerError::Numerical(
            "arcsine effect is outside the attainable proportion range".into(),
        ));
    }
    Ok((transformed / 2.0).sin().powi(2).clamp(0.0, 1.0))
}

fn prop_power(
    h: f64,
    n1: f64,
    allocation_ratio: f64,
    alternative: Alternative,
    alpha: f64,
) -> Result<f64, PowerError> {
    let ncp = if allocation_ratio == 0.0 {
        h * n1.sqrt()
    } else {
        let n2 = (n1 * allocation_ratio).max(1.0);
        h / (1.0 / n1 + 1.0 / n2).sqrt()
    };
    let critical = match alternative {
        Alternative::TwoSided => normal_quantile(1.0 - alpha / 2.0)?,
        Alternative::Greater => normal_quantile(1.0 - alpha)?,
        Alternative::Less => normal_quantile(alpha)?,
    };
    let ncp = if alternative == Alternative::Less {
        -ncp.abs()
    } else {
        ncp.abs()
    };
    let upper = 1.0 - normal_cdf(critical - ncp)?;
    let power = match alternative {
        Alternative::Greater => upper,
        Alternative::Less => normal_cdf(critical - ncp)?,
        Alternative::TwoSided => upper + normal_cdf(-critical - ncp)?,
    };
    Ok(power.clamp(0.0, 1.0))
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerChisqTestSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default)]
    pub n: Option<u64>,
    /// Alternative cell probabilities by row. Each row must have equal length.
    #[serde(default)]
    pub probabilities: Option<Vec<Vec<f64>>>,
}

pub struct PowerChisqTestNodeFactory;

impl NodeFactory for PowerChisqTestNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_CHISQ_TEST_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power or sample size for a Pearson chi-square independence test."
    }
    fn doc(&self) -> &'static str {
        "Uses the noncentral chi-square distribution. The alternative is a \
        full row-by-row cell probability matrix, not only a total sample size."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerChisqTestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerChisqTestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerChisqTestNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerChisqTestNode {
    meta: NodePorts,
    spec: PowerChisqTestSpec,
}

#[async_trait]
impl DagNode for PowerChisqTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_CHISQ_TEST_NODE_KIND
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
        match compute_chisq(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute_chisq(spec: &PowerChisqTestSpec) -> Result<PowerResult, PowerError> {
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    let supplied_effect = spec.probabilities.clone();
    if supplied_effect.is_none() {
        return Err(PowerError::Spec("probabilities are required".into()));
    }
    let probabilities = match supplied_effect {
        Some(p) => p,
        None => return Err(PowerError::Spec("probabilities are required".into())),
    };
    if probabilities.len() < 2 || probabilities.iter().any(|r| r.len() < 2) {
        return Err(PowerError::Spec(
            "the table must have at least two rows and two columns".into(),
        ));
    }
    let columns = probabilities[0].len();
    if probabilities.iter().any(|r| r.len() != columns) {
        return Err(PowerError::Spec(
            "all probability rows must have equal length".into(),
        ));
    }
    let total: f64 = probabilities.iter().flatten().sum();
    if !(0.999_999..=1.000_001).contains(&total) || probabilities.iter().flatten().any(|&p| p < 0.0)
    {
        return Err(PowerError::Input(
            "cell probabilities must be nonnegative and sum to 1".into(),
        ));
    }
    let rows = probabilities.len();
    let row_margins: Vec<f64> = probabilities.iter().map(|r| r.iter().sum()).collect();
    let col_margins: Vec<f64> = (0..columns)
        .map(|c| probabilities.iter().map(|r| r[c]).sum::<f64>())
        .collect();
    let null_probs: Vec<f64> = row_margins
        .iter()
        .flat_map(|&rm| col_margins.iter().map(move |&cm| rm * cm))
        .collect();
    let alternative: Vec<f64> = probabilities.iter().flatten().copied().collect();
    let w_squared: f64 = alternative
        .iter()
        .zip(&null_probs)
        .filter(|&(_, e)| *e > 0.0)
        .map(|(&p, &e)| (p - e).powi(2) / e)
        .sum();
    let w = w_squared.sqrt();
    if w <= 0.0 {
        return Err(PowerError::Input(
            "the alternative table must violate independence".into(),
        ));
    }
    let df = ((rows - 1) * (columns - 1)) as f64;

    let power_for_n = |n: f64| -> Result<f64, PowerError> {
        let critical = chisq_critical(df, spec.alpha)?;
        Ok(1.0 - noncentral_chisq_cdf(critical, df, n * w_squared)?)
    };
    let n = match spec.solve_for {
        SolveFor::Power => spec
            .n
            .ok_or_else(|| PowerError::Spec("n is required for power".into()))?,
        SolveFor::SampleSize => integer_root_up(2, 10_000_000, effective_target.unwrap(), |n| {
            power_for_n(n as f64)
        })?,
        SolveFor::Mde => spec
            .n
            .ok_or_else(|| PowerError::Spec("n is required for mde".into()))?,
    };
    let solved_w = match spec.solve_for {
        SolveFor::Mde => {
            let target = effective_target.unwrap();
            let max_w = find_root_increasing(1e-10, 100.0, target, |scale| {
                let scaled: Vec<f64> = alternative
                    .iter()
                    .zip(&null_probs)
                    .map(|(&p, &e)| e + scale * (p - e))
                    .collect();
                let sw: f64 = scaled
                    .iter()
                    .zip(&null_probs)
                    .map(|(&p, &e)| (p - e).powi(2) / e)
                    .sum();
                Ok(1.0 - noncentral_chisq_cdf(chisq_critical(df, spec.alpha)?, df, n as f64 * sw)?)
            })?;
            let scaled: Vec<f64> = alternative
                .iter()
                .zip(&null_probs)
                .map(|(&p, &e)| e + max_w * (p - e))
                .collect();
            scaled
                .iter()
                .zip(&null_probs)
                .map(|(&p, &e)| (p - e).powi(2) / e)
                .sum::<f64>()
                .sqrt()
        }
        _ => w,
    };
    let solved_w_squared = solved_w * solved_w;
    let power = 1.0
        - noncentral_chisq_cdf(
            chisq_critical(df, spec.alpha)?,
            df,
            n as f64 * solved_w_squared,
        )?;
    let expected_small =
        (n as f64 * null_probs.iter().copied().fold(f64::INFINITY, f64::min)) < 5.0;

    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: effective_target,
        alpha: spec.alpha,
        alternative: Alternative::TwoSided,
        effect_size: solved_w,
        effect_size_scale: "Cohen's w for departure from independence".into(),
        method: "noncentral chi-square distribution".into(),
        method_type: "analytic approximation",
        n1: None,
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
            SolveFor::Mde => Some(solved_w),
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "total observations".into(),
            SolveFor::Mde => "Cohen's w".into(),
        },
        input_assumptions: format!(
            "multinomial sampling under the specified {rows}x{columns} alternative; \
             null expected probabilities are row-margin times column-margin products"
        ),
        warnings: if expected_small {
            "minimum null expected count is below 5; the chi-square approximation may be unreliable"
                .into()
        } else {
            String::new()
        },
    })
}
