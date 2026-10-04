//! Shared contracts and numerical routines for power nodes.

use std::sync::{Arc, OnceLock};

use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::NodeCtx;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use statrs::distribution::{Beta, ChiSquared, ContinuousCDF, FisherSnedecor, Normal, StudentsT};
use statrs::function::gamma::ln_gamma;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PowerError {
    #[error("invalid specification: {0}")]
    Spec(String),
    #[error("invalid input: {0}")]
    Input(String),
    #[error("numerical calculation failed: {0}")]
    Numerical(String),
    #[error("solver failed to bracket the target power")]
    Solver,
    #[error("output assembly failed: {0}")]
    Output(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl ::dag_core::dag::NodeError for PowerError {
    fn node_type(&self) -> &str {
        "power"
    }
}

impl From<arrow_schema::ArrowError> for PowerError {
    fn from(value: arrow_schema::ArrowError) -> Self {
        Self::Output(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SolveFor {
    Power,
    SampleSize,
    Mde,
}

impl SolveFor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Power => "power",
            Self::SampleSize => "sample_size",
            Self::Mde => "mde",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Alternative {
    TwoSided,
    Greater,
    Less,
}

impl Alternative {
    pub fn parse(value: &str) -> Result<Self, PowerError> {
        match value.replace('.', "_").to_lowercase().as_str() {
            "two_sided" | "twosided" | "two" => Ok(Self::TwoSided),
            "greater" => Ok(Self::Greater),
            "less" => Ok(Self::Less),
            _ => Err(PowerError::Spec(format!(
                "alternative must be two_sided, greater, or less; got '{value}'"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TwoSided => "two_sided",
            Self::Greater => "greater",
            Self::Less => "less",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PowerResult {
    pub solve_for: SolveFor,
    pub power: f64,
    pub actual_power: f64,
    pub target_power: Option<f64>,
    pub alpha: f64,
    pub alternative: Alternative,
    pub effect_size: f64,
    pub effect_size_scale: String,
    pub method: String,
    pub method_type: &'static str,
    pub n1: Option<u64>,
    pub n2: Option<u64>,
    pub total_n: Option<u64>,
    pub clusters1: Option<u64>,
    pub clusters2: Option<u64>,
    pub total_clusters: Option<u64>,
    pub events: Option<u64>,
    pub design_effect: Option<f64>,
    pub result_value: Option<f64>,
    pub result_unit: String,
    pub input_assumptions: String,
    pub warnings: String,
}

pub fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("solve_for", DataType::Utf8, false),
        Field::new("power", DataType::Float64, false),
        Field::new("actual_power", DataType::Float64, false),
        Field::new("target_power", DataType::Float64, true),
        Field::new("alpha", DataType::Float64, false),
        Field::new("alternative", DataType::Utf8, false),
        Field::new("effect_size", DataType::Float64, false),
        Field::new("effect_size_scale", DataType::Utf8, false),
        Field::new("method", DataType::Utf8, false),
        Field::new("method_type", DataType::Utf8, false),
        Field::new("n1", DataType::UInt64, true),
        Field::new("n2", DataType::UInt64, true),
        Field::new("total_n", DataType::UInt64, true),
        Field::new("clusters1", DataType::UInt64, true),
        Field::new("clusters2", DataType::UInt64, true),
        Field::new("total_clusters", DataType::UInt64, true),
        Field::new("events", DataType::UInt64, true),
        Field::new("design_effect", DataType::Float64, true),
        Field::new("result_value", DataType::Float64, true),
        Field::new("result_unit", DataType::Utf8, false),
        Field::new("input_assumptions", DataType::Utf8, false),
        Field::new("warnings", DataType::Utf8, false),
    ]))
}

pub fn emit_result(ctx: &NodeCtx, result: PowerResult) -> Result<PortOutputs, DagError> {
    if !result.power.is_finite() || !result.actual_power.is_finite() {
        return Err(PowerError::Numerical("non-finite power".into()).into());
    }
    if !(0.0..=1.0).contains(&result.power) || !(0.0..=1.0).contains(&result.actual_power) {
        return Err(PowerError::Numerical("power is outside [0, 1]".into()).into());
    }

    let batch = result_to_batch(&result)?;
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| PowerError::ReadBatch(e.to_string()))?;
    let mut outputs = PortOutputs::new();
    outputs.insert(0, df);
    Ok(outputs)
}

fn result_to_batch(r: &PowerResult) -> Result<RecordBatch, PowerError> {
    fn f64_col(value: f64) -> ArrayRef {
        Arc::new(Float64Array::from(vec![value]))
    }
    fn opt_f64_col(value: Option<f64>) -> ArrayRef {
        Arc::new(Float64Array::from(vec![value]))
    }
    fn u64_col(value: Option<u64>) -> ArrayRef {
        Arc::new(UInt64Array::from(vec![value]))
    }
    fn string_col(value: &str) -> ArrayRef {
        Arc::new(StringArray::from(vec![value]))
    }

    Ok(RecordBatch::try_new(
        output_schema(),
        vec![
            string_col(r.solve_for.as_str()),
            f64_col(r.power),
            f64_col(r.actual_power),
            opt_f64_col(r.target_power),
            f64_col(r.alpha),
            string_col(r.alternative.as_str()),
            f64_col(r.effect_size),
            string_col(&r.effect_size_scale),
            string_col(&r.method),
            string_col(r.method_type),
            u64_col(r.n1),
            u64_col(r.n2),
            u64_col(r.total_n),
            u64_col(r.clusters1),
            u64_col(r.clusters2),
            u64_col(r.total_clusters),
            u64_col(r.events),
            opt_f64_col(r.design_effect),
            opt_f64_col(r.result_value),
            string_col(&r.result_unit),
            string_col(&r.input_assumptions),
            string_col(&r.warnings),
        ],
    )?)
}

pub fn validate_common(alpha: f64, target_power: Option<f64>) -> Result<(), PowerError> {
    if !(0.0..=1.0).contains(&alpha) || alpha <= 0.0 || alpha >= 1.0 {
        return Err(PowerError::Input("alpha must be in (0, 1)".into()));
    }
    if let Some(p) = target_power {
        if !(0.0..=1.0).contains(&p) || p <= 0.0 || p >= 1.0 {
            return Err(PowerError::Input("target_power must be in (0, 1)".into()));
        }
    }
    Ok(())
}

pub fn resolve_target_power(
    solve_for: SolveFor,
    target_power: Option<f64>,
    default: f64,
) -> Result<Option<f64>, PowerError> {
    match solve_for {
        SolveFor::Power => {
            if target_power.is_some() {
                Err(PowerError::Spec(
                    "target_power must not be supplied when solve_for='power'".into(),
                ))
            } else {
                Ok(None)
            }
        }
        SolveFor::SampleSize | SolveFor::Mde => Ok(Some(target_power.unwrap_or(default))),
    }
}

pub fn ensure_solved_input(
    solve_for: SolveFor,
    target_power: Option<f64>,
    known_input: Option<f64>,
    input_name: &str,
) -> Result<f64, PowerError> {
    match solve_for {
        SolveFor::Power => {
            if target_power.is_some() {
                return Err(PowerError::Spec(
                    "target_power must not be supplied when solve_for='power'".into(),
                ));
            }
            known_input.ok_or_else(|| PowerError::Spec(format!("{input_name} is required")))
        }
        SolveFor::SampleSize => {
            if known_input.is_some() {
                Ok(target_power.unwrap())
            } else {
                Err(PowerError::Spec(format!(
                    "{input_name} is required when solve_for='sample_size'"
                )))
            }
        }
        SolveFor::Mde => {
            if known_input.is_some() {
                return Err(PowerError::Spec(
                    "effect input must not be supplied when solve_for='mde'".into(),
                ));
            }
            target_power.ok_or_else(|| {
                PowerError::Spec("target_power is required when solve_for='mde'".into())
            })
        }
    }
}

pub fn normal_quantile(p: f64) -> Result<f64, PowerError> {
    Normal::new(0.0, 1.0)
        .map(|d| d.inverse_cdf(p))
        .map_err(|e| PowerError::Numerical(e.to_string()))
}

pub fn normal_cdf(x: f64) -> Result<f64, PowerError> {
    Normal::new(0.0, 1.0)
        .map(|d| d.cdf(x))
        .map_err(|e| PowerError::Numerical(e.to_string()))
}

pub fn t_critical(df: f64, alpha: f64, alternative: Alternative) -> Result<f64, PowerError> {
    if df <= 0.0 || !df.is_finite() {
        return Err(PowerError::Input(
            "degrees of freedom must be positive".into(),
        ));
    }
    let dist = StudentsT::new(0.0, 1.0, df).map_err(|e| PowerError::Numerical(e.to_string()))?;
    let p = match alternative {
        Alternative::TwoSided => 1.0 - alpha / 2.0,
        Alternative::Greater => 1.0 - alpha,
        Alternative::Less => alpha,
    };
    Ok(dist.inverse_cdf(p))
}

pub fn f_critical(df1: f64, df2: f64, alpha: f64) -> Result<f64, PowerError> {
    if df1 <= 0.0 || df2 <= 0.0 {
        return Err(PowerError::Input(
            "F degrees of freedom must be positive".into(),
        ));
    }
    FisherSnedecor::new(df1, df2)
        .map(|d| d.inverse_cdf(1.0 - alpha))
        .map_err(|e| PowerError::Numerical(e.to_string()))
}

pub fn chisq_critical(df: f64, alpha: f64) -> Result<f64, PowerError> {
    if df <= 0.0 {
        return Err(PowerError::Input(
            "chi-square degrees of freedom must be positive".into(),
        ));
    }
    ChiSquared::new(df)
        .map(|d| d.inverse_cdf(1.0 - alpha))
        .map_err(|e| PowerError::Numerical(e.to_string()))
}

pub fn ln_beta(a: f64, b: f64) -> Result<f64, PowerError> {
    if a <= 0.0 || b <= 0.0 {
        return Err(PowerError::Numerical(
            "beta parameters must be positive".into(),
        ));
    }
    Ok(ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b))
}

pub fn regularized_beta(x: f64, a: f64, b: f64) -> Result<f64, PowerError> {
    if !(0.0..=1.0).contains(&x) {
        return Err(PowerError::Numerical(
            "incomplete-beta argument outside [0,1]".into(),
        ));
    }
    if x == 0.0 {
        return Ok(0.0);
    }
    if x == 1.0 {
        return Ok(1.0);
    }
    Beta::new(a, b)
        .map(|dist| dist.cdf(x))
        .map_err(|e| PowerError::Numerical(e.to_string()))
}

pub fn noncentral_t_cdf(x: f64, df: f64, ncp: f64) -> Result<f64, PowerError> {
    if df <= 0.0 || !ncp.is_finite() {
        return Err(PowerError::Numerical(
            "invalid noncentral-t parameters".into(),
        ));
    }
    if x.is_nan() {
        return Err(PowerError::Numerical("noncentral-t argument is NaN".into()));
    }
    if x == 0.0 {
        return normal_cdf(-ncp);
    }
    let (abs_x, mu) = if x > 0.0 { (x, ncp) } else { (-x, -ncp) };
    let chi = ChiSquared::new(df).map_err(|e| PowerError::Numerical(e.to_string()))?;
    let mut cdf = 0.0;
    for (node, weight) in gauss_legendre_nodes() {
        let p = (node + 1.0) / 2.0;
        let v = chi.inverse_cdf(p);
        if v > 0.0 && v.is_finite() {
            cdf += weight * 0.5 * normal_cdf(abs_x * (v / df).sqrt() - mu)?;
        }
    }
    Ok(if x > 0.0 { cdf } else { 1.0 - cdf }.clamp(0.0, 1.0))
}

fn gauss_legendre_nodes() -> &'static Vec<(f64, f64)> {
    static NODES: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    NODES.get_or_init(|| {
        let n = 256;
        let mut nodes = Vec::with_capacity(n);
        for i in 1..=n {
            let mut x = (std::f64::consts::PI * (i as f64 - 0.25) / (n as f64 + 0.5)).cos();
            for _ in 0..100 {
                let (mut p0, mut p1) = (1.0, x);
                for j in 2..=n {
                    let jp = j as f64;
                    let p2 = ((2.0 * jp - 1.0) * x * p1 - (jp - 1.0) * p0) / jp;
                    p0 = p1;
                    p1 = p2;
                }
                let dp = n as f64 * (x * p1 - p0) / (x * x - 1.0);
                let dx = p1 / dp;
                x -= dx;
                if dx.abs() < 1e-14 {
                    break;
                }
            }
            let (mut p0, mut p1) = (1.0, x);
            for j in 2..=n {
                let jp = j as f64;
                let p2 = ((2.0 * jp - 1.0) * x * p1 - (jp - 1.0) * p0) / jp;
                p0 = p1;
                p1 = p2;
            }
            let dp = n as f64 * (x * p1 - p0) / (x * x - 1.0);
            nodes.push((x, 2.0 / ((1.0 - x * x) * dp * dp)));
        }
        nodes
    })
}

pub fn power_noncentral_t(
    df: f64,
    ncp: f64,
    critical: f64,
    alternative: Alternative,
) -> Result<f64, PowerError> {
    let upper = 1.0 - noncentral_t_cdf(critical, df, ncp)?;
    Ok(match alternative {
        Alternative::Greater => upper,
        Alternative::Less => noncentral_t_cdf(critical, df, ncp)?,
        Alternative::TwoSided => upper + noncentral_t_cdf(-critical, df, ncp)?,
    }
    .clamp(0.0, 1.0))
}

pub fn noncentral_f_cdf(x: f64, df1: f64, df2: f64, ncp: f64) -> Result<f64, PowerError> {
    if x <= 0.0 {
        return Ok(0.0);
    }
    if df1 <= 0.0 || df2 <= 0.0 || !(ncp >= 0.0) {
        return Err(PowerError::Numerical(
            "invalid noncentral-F parameters".into(),
        ));
    }
    let y = (df1 * x / df2) / (1.0 + df1 * x / df2);
    let lambda = ncp / 2.0;
    let mut cdf = 0.0;
    let mut term = (-lambda).exp();
    for k in 0..300 {
        if k > 0 {
            term *= lambda / k as f64;
        }
        let beta = regularized_beta(y, df1 / 2.0 + k as f64, df2 / 2.0)?;
        cdf += term * beta;
        if term < 1e-16 && k as f64 > lambda + 20.0 {
            return Ok(cdf.clamp(0.0, 1.0));
        }
    }
    Err(PowerError::Numerical(
        "noncentral-F series did not converge".into(),
    ))
}

pub fn noncentral_chisq_cdf(x: f64, df: f64, ncp: f64) -> Result<f64, PowerError> {
    if x <= 0.0 {
        return Ok(0.0);
    }
    if df <= 0.0 || !(ncp >= 0.0) {
        return Err(PowerError::Numerical(
            "invalid noncentral chi-square parameters".into(),
        ));
    }
    let lambda = ncp / 2.0;
    let mut cdf = 0.0;
    let mut term = (-lambda).exp();
    for k in 0..300 {
        if k > 0 {
            term *= lambda / k as f64;
        }
        let central = ChiSquared::new(df + 2.0 * k as f64)
            .map_err(|e| PowerError::Numerical(e.to_string()))?;
        cdf += term * central.cdf(x);
        if term < 1e-16 && k as f64 > lambda + 20.0 {
            return Ok(cdf.clamp(0.0, 1.0));
        }
    }
    Err(PowerError::Numerical(
        "noncentral chi-square series did not converge".into(),
    ))
}

pub fn find_root_increasing<F>(
    mut lo: f64,
    mut hi: f64,
    target: f64,
    f: F,
) -> Result<f64, PowerError>
where
    F: Fn(f64) -> Result<f64, PowerError>,
{
    let mut flo = f(lo)?;
    let fhi = f(hi)?;
    if flo > target || fhi < target {
        return Err(PowerError::Solver);
    }
    if (flo - target).abs() < 1e-12 {
        return Ok(lo);
    }
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        if !mid.is_finite() {
            return Err(PowerError::Solver);
        }
        let value = f(mid)?;
        if value < target {
            lo = mid;
            flo = value;
        } else {
            hi = mid;
        }
        if (hi - lo).max((flo - target).abs()) < 1e-10 {
            return Ok((lo + hi) / 2.0);
        }
    }
    Ok((lo + hi) / 2.0)
}

pub fn integer_root_up<F>(min: u64, max: u64, target: f64, f: F) -> Result<u64, PowerError>
where
    F: Fn(u64) -> Result<f64, PowerError>,
{
    if f(min)? >= target {
        return Ok(min);
    }
    if f(max)? < target {
        return Err(PowerError::Solver);
    }
    let (mut lo, mut hi) = (min, max);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if f(mid)? < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(hi)
}

pub fn integer_root_near<F>(min: u64, target: f64, initial: u64, f: F) -> Result<u64, PowerError>
where
    F: Fn(u64) -> Result<f64, PowerError>,
{
    let initial = initial.max(min);
    if f(initial)? >= target {
        let mut hi = initial;
        let mut step = 64u64;
        loop {
            let candidate = hi.saturating_sub(step).max(min);
            if candidate == hi {
                return Ok(hi);
            }
            if f(candidate)? < target {
                let mut lo = candidate;
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    if f(mid)? < target {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                return Ok(hi);
            }
            hi = candidate;
            step *= 2;
        }
    }

    let mut lo = initial;
    let mut step = 64u64;
    let mut hi = initial.saturating_add(step);
    while f(hi)? < target {
        lo = hi;
        step = step.saturating_mul(2);
        hi = hi.saturating_add(step);
        if hi >= 1_000_000 {
            if f(1_000_000)? < target {
                return Err(PowerError::Solver);
            }
            hi = 1_000_000;
            break;
        }
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if f(mid)? < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(hi)
}
