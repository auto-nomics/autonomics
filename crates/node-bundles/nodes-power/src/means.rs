//! `power_t_test`: prospective power for one-sample, paired, and two-sample t tests.

use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::common::*;

pub const POWER_T_TEST_NODE_KIND: &str = "power_t_test";

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TTestDesign {
    OneSample,
    Paired,
    TwoSample,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerTTestSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_alternative")]
    pub alternative: String,
    pub design: Option<TTestDesign>,
    #[serde(default)]
    pub n: Option<u64>,
    #[serde(default)]
    pub n1: Option<u64>,
    #[serde(default)]
    pub n2: Option<u64>,
    #[serde(default = "default_allocation")]
    pub allocation_ratio: f64,
    #[serde(default)]
    pub effect_size: Option<f64>,
    #[serde(default)]
    pub mean_difference: Option<f64>,
    #[serde(default)]
    pub sd: Option<f64>,
}

pub struct PowerTTestNodeFactory;

impl NodeFactory for PowerTTestNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_T_TEST_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for t tests."
    }
    fn doc(&self) -> &'static str {
        "Computes prospective power for one-sample, paired, or independent-groups \
        t tests using the noncentral t distribution. Supports Cohen's d or raw \
        mean difference with a planning standard deviation."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerTTestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerTTestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerTTestNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerTTestNode {
    meta: NodePorts,
    spec: PowerTTestSpec,
}

#[async_trait]
impl DagNode for PowerTTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_T_TEST_NODE_KIND
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
        match compute(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute(spec: &PowerTTestSpec) -> Result<PowerResult, PowerError> {
    let alternative = Alternative::parse(&spec.alternative)?;
    let target_power = resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, spec.target_power)?;
    let design = spec.design.unwrap_or(TTestDesign::TwoSample);
    if spec.allocation_ratio <= 0.0 || !spec.allocation_ratio.is_finite() {
        return Err(PowerError::Input(
            "allocation_ratio must be a positive finite number".into(),
        ));
    }

    let raw_scale = spec.sd.is_some() && spec.mean_difference.is_some();
    let d = if let Some(direct) = spec.effect_size {
        ensure_solved_input(
            spec.solve_for,
            spec.target_power,
            Some(direct),
            "effect_size",
        )?;
        direct
    } else if raw_scale {
        ensure_solved_input(
            spec.solve_for,
            spec.target_power,
            Some(spec.mean_difference.unwrap()),
            "mean_difference",
        )?;
        if !(spec.sd.unwrap() > 0.0) {
            return Err(PowerError::Input("sd must be positive".into()));
        }
        spec.mean_difference.unwrap() / spec.sd.unwrap()
    } else {
        if spec.sd.is_some() != spec.mean_difference.is_some() {
            return Err(PowerError::Spec(
                "mean_difference and sd must be supplied together".into(),
            ));
        }
        ensure_solved_input(spec.solve_for, spec.target_power, None, "effect_size")?;
        return Err(PowerError::Spec(
            "provide effect_size, or mean_difference and sd".into(),
        ));
    };
    if !d.is_finite() || d == 0.0 {
        return Err(PowerError::Input(
            "the specified effect must be nonzero and finite".into(),
        ));
    }
    if alternative == Alternative::Greater && d < 0.0 {
        return Err(PowerError::Input(
            "a negative effect is incompatible with alternative='greater'".into(),
        ));
    }
    if alternative == Alternative::Less && d > 0.0 {
        return Err(PowerError::Input(
            "a positive effect is incompatible with alternative='less'".into(),
        ));
    }

    let (n1, n2, df, ncp_factor) = match design {
        TTestDesign::OneSample | TTestDesign::Paired => {
            let n = match spec.solve_for {
                SolveFor::Power => spec
                    .n
                    .ok_or_else(|| PowerError::Spec("n is required for power".into()))?,
                SolveFor::SampleSize => {
                    let target = target_power.unwrap();
                    let critical_p = match alternative {
                        Alternative::TwoSided => 1.0 - spec.alpha / 2.0,
                        Alternative::Greater => 1.0 - spec.alpha,
                        Alternative::Less => spec.alpha,
                    };
                    let approximate_n = ((normal_quantile(critical_p)? + normal_quantile(target)?)
                        / d.abs())
                    .powi(2)
                    .ceil() as u64;
                    integer_root_near(2, target, approximate_n, |n| {
                        t_power(design, d, n as f64, 1.0, alternative, spec.alpha)
                    })?
                }
                SolveFor::Mde => spec
                    .n
                    .ok_or_else(|| PowerError::Spec("n is required for mde".into()))?,
            };
            (n, None, (n - 1) as f64, (n as f64).sqrt())
        }
        TTestDesign::TwoSample => match spec.solve_for {
            SolveFor::Power => {
                let n1 = spec
                    .n1
                    .or(spec.n)
                    .ok_or_else(|| PowerError::Spec("n1 (or n) is required for power".into()))?;
                let n2 = spec.n2.ok_or_else(|| {
                    PowerError::Spec("n2 is required for the two-sample design".into())
                })?;
                let factor = 1.0 / (1.0 / n1 as f64 + 1.0 / n2 as f64).sqrt();
                (n1, Some(n2), (n1 + n2 - 2) as f64, factor)
            }
            SolveFor::SampleSize => {
                if spec.n1.is_some() || spec.n2.is_some() || spec.n.is_some() {
                    return Err(PowerError::Spec(
                        "sample-size inputs must not be supplied when solve_for='sample_size' or 'mde'"
                            .into(),
                    ));
                }
                let ratio = spec.allocation_ratio;
                let target = target_power.unwrap();
                let critical_p = match alternative {
                    Alternative::TwoSided => 1.0 - spec.alpha / 2.0,
                    Alternative::Greater => 1.0 - spec.alpha,
                    Alternative::Less => spec.alpha,
                };
                let approximate_n1 =
                    (((normal_quantile(critical_p)? + normal_quantile(target)?) / d.abs()).powi(2)
                        * (1.0 + ratio))
                        .ceil() as u64;
                let n1 = integer_root_near(2, target, approximate_n1, |n1| {
                    t_power(
                        TTestDesign::TwoSample,
                        d,
                        n1 as f64,
                        ratio,
                        alternative,
                        spec.alpha,
                    )
                })?;
                let n2 = (n1 as f64 * ratio).ceil() as u64;
                let factor = 1.0 / (1.0 / n1 as f64 + 1.0 / n2 as f64).sqrt();
                (n1, Some(n2), (n1 + n2 - 2) as f64, factor)
            }
            SolveFor::Mde => {
                let n1 = spec
                    .n1
                    .or(spec.n)
                    .ok_or_else(|| PowerError::Spec("n1 (or n) is required for mde".into()))?;
                let n2 = spec.n2.ok_or_else(|| {
                    PowerError::Spec("n2 is required for the two-sample design".into())
                })?;
                let factor = 1.0 / (1.0 / n1 as f64 + 1.0 / n2 as f64).sqrt();
                (n1, Some(n2), (n1 + n2 - 2) as f64, factor)
            }
        },
    };

    let solved_ncp = match spec.solve_for {
        SolveFor::Mde => {
            let target = target_power.unwrap();
            find_root_increasing(1e-8, 1e4, target, |ncp| {
                power_noncentral_t(
                    df,
                    ncp,
                    t_critical(df, spec.alpha, Alternative::Greater)?,
                    Alternative::Greater,
                )
            })?
        }
        _ => d.abs() * ncp_factor,
    };
    let solved_d = solved_ncp / ncp_factor;
    let signed_d = if alternative == Alternative::Less {
        -solved_d
    } else {
        solved_d
    };
    let power = power_noncentral_t(
        df,
        signed_d * ncp_factor,
        t_critical(df, spec.alpha, alternative)?,
        alternative,
    )?;

    let raw_mde = if raw_scale {
        Some(signed_d * spec.sd.unwrap())
    } else {
        None
    };
    let result_value = match spec.solve_for {
        SolveFor::Power => Some(power),
        SolveFor::SampleSize => Some(n1 as f64 + n2.map(|n| n as f64).unwrap_or_default()),
        SolveFor::Mde => Some(raw_mde.unwrap_or(signed_d)),
    };
    let assumptions = match design {
        TTestDesign::OneSample => "one-sample t test; normal outcome".to_string(),
        TTestDesign::Paired => "paired t test; normal differences".to_string(),
        TTestDesign::TwoSample => {
            "independent-groups t test; normal outcomes and equal group variances".to_string()
        }
    };

    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: if spec.solve_for == SolveFor::Power {
            None
        } else {
            target_power
        },
        alpha: spec.alpha,
        alternative,
        effect_size: signed_d,
        effect_size_scale: if raw_scale {
            "Cohen's d (from mean_difference / sd)".into()
        } else {
            "Cohen's d".into()
        },
        method: "noncentral t distribution".into(),
        method_type: "analytic exact under model assumptions",
        n1: Some(n1),
        n2,
        total_n: Some(n1 + n2.unwrap_or_default()),
        clusters1: None,
        clusters2: None,
        total_clusters: None,
        events: None,
        design_effect: None,
        result_value,
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "total observations".into(),
            SolveFor::Mde => {
                if raw_scale {
                    "mean difference".into()
                } else {
                    "Cohen's d".into()
                }
            }
        },
        input_assumptions: format!(
            "{assumptions}; planning effect is fixed, not estimated from the analyzed data{}",
            if raw_scale {
                "; sd is a design assumption"
            } else {
                ""
            }
        ),
        warnings: if matches!(design, TTestDesign::OneSample | TTestDesign::Paired) && n1 < 10 {
            "small df makes normality assumptions influential".into()
        } else {
            String::new()
        },
    })
}

fn t_power(
    design: TTestDesign,
    d: f64,
    n1: f64,
    allocation_ratio: f64,
    alternative: Alternative,
    alpha: f64,
) -> Result<f64, PowerError> {
    let (df, factor) = match design {
        TTestDesign::OneSample | TTestDesign::Paired => (n1 - 1.0, n1.sqrt()),
        TTestDesign::TwoSample => {
            let n2 = (n1 * allocation_ratio).max(1.0);
            (n1 + n2 - 2.0, 1.0 / (1.0 / n1 + 1.0 / n2).sqrt())
        }
    };
    if df <= 0.0 {
        return Ok(0.0);
    }
    power_noncentral_t(
        df,
        d.abs() * factor,
        t_critical(df, alpha, alternative)?,
        alternative,
    )
}
