//! `power_correlation`: prospective power for testing a Pearson correlation.

use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::common::*;

pub const POWER_CORRELATION_NODE_KIND: &str = "power_correlation";

fn default_alpha() -> f64 {
    0.05
}
fn default_power() -> f64 {
    0.8
}
fn default_alternative() -> String {
    "two_sided".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerCorrelationSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_alternative")]
    pub alternative: String,
    #[serde(default)]
    pub n: Option<u64>,
    #[serde(default)]
    pub correlation: Option<f64>,
}

pub struct PowerCorrelationNodeFactory;

impl NodeFactory for PowerCorrelationNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_CORRELATION_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for a Pearson correlation test."
    }
    fn doc(&self) -> &'static str {
        "Tests H0: rho=0 using the Fisher z normal approximation. Reports \
        the approximation explicitly in the result row."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerCorrelationSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerCorrelationSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerCorrelationNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerCorrelationNode {
    meta: NodePorts,
    spec: PowerCorrelationSpec,
}

#[async_trait]
impl DagNode for PowerCorrelationNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_CORRELATION_NODE_KIND
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

fn compute(spec: &PowerCorrelationSpec) -> Result<PowerResult, PowerError> {
    let alternative = Alternative::parse(&spec.alternative)?;
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    let supplied_r = spec.correlation;
    ensure_solved_input(spec.solve_for, effective_target, supplied_r, "correlation")?;
    if let Some(r) = supplied_r {
        if !(-1.0..=1.0).contains(&r) || r == 0.0 {
            return Err(PowerError::Input(
                "correlation must be nonzero and in [-1, 1]".into(),
            ));
        }
        if alternative == Alternative::Greater && r < 0.0
            || alternative == Alternative::Less && r > 0.0
        {
            return Err(PowerError::Input(
                "the effect direction is incompatible with the one-sided alternative".into(),
            ));
        }
    }

    let n = match spec.solve_for {
        SolveFor::Power => spec
            .n
            .ok_or_else(|| PowerError::Spec("n is required for power".into()))?,
        SolveFor::SampleSize => integer_root_up(4, 10_000_000, effective_target.unwrap(), |n| {
            correlation_power(supplied_r.unwrap(), n as f64, alternative, spec.alpha)
        })?,
        SolveFor::Mde => spec
            .n
            .ok_or_else(|| PowerError::Spec("n is required for mde".into()))?,
    };
    if n < 4 {
        return Err(PowerError::Input("n must be at least 4".into()));
    }

    let solved_r = match spec.solve_for {
        SolveFor::Mde => {
            let target = effective_target.unwrap();
            let z = find_root_increasing(1e-10, 10.0, target, |z| {
                correlation_power(z.tanh(), n as f64, Alternative::Greater, spec.alpha)
            })?;
            let signed = if alternative == Alternative::Less {
                -z
            } else {
                z
            };
            signed.tanh()
        }
        _ => supplied_r.unwrap(),
    };
    let power = correlation_power(solved_r, n as f64, alternative, spec.alpha)?;

    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: effective_target,
        alpha: spec.alpha,
        alternative,
        effect_size: solved_r,
        effect_size_scale: "Pearson correlation coefficient (rho)".into(),
        method: "Fisher z normal approximation".into(),
        method_type: "normal approximation",
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
            SolveFor::Mde => Some(solved_r),
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "observations".into(),
            SolveFor::Mde => "Pearson correlation coefficient".into(),
        },
        input_assumptions:
            "bivariate-normal pairs; H0: rho=0; the specified correlation is a design effect".into(),
        warnings: if n < 20 {
            "Fisher z is less accurate in very small samples".into()
        } else {
            String::new()
        },
    })
}

fn correlation_power(
    rho: f64,
    n: f64,
    alternative: Alternative,
    alpha: f64,
) -> Result<f64, PowerError> {
    if n < 4.0 || !(-1.0..=1.0).contains(&rho) || rho == 0.0 {
        return Ok(0.0);
    }
    let z = rho.atanh();
    let ncp = z * (n - 3.0).sqrt();
    let critical = match alternative {
        Alternative::TwoSided => normal_quantile(1.0 - alpha / 2.0)?,
        Alternative::Greater => normal_quantile(1.0 - alpha)?,
        Alternative::Less => normal_quantile(alpha)?,
    };
    let upper = 1.0 - normal_cdf(critical - ncp)?;
    let power = match alternative {
        Alternative::Greater => upper,
        Alternative::Less => normal_cdf(critical - ncp)?,
        Alternative::TwoSided => upper + normal_cdf(-critical - ncp)?,
    };
    Ok(power.clamp(0.0, 1.0))
}
