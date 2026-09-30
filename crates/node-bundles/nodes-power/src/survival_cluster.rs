//! `power_survival` and `power_cluster` design nodes.

use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::common::*;

pub const POWER_SURVIVAL_NODE_KIND: &str = "power_survival";
pub const POWER_CLUSTER_NODE_KIND: &str = "power_cluster";

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
fn default_zero() -> f64 {
    0.0
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerSurvivalSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_alternative")]
    pub alternative: String,
    #[serde(default)]
    pub total_events: Option<u64>,
    #[serde(default)]
    pub total_subjects: Option<u64>,
    #[serde(default = "default_allocation")]
    pub allocation_ratio: f64,
    #[serde(default)]
    pub hazard_ratio: Option<f64>,
    #[serde(default)]
    pub control_hazard: Option<f64>,
    #[serde(default)]
    pub follow_up_duration: Option<f64>,
    #[serde(default = "default_zero")]
    pub accrual_duration: f64,
    #[serde(default = "default_zero")]
    pub dropout: f64,
}

pub struct PowerSurvivalNodeFactory;

impl NodeFactory for PowerSurvivalNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_SURVIVAL_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for a two-group log-rank or Cox test."
    }
    fn doc(&self) -> &'static str {
        "Uses the Schoenfeld normal approximation for the required number of \
        events, then converts events to enrollment using exponential event-rate \
        and uniform-accrual assumptions."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerSurvivalSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerSurvivalSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerSurvivalNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerSurvivalNode {
    meta: NodePorts,
    spec: PowerSurvivalSpec,
}

#[async_trait]
impl DagNode for PowerSurvivalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_SURVIVAL_NODE_KIND
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
        match compute_survival(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute_survival(spec: &PowerSurvivalSpec) -> Result<PowerResult, PowerError> {
    let alternative = Alternative::parse(&spec.alternative)?;
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    if !(spec.allocation_ratio > 0.0)
        || !spec.allocation_ratio.is_finite()
        || !(0.0..=0.99).contains(&spec.dropout)
        || !(spec.accrual_duration >= 0.0)
    {
        return Err(PowerError::Input(
            "allocation_ratio and accrual must be positive/zero as appropriate; dropout < 1".into(),
        ));
    }
    ensure_solved_input(
        spec.solve_for,
        effective_target,
        spec.hazard_ratio,
        "hazard_ratio",
    )?;
    let supplied_hr = spec.hazard_ratio;
    if let Some(hr) = supplied_hr {
        if !(hr > 0.0) || !hr.is_finite() || hr == 1.0 {
            return Err(PowerError::Input(
                "hazard_ratio must be positive, finite, and different from 1".into(),
            ));
        }
        if alternative == Alternative::Greater && hr < 1.0
            || alternative == Alternative::Less && hr > 1.0
        {
            return Err(PowerError::Input(
                "hazard-ratio direction is incompatible with the one-sided alternative".into(),
            ));
        }
    }

    let event_model = spec.control_hazard.is_some() || spec.follow_up_duration.is_some();
    if event_model {
        if spec.control_hazard.is_none() || spec.follow_up_duration.is_none() {
            return Err(PowerError::Spec(
                "control_hazard and follow_up_duration must be supplied together".into(),
            ));
        }
        let hazard = spec.control_hazard.unwrap();
        let duration = spec.follow_up_duration.unwrap();
        if !(hazard > 0.0) || !(duration > 0.0) {
            return Err(PowerError::Input(
                "control_hazard and follow_up_duration must be positive".into(),
            ));
        }
    }

    let allocation_fraction = 1.0 / (1.0 + spec.allocation_ratio);
    let target = effective_target;
    if spec.solve_for == SolveFor::SampleSize
        && (spec.total_events.is_some() || spec.total_subjects.is_some())
    {
        return Err(PowerError::Spec(
            "event or subject counts must not be supplied when solve_for='sample_size'".into(),
        ));
    }
    let (events, subjects, _expected_prob, hr) = match spec.solve_for {
        SolveFor::Power => {
            let hr = supplied_hr.unwrap();
            if let Some(events) = spec.total_events {
                (events, spec.total_subjects, None, hr)
            } else {
                let subjects = spec.total_subjects.ok_or_else(|| {
                    PowerError::Spec(
                        "provide total_events, or total_subjects with event-rate assumptions"
                            .into(),
                    )
                })?;
                let probability = expected_event_probability(
                    hr,
                    spec.control_hazard,
                    spec.follow_up_duration,
                    spec.accrual_duration,
                    allocation_fraction,
                )?;
                let actual_events = (subjects as f64 * probability).floor() as u64;
                (actual_events, Some(subjects), Some(probability), hr)
            }
        }
        SolveFor::SampleSize => {
            let hr = supplied_hr.unwrap();
            let probability = expected_event_probability(
                hr,
                spec.control_hazard,
                spec.follow_up_duration,
                spec.accrual_duration,
                allocation_fraction,
            )?;
            let required = integer_root_up(2, 100_000_000, target.unwrap(), |events| {
                survival_power(
                    hr.max(1.0 / hr),
                    events as f64,
                    allocation_fraction,
                    if alternative == Alternative::TwoSided {
                        Alternative::TwoSided
                    } else {
                        Alternative::Greater
                    },
                    spec.alpha,
                )
            })?;
            let analyzable = (required as f64 / ((1.0 - spec.dropout) * probability)).ceil();
            let total_subjects = analyzable as u64;
            let n1 = (total_subjects as f64 * allocation_fraction).ceil() as u64;
            let n2 = total_subjects - n1;
            let actual_events =
                ((n1 + n2) as f64 * (1.0 - spec.dropout) * probability).floor() as u64;
            (actual_events, Some(total_subjects), Some(probability), hr)
        }
        SolveFor::Mde => {
            if let Some(events) = spec.total_events {
                let solved_hr = solve_survival_mde(
                    events as f64,
                    target.unwrap(),
                    allocation_fraction,
                    spec.alpha,
                    alternative,
                )?;
                (events, spec.total_subjects, None, solved_hr)
            } else {
                let subjects = spec.total_subjects.ok_or_else(|| {
                    PowerError::Spec("provide total_events or total_subjects for mde".into())
                })?;
                let (events, probability, solved_hr) = solve_survival_mde_from_subjects(
                    subjects as f64,
                    target.unwrap(),
                    spec.control_hazard,
                    spec.follow_up_duration,
                    spec.accrual_duration,
                    spec.dropout,
                    allocation_fraction,
                    spec.alpha,
                    alternative,
                )?;
                (events, Some(subjects), Some(probability), solved_hr)
            }
        }
    };
    if events == 0 {
        return Err(PowerError::Input(
            "the design yields zero expected events".into(),
        ));
    }
    let power = survival_power(
        hr,
        events as f64,
        allocation_fraction,
        alternative,
        spec.alpha,
    )?;
    let n1 = subjects.map(|n| (n as f64 * allocation_fraction).ceil() as u64);
    let n2 = subjects.map(|n| n - n1.unwrap_or_default());
    let mut warnings = vec![
        "Schoenfeld approximation assumes proportional hazards and approximately balanced event information".to_string(),
    ];
    if event_model {
        warnings.push(
            "event-rate conversion assumes exponential survival and uniform accrual".to_string(),
        );
    }
    if spec.dropout > 0.0 {
        warnings.push("dropout is applied as a uniform retention factor".to_string());
    }

    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: effective_target,
        alpha: spec.alpha,
        alternative,
        effect_size: hr,
        effect_size_scale: "hazard ratio (treatment/control)".into(),
        method: "Schoenfeld log-rank/Cox normal approximation".into(),
        method_type: "normal approximation",
        n1,
        n2,
        total_n: subjects,
        clusters1: None,
        clusters2: None,
        total_clusters: None,
        events: Some(events),
        design_effect: None,
        result_value: match spec.solve_for {
            SolveFor::Power => Some(power),
            SolveFor::SampleSize => Some(subjects.unwrap_or_default() as f64),
            SolveFor::Mde => Some(hr),
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "enrolled subjects (events reported separately)".into(),
            SolveFor::Mde => "hazard ratio".into(),
        },
        input_assumptions: format!(
            "allocation ratio n2/n1={}; control hazard and follow-up assumptions are design inputs, \
             not estimates recovered from the observed test",
            spec.allocation_ratio
        ),
        warnings: warnings.join("; "),
    })
}

fn expected_event_probability(
    hr: f64,
    control_hazard: Option<f64>,
    follow_up: Option<f64>,
    accrual: f64,
    allocation_fraction: f64,
) -> Result<f64, PowerError> {
    let lambda = control_hazard.ok_or_else(|| {
        PowerError::Spec("control_hazard is required to convert events to subjects".into())
    })?;
    let duration = follow_up.ok_or_else(|| {
        PowerError::Spec("follow_up_duration is required to convert events to subjects".into())
    })?;
    if !(lambda > 0.0) || !(duration > 0.0) || !(accrual >= 0.0) {
        return Err(PowerError::Input(
            "event-rate assumptions must be positive".into(),
        ));
    }
    fn average_event(hazard: f64, minimum_follow_up: f64, accrual: f64) -> f64 {
        if accrual == 0.0 {
            1.0 - (-hazard * minimum_follow_up).exp()
        } else {
            1.0 - ((-hazard * minimum_follow_up).exp()
                - (-hazard * (minimum_follow_up + accrual)).exp())
                / (hazard * accrual)
        }
    }
    let control = average_event(lambda, duration, accrual);
    let treatment = average_event(lambda * hr, duration, accrual);
    Ok(allocation_fraction * control + (1.0 - allocation_fraction) * treatment)
}

fn survival_power(
    hr: f64,
    events: f64,
    allocation_fraction: f64,
    alternative: Alternative,
    alpha: f64,
) -> Result<f64, PowerError> {
    if !(hr > 0.0) || events <= 0.0 || !(0.0..=1.0).contains(&allocation_fraction) {
        return Ok(0.0);
    }
    let information = events * allocation_fraction * (1.0 - allocation_fraction);
    let ncp = hr.ln() * information.sqrt();
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

fn solve_survival_mde(
    events: f64,
    target: f64,
    allocation_fraction: f64,
    alpha: f64,
    alternative: Alternative,
) -> Result<f64, PowerError> {
    let log_hr = find_root_increasing(1e-8, 8.0, target, |log_hr| {
        survival_power(
            log_hr.exp(),
            events,
            allocation_fraction,
            Alternative::Greater,
            alpha,
        )
    })?;
    let signed = if alternative == Alternative::Less {
        -log_hr
    } else {
        log_hr
    };
    Ok(signed.exp())
}

#[allow(clippy::too_many_arguments)]
fn solve_survival_mde_from_subjects(
    subjects: f64,
    target: f64,
    control_hazard: Option<f64>,
    follow_up: Option<f64>,
    accrual: f64,
    dropout: f64,
    allocation_fraction: f64,
    alpha: f64,
    alternative: Alternative,
) -> Result<(u64, f64, f64), PowerError> {
    let log_hr = find_root_increasing(1e-8, 8.0, target, |log_hr| {
        let hr = log_hr.exp();
        let probability = expected_event_probability(
            hr,
            control_hazard,
            follow_up,
            accrual,
            allocation_fraction,
        )?;
        let events = (subjects * (1.0 - dropout) * probability).floor();
        survival_power(hr, events, allocation_fraction, Alternative::Greater, alpha)
    })?;
    let signed_log_hr = if alternative == Alternative::Less {
        -log_hr
    } else {
        log_hr
    };
    let hr = signed_log_hr.exp();
    let probability =
        expected_event_probability(hr, control_hazard, follow_up, accrual, allocation_fraction)?;
    let events = (subjects * (1.0 - dropout) * probability).floor() as u64;
    Ok((events, probability, hr))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterOutcome {
    Continuous,
    Proportion,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PowerClusterSpec {
    pub solve_for: SolveFor,
    #[serde(default)]
    pub target_power: Option<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_alternative")]
    pub alternative: String,
    #[serde(default)]
    pub outcome: Option<ClusterOutcome>,
    #[serde(default)]
    pub clusters: Option<u64>,
    #[serde(default)]
    pub clusters1: Option<u64>,
    #[serde(default)]
    pub clusters2: Option<u64>,
    #[serde(default)]
    pub mean_cluster_size: Option<f64>,
    #[serde(default = "default_allocation")]
    pub allocation_ratio: f64,
    #[serde(default)]
    pub icc: Option<f64>,
    #[serde(default = "default_zero")]
    pub dropout: f64,
    #[serde(default)]
    pub effect_size: Option<f64>,
    #[serde(default)]
    pub mean_difference: Option<f64>,
    #[serde(default)]
    pub sd: Option<f64>,
    #[serde(default)]
    pub p1: Option<f64>,
    #[serde(default)]
    pub p2: Option<f64>,
}

pub struct PowerClusterNodeFactory;

impl NodeFactory for PowerClusterNodeFactory {
    fn kind(&self) -> &'static str {
        POWER_CLUSTER_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Power, sample size, or MDE for cluster-randomized or cluster-sampled designs."
    }
    fn doc(&self) -> &'static str {
        "Applies a design-effect approximation based on ICC and average analyzable \
        cluster size, then computes the underlying two-group test power."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerClusterSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PowerClusterSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerClusterNode {
            meta: self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
pub struct PowerClusterNode {
    meta: NodePorts,
    spec: PowerClusterSpec,
}

#[async_trait]
impl DagNode for PowerClusterNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        POWER_CLUSTER_NODE_KIND
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
        match compute_cluster(&self.spec) {
            Ok(result) => emit_result(ctx, result),
            Err(e) => Err(e.into()),
        }
    }
}

fn compute_cluster(spec: &PowerClusterSpec) -> Result<PowerResult, PowerError> {
    let alternative = Alternative::parse(&spec.alternative)?;
    let effective_target =
        resolve_target_power(spec.solve_for, spec.target_power, default_power())?;
    validate_common(spec.alpha, effective_target)?;
    let outcome = spec.outcome.unwrap_or(ClusterOutcome::Continuous);
    let m = spec
        .mean_cluster_size
        .ok_or_else(|| PowerError::Spec("mean_cluster_size is required".into()))?;
    let icc = spec
        .icc
        .ok_or_else(|| PowerError::Spec("icc is required".into()))?;
    if !(m > 0.0) || !m.is_finite() || !(0.0..=1.0).contains(&icc) {
        return Err(PowerError::Input(
            "mean_cluster_size must be positive and ICC in [0,1]".into(),
        ));
    }
    if !(spec.allocation_ratio > 0.0) || !(0.0..=0.99).contains(&spec.dropout) {
        return Err(PowerError::Input(
            "allocation_ratio must be positive and dropout below 1".into(),
        ));
    }
    let raw_mean = spec.sd.is_some() && spec.mean_difference.is_some();
    let supplied_effect = if outcome == ClusterOutcome::Continuous {
        if raw_mean {
            if !(spec.sd.unwrap() > 0.0) {
                return Err(PowerError::Input("sd must be positive".into()));
            }
            Some(spec.mean_difference.unwrap() / spec.sd.unwrap())
        } else {
            spec.effect_size
        }
    } else {
        match (spec.p1, spec.p2) {
            (Some(p1), Some(p2)) => {
                if !(0.0..=1.0).contains(&p1) || !(0.0..=1.0).contains(&p2) || p1 == p2 {
                    return Err(PowerError::Input(
                        "p1 and p2 must be distinct proportions in [0,1]".into(),
                    ));
                }
                Some(2.0 * (p2.sqrt().asin() - p1.sqrt().asin()))
            }
            (None, None) => spec.effect_size,
            _ => return Err(PowerError::Spec("both p1 and p2 are required".into())),
        }
    };
    ensure_solved_input(spec.solve_for, effective_target, supplied_effect, "effect")?;
    if let Some(effect) = supplied_effect {
        if !effect.is_finite() || effect == 0.0 {
            return Err(PowerError::Input(
                "the design effect must be nonzero and finite".into(),
            ));
        }
        if alternative == Alternative::Greater && effect < 0.0
            || alternative == Alternative::Less && effect > 0.0
        {
            return Err(PowerError::Input(
                "effect direction is incompatible with the one-sided alternative".into(),
            ));
        }
    }

    let analyzable_m = m * (1.0 - spec.dropout);
    let design_effect = 1.0 + (analyzable_m - 1.0) * icc;
    let ratio = spec.allocation_ratio;
    let explicit_pair = match (spec.clusters1, spec.clusters2) {
        (Some(c1), Some(c2)) => Some((c1, c2)),
        (None, None) => None,
        _ => {
            return Err(PowerError::Spec(
                "supply both clusters1 and clusters2, or total clusters only".into(),
            ));
        }
    };
    if spec.clusters.is_some() && explicit_pair.is_some() {
        return Err(PowerError::Spec(
            "supply total clusters or clusters1/clusters2, not both".into(),
        ));
    }
    let cluster_power =
        |c1: u64, c2: u64, effect: f64, test_alternative: Alternative| -> Result<f64, PowerError> {
            let effective_n1 = c1 as f64 * analyzable_m / design_effect;
            let effective_n2 = c2 as f64 * analyzable_m / design_effect;
            proportion_effect_power(
                effect,
                effective_n1,
                effective_n2,
                test_alternative,
                spec.alpha,
            )
        };
    let cluster_power_for_total = |total_clusters: u64,
                                   effect: f64,
                                   test_alternative: Alternative|
     -> Result<f64, PowerError> {
        let c1 = ((total_clusters as f64 / (1.0 + ratio)).ceil()).max(1.0) as u64;
        let c2 = ((c1 as f64 * ratio).ceil()).max(1.0) as u64;
        cluster_power(c1, c2, effect, test_alternative)
    };

    let total_clusters = match spec.solve_for {
        SolveFor::Power => spec
            .clusters
            .or_else(|| explicit_pair.map(|(c1, c2)| c1 + c2))
            .ok_or_else(|| PowerError::Spec("clusters is required for power".into()))?,
        SolveFor::SampleSize => {
            if spec.clusters.is_some() || spec.clusters1.is_some() || spec.clusters2.is_some() {
                return Err(PowerError::Spec(
                    "cluster counts must not be supplied when solve_for='sample_size'".into(),
                ));
            }
            let allocation_fraction = 1.0 / (1.0 + ratio);
            let critical_p = match alternative {
                Alternative::TwoSided => 1.0 - spec.alpha / 2.0,
                Alternative::Greater => 1.0 - spec.alpha,
                Alternative::Less => spec.alpha,
            };
            let effective_n_needed = ((normal_quantile(critical_p)?
                + normal_quantile(effective_target.unwrap())?)
                / supplied_effect.unwrap().abs())
            .powi(2)
                / (allocation_fraction * (1.0 - allocation_fraction));
            let approximate_clusters =
                (effective_n_needed * design_effect / analyzable_m).ceil() as u64;
            integer_root_near(
                2,
                effective_target.unwrap(),
                approximate_clusters,
                |clusters| cluster_power_for_total(clusters, supplied_effect.unwrap(), alternative),
            )?
        }
        SolveFor::Mde => spec
            .clusters
            .or_else(|| explicit_pair.map(|(c1, c2)| c1 + c2))
            .ok_or_else(|| PowerError::Spec("clusters is required for mde".into()))?,
    };
    let (c1, c2) = explicit_pair.unwrap_or_else(|| {
        let c1 = ((total_clusters as f64 / (1.0 + ratio)).ceil()).max(1.0) as u64;
        let c2 = ((c1 as f64 * ratio).ceil()).max(1.0) as u64;
        (c1, c2)
    });
    let solved_effect = match spec.solve_for {
        SolveFor::Mde => find_root_increasing(
            1e-10,
            if outcome == ClusterOutcome::Continuous {
                100.0
            } else {
                std::f64::consts::PI
            },
            effective_target.unwrap(),
            |effect| cluster_power(c1, c2, effect, Alternative::Greater),
        )?,
        _ => supplied_effect.unwrap(),
    };
    let signed_effect = if alternative == Alternative::Less {
        -solved_effect
    } else {
        solved_effect
    };
    let allocated_total_clusters = c1 + c2;
    let power = cluster_power(c1, c2, signed_effect, alternative)?;
    let enrolled_total = ((c1 + c2) as f64 * m).ceil() as u64;
    let analyzable_total = ((c1 + c2) as f64 * analyzable_m).floor() as u64;

    let (scale, result_value) = if outcome == ClusterOutcome::Continuous {
        (
            "Cohen's d".to_string(),
            if raw_mean {
                Some(signed_effect * spec.sd.unwrap())
            } else {
                Some(signed_effect)
            },
        )
    } else {
        (
            "Cohen's h (arcsine effect)".to_string(),
            Some(signed_effect),
        )
    };
    let mut warnings = vec![
        "design-effect approximation assumes equal cluster sizes and an exchangeable within-cluster correlation"
            .to_string(),
    ];
    if total_clusters < 20 {
        warnings.push(
            "few clusters make the design-effect approximation unreliable; simulation is preferable"
                .to_string(),
        );
    }
    Ok(PowerResult {
        solve_for: spec.solve_for,
        power,
        actual_power: power,
        target_power: effective_target,
        alpha: spec.alpha,
        alternative,
        effect_size: signed_effect,
        effect_size_scale: scale.clone(),
        method: if outcome == ClusterOutcome::Continuous {
            "design effect plus normal approximation".to_string()
        } else {
            "design effect plus arcsine normal approximation".to_string()
        },
        method_type: "approximate design-effect method",
        n1: Some(((c1 as f64 * m).ceil()) as u64),
        n2: Some(((c2 as f64 * m).ceil()) as u64),
        total_n: Some(enrolled_total),
        clusters1: Some(c1),
        clusters2: Some(c2),
        total_clusters: Some(allocated_total_clusters),
        events: None,
        design_effect: Some(design_effect),
        result_value: match spec.solve_for {
            SolveFor::Power => Some(power),
            SolveFor::SampleSize => Some(allocated_total_clusters as f64),
            SolveFor::Mde => result_value,
        },
        result_unit: match spec.solve_for {
            SolveFor::Power => "power".into(),
            SolveFor::SampleSize => "total clusters".into(),
            SolveFor::Mde => {
                if outcome == ClusterOutcome::Continuous && raw_mean {
                    "mean difference".into()
                } else {
                    scale
                }
            }
        },
        input_assumptions: format!(
            "mean enrolled cluster size={m}; ICC={icc}; retention={}; analyzable observations={analyzable_total}; \
             cluster counts and ICC are fixed planning assumptions",
            1.0 - spec.dropout
        ),
        warnings: warnings.join("; "),
    })
}

fn proportion_effect_power(
    h: f64,
    n1: f64,
    n2: f64,
    alternative: Alternative,
    alpha: f64,
) -> Result<f64, PowerError> {
    let ncp = h / (1.0 / n1 + 1.0 / n2).sqrt();
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
