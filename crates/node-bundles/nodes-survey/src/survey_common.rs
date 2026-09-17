//! Shared infrastructure for all survey-package DAG nodes.
//!
//! This module provides:
//! - [`SurveyDesignSpec`] — the common design specification embedded in every
//!   survey analysis node (mirrors R's `svydesign(ids, strata, probs, weights,
//!   fpc, nest, pps)`).
//! - [`SurveyStubNode`] — a generic stub node used until per-node Rust
//!   implementations are written. Returns a "not yet implemented" error from
//!   `execute`.
//!
//! See `reference/survey/` for the R package source (v4.5, Thomas Lumley).
//! The porting plan is documented in memory `[[survey-crate-scope]]`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::NodeCtx,
};

// =====================================================================
// Common design specification
// =====================================================================

/// Survey design specification shared by every survey analysis node.
///
/// Mirrors the arguments of R's `svydesign(ids, strata, probs, weights, fpc,
/// nest, pps, variance)`. Every analysis node embeds this as `design` in its
/// spec, builds the design object internally, then runs the analysis — so
/// designs are **self-contained per node**, not passed between nodes.
///
/// For calibration chaining (`calibrate` → `svymean`), the calibration node
/// outputs data with an updated weight column, and the downstream node
/// references that column in `weights`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SurveyDesignSpec {
    /// Cluster / PSU identifier column name(s). Use `[]` (R `~1`) for
    /// independent samples (no clustering). Multiple columns indicate
    /// multistage sampling, e.g. `["county", "school"]`.
    #[serde(default)]
    pub ids: Vec<String>,

    /// Stratification variable column name(s). Empty = no stratification.
    #[serde(default)]
    pub strata: Vec<String>,

    /// Sampling probability column name(s) (multi-stage: one column per stage).
    /// Mutually exclusive with `weights`.
    #[serde(default)]
    pub probs: Vec<String>,

    /// Sampling weight column name (single column). Mutually exclusive with
    /// `probs`.
    #[serde(default)]
    pub weights: Option<String>,

    /// Finite population correction column name(s).
    #[serde(default)]
    pub fpc: Vec<String>,

    /// If `true`, force clusters to be nested in strata (R `nest = TRUE`).
    #[serde(default)]
    pub nest: bool,

    /// PPS sampling method: `"none"` (default), `"brewer"`, `"overton"`.
    #[serde(default = "default_pps")]
    pub pps: String,

    /// Variance estimator: `"HT"` (Horvitz-Thompson, default) or `"YG"`
    /// (Yates-Grundy, PPS only).
    #[serde(default = "default_variance")]
    pub variance: String,

    /// Lonely-PSU handling policy: `"remove"`, `"adjust"`, `"fail"`,
    /// `"certainty"`, or `"average"`. Maps to R's
    /// `options(survey.lonely.psu = ...)`.
    #[serde(default)]
    pub lonely_psu: Option<String>,
}

fn default_pps() -> String {
    "none".to_string()
}

fn default_variance() -> String {
    "HT".to_string()
}

// =====================================================================
// Stub node — used until per-node Rust implementations are written
// =====================================================================

/// A generic stub node for survey analysis nodes.
///
/// The `execute` method returns a "not yet implemented" error. Each node kind
/// is differentiated only by its `kind` string and port layout.
///
/// When implementing a node's Rust execution path, replace this with a
/// dedicated struct that stores the parsed spec and implements real logic
/// in `execute`.
#[derive(Clone)]
pub struct SurveyStubNode {
    meta: NodePorts,
    kind: &'static str,
}

impl SurveyStubNode {
    /// Create a stub node with a single input port and single output port.
    pub fn new(kind: &'static str) -> Self {
        Self {
            meta: one_in_one_out(),
            kind,
        }
    }

    /// Create a stub node with a custom port layout.
    pub fn with_ports(kind: &'static str, meta: NodePorts) -> Self {
        Self { meta, kind }
    }
}

#[async_trait]
impl DagNode for SurveyStubNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        self.kind
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        _ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        Err(DagError::NodeError {
            node_type: self.kind.to_string(),
            msg: format!("Rust execution not yet implemented for '{}'", self.kind),
        })
    }
}

// =====================================================================
// Port layout helpers
// =====================================================================

/// One untyped input port + one untyped output port (the standard layout for
/// most survey analysis nodes).
pub fn one_in_one_out() -> NodePorts {
    NodePorts::new().add_input_port(None).add_output_port(None)
}

/// One untyped input port + two untyped output ports.
pub fn one_in_two_out() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(None)
        .add_output_port(None)
}

// =====================================================================
// Generic spec-validation build helper
// =====================================================================

/// Deserialize a spec JSON into a typed config, returning a stub node.
///
/// Used by factory `build` methods to validate the spec during DAG
/// construction so errors surface early while execution remains deferred.
pub fn build_stub<S>(
    kind: &'static str,
    spec: serde_json::Value,
) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error>
where
    S: serde::de::DeserializeOwned,
{
    let _parsed: S = serde_json::from_value(spec)?;
    Ok(Box::new(SurveyStubNode::new(kind)))
}

// =====================================================================
// Arrow → survey::SurveyDesign builder
// =====================================================================

use arrow_array::{Array, RecordBatch, StringArray, StringViewArray};

use survey::{LonelyPsu, SurveyDesign, SurveyDesignBuilder};

/// Error from building a survey design or running an analysis.
#[derive(Debug)]
pub struct SurveyNodeError(pub String);

impl std::fmt::Display for SurveyNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SurveyNodeError {}

impl ::dag_core::dag::NodeError for SurveyNodeError {
    fn node_type(&self) -> &str {
        "survey"
    }
}

/// Public wrapper for [`extract_string_column`] — used by svyby to extract
/// grouping variables.
pub fn extract_string_column_pub(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<String>, SurveyNodeError> {
    extract_string_column(batches, name)
}

/// Extract a categorical column from Arrow batches as `Vec<String>`,
/// accepting Utf8, Utf8View, and integer types (which are common for
/// stratum/cluster identifiers).
fn extract_string_column(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<String>, SurveyNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema())
        .ok_or_else(|| SurveyNodeError("empty input".into()))?;
    let idx = schema
        .index_of(name)
        .map_err(|_| SurveyNodeError(format!("missing column '{name}'")))?;

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        // Try string types first.
        if let Some(a) = col.as_any().downcast_ref::<StringArray>() {
            for i in 0..a.len() {
                values.push(if a.is_null(i) {
                    String::new()
                } else {
                    a.value(i).to_string()
                });
            }
        } else if let Some(a) = col.as_any().downcast_ref::<StringViewArray>() {
            for i in 0..a.len() {
                values.push(if a.is_null(i) {
                    String::new()
                } else {
                    a.value(i).to_string()
                });
            }
        } else {
            // Try integer types — convert to string (like R's factor conversion).
            use arrow_array::*;
            macro_rules! int_to_string {
                ($t:ty) => {
                    if let Some(a) = col.as_any().downcast_ref::<$t>() {
                        for v in a.iter() {
                            values.push(v.map(|x| x.to_string()).unwrap_or_default());
                        }
                        true
                    } else {
                        false
                    }
                };
            }
            if int_to_string!(Int8Array)
                || int_to_string!(Int16Array)
                || int_to_string!(Int32Array)
                || int_to_string!(Int64Array)
                || int_to_string!(UInt8Array)
                || int_to_string!(UInt16Array)
                || int_to_string!(UInt32Array)
                || int_to_string!(UInt64Array)
            {
                // done
            } else {
                return Err(SurveyNodeError(format!(
                    "column '{name}' is not a string or integer type"
                )));
            }
        }
    }
    Ok(values)
}

/// Extract a numeric (f64) column from Arrow batches.
fn extract_f64_column(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, SurveyNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema())
        .ok_or_else(|| SurveyNodeError("empty input".into()))?;
    let idx = schema
        .index_of(name)
        .map_err(|_| SurveyNodeError(format!("missing column '{name}'")))?;

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        use arrow_array::*;
        macro_rules! extract {
            ($t:ty) => {
                if let Some(a) = col.as_any().downcast_ref::<$t>() {
                    for v in a.iter() {
                        values.push(v.map(|x| x as f64).unwrap_or(f64::NAN));
                    }
                    true
                } else {
                    false
                }
            };
        }
        if extract!(Int8Array)
            || extract!(Int16Array)
            || extract!(Int32Array)
            || extract!(Int64Array)
            || extract!(UInt8Array)
            || extract!(UInt16Array)
            || extract!(UInt32Array)
            || extract!(UInt64Array)
            || extract!(Float32Array)
            || extract!(Float64Array)
        {
            // done
        } else {
            return Err(SurveyNodeError(format!("column '{name}' is not numeric")));
        }
    }
    Ok(values)
}

/// Build a `survey::SurveyDesign` from Arrow batches + a [`SurveyDesignSpec`].
///
/// Extracts strata, cluster, weights/probs, and FPC columns, then constructs
/// the design object. Assumes single-stage (first column of ids/strata/probs).
pub fn build_survey_design(
    spec: &SurveyDesignSpec,
    batches: &[RecordBatch],
) -> Result<SurveyDesign, SurveyNodeError> {
    // Strata: use first stratum column or "1" for all obs.
    let n = batches.iter().map(|b| b.num_rows()).sum::<usize>();
    let strata = if spec.strata.is_empty() {
        vec!["1".to_string(); n]
    } else {
        extract_string_column(batches, &spec.strata[0])?
    };

    // Cluster: use first id column or unique ids.
    let cluster = if spec.ids.is_empty() {
        (0..n).map(|i| i.to_string()).collect::<Vec<_>>()
    } else {
        // If nest=true, combine strata:cluster (R's interaction).
        let raw = extract_string_column(batches, &spec.ids[0])?;
        if spec.nest {
            strata
                .iter()
                .zip(raw.iter())
                .map(|(s, c)| format!("{s}.{c}"))
                .collect()
        } else {
            raw
        }
    };

    // Weights or probs.
    let mut builder = SurveyDesignBuilder::new()
        .strata(strata)
        .cluster(cluster)
        .lonely_psu(
            spec.lonely_psu
                .as_deref()
                .map(LonelyPsu::from_str)
                .unwrap_or_default(),
        );

    if let Some(w) = &spec.weights {
        let weights = extract_f64_column(batches, w)?;
        builder = builder.weights(weights);
    } else if !spec.probs.is_empty() {
        let probs = extract_f64_column(batches, &spec.probs[0])?;
        builder = builder.probs(probs);
    }

    // FPC: population sizes per stratum.
    if !spec.fpc.is_empty() {
        let fpc_vals = extract_f64_column(batches, &spec.fpc[0])?;
        let mut popsize = std::collections::HashMap::new();
        // Each stratum should have a constant popsize.
        let strata_vals = if spec.strata.is_empty() {
            vec!["1".to_string(); n]
        } else {
            extract_string_column(batches, &spec.strata[0])?
        };
        for (s, &f) in strata_vals.iter().zip(fpc_vals.iter()) {
            popsize.insert(s.clone(), f);
        }
        builder = builder.fpc_popsize(popsize);
    }

    builder.build().map_err(|e| SurveyNodeError(e.to_string()))
}

/// Extract multiple numeric variable columns from Arrow batches.
pub fn extract_variables(
    batches: &[RecordBatch],
    names: &[String],
) -> Result<Vec<Vec<f64>>, SurveyNodeError> {
    names
        .iter()
        .map(|n| extract_f64_column(batches, n))
        .collect()
}

/// Build the standard output RecordBatch for a survey describe node.
///
/// Columns: `variable` (Utf8), `estimate` (Float64), `se` (Float64),
/// `ci_lower` (Float64), `ci_upper` (Float64), `df` (Float64).
pub fn build_describe_output_batch(
    variables: &[String],
    stat: &survey::SurveyStat,
) -> Result<arrow_array::RecordBatch, DagError> {
    use arrow_array::{Float64Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    let p = variables.len();
    let se = stat.se();
    // Normal approximation for 95% CI (good enough for df > 30).
    let z = 1.959963984540054_f64;
    let ci_lower: Vec<f64> = stat
        .estimate
        .iter()
        .zip(&se)
        .map(|(m, s)| m - z * s)
        .collect();
    let ci_upper: Vec<f64> = stat
        .estimate
        .iter()
        .zip(&se)
        .map(|(m, s)| m + z * s)
        .collect();
    let df_col: Vec<f64> = vec![stat.df as f64; p];

    arrow_array::RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("variable", DataType::Utf8, false),
            Field::new("estimate", DataType::Float64, false),
            Field::new("se", DataType::Float64, false),
            Field::new("ci_lower", DataType::Float64, false),
            Field::new("ci_upper", DataType::Float64, false),
            Field::new("df", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(variables.to_vec())),
            Arc::new(Float64Array::from(stat.estimate.clone())),
            Arc::new(Float64Array::from(se)),
            Arc::new(Float64Array::from(ci_lower)),
            Arc::new(Float64Array::from(ci_upper)),
            Arc::new(Float64Array::from(df_col)),
        ],
    )
    .map_err(|e| DagError::NodeError {
        node_type: "survey".into(),
        msg: format!("failed to build output batch: {e}"),
    })
}

/// Shared execute path for survey describe nodes (svymean/svytotal/svyvar).
///
/// Collects Arrow batches, builds the design, extracts variables, runs the
/// analysis function, and returns the output DataFrame.
pub async fn execute_describe(
    kind: &str,
    variables: &[String],
    design_spec: &SurveyDesignSpec,
    inputs: &[NodeInput],
    node_ctx: &NodeCtx,
    analysis_fn: fn(&[Vec<f64>], &survey::SurveyDesign, bool) -> survey::Result<survey::SurveyStat>,
    na_rm: bool,
) -> Result<PortOutputs, DagError> {
    let input = inputs.first().ok_or_else(|| DagError::NodeError {
        node_type: kind.into(),
        msg: "no input data".into(),
    })?;
    let batches = input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("collect failed: {e}"),
        })?;

    let design = build_survey_design(design_spec, &batches)?;
    let x = extract_variables(&batches, variables)?;

    let stat = analysis_fn(&x, &design, na_rm).map_err(|e| DagError::NodeError {
        node_type: kind.into(),
        msg: e.to_string(),
    })?;

    let batch = build_describe_output_batch(variables, &stat)?;
    let ctx = node_ctx.session();
    let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: kind.into(),
        msg: format!("read_batch failed: {e}"),
    })?;

    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

/// Shared execute path for survey model nodes (svyglm/svycoxph/svyolr/etc).
///
/// Collects batches, builds design, extracts y + x, calls analysis_fn,
/// builds output: term, estimate, se, t_stat, p_value, df.
pub async fn execute_model(
    kind: &str,
    response_col: &str,
    predictor_cols: &[String],
    design_spec: &SurveyDesignSpec,
    inputs: &[NodeInput],
    node_ctx: &NodeCtx,
    analysis_fn: ModelAnalysisFn,
    term_names: Vec<String>,
) -> Result<PortOutputs, DagError>
where
{
    let input = inputs.first().ok_or_else(|| DagError::NodeError {
        node_type: kind.into(),
        msg: "no input data".into(),
    })?;
    let batches = input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: kind.into(),
            msg: format!("collect failed: {e}"),
        })?;

    let design = build_survey_design(design_spec, &batches)?;
    let y = extract_variables(&batches, &[response_col.to_string()])?;
    let x = extract_variables(&batches, predictor_cols)?;

    let result = analysis_fn(&y[0], &x, &design).map_err(|e| DagError::NodeError {
        node_type: kind.into(),
        msg: e.to_string(),
    })?;

    let p = result.coef.len();
    let se = result.se.clone();
    let t_stats: Vec<f64> = result
        .coef
        .iter()
        .zip(&se)
        .map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 })
        .collect();
    let p_values: Vec<f64> = t_stats
        .iter()
        .map(|&t| 2.0 * (1.0 - normal_cdf(t.abs())))
        .collect();
    let df_col = vec![result.df as f64; p];

    let batch =
        build_model_output_batch(&term_names, &result.coef, &se, &t_stats, &p_values, &df_col)?;
    let ctx = node_ctx.session();
    let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: kind.into(),
        msg: format!("read_batch failed: {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

/// Function pointer for model analysis (y, x, design → coefficients + cov).
pub struct ModelResult {
    pub coef: Vec<f64>,
    pub se: Vec<f64>,
    pub df: usize,
}

pub type ModelAnalysisFn = Box<
    dyn Fn(&[f64], &[Vec<f64>], &survey::SurveyDesign) -> survey::Result<ModelResult> + Send + Sync,
>;

/// Build the standard model output batch: term, estimate, se, t_stat, p_value, df.
pub fn build_model_output_batch(
    terms: &[String],
    coef: &[f64],
    se: &[f64],
    t_stats: &[f64],
    p_values: &[f64],
    df: &[f64],
) -> Result<arrow_array::RecordBatch, DagError> {
    use arrow_array::{Float64Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    arrow_array::RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("term", DataType::Utf8, false),
            Field::new("estimate", DataType::Float64, false),
            Field::new("std_error", DataType::Float64, false),
            Field::new("t_stat", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("df", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(terms.to_vec())),
            Arc::new(Float64Array::from(coef.to_vec())),
            Arc::new(Float64Array::from(se.to_vec())),
            Arc::new(Float64Array::from(t_stats.to_vec())),
            Arc::new(Float64Array::from(p_values.to_vec())),
            Arc::new(Float64Array::from(df.to_vec())),
        ],
    )
    .map_err(|e| DagError::NodeError {
        node_type: "survey".into(),
        msg: format!("failed to build model output: {e}"),
    })
}

/// Standard normal CDF.
pub fn normal_cdf(x: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, Normal};
    Normal::new(0.0, 1.0).unwrap().cdf(x)
}

/// Two-sided p-value from Student's t with survey design degrees of freedom.
pub fn student_t_two_sided_p(t: f64, df: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, StudentsT};

    if !t.is_finite() || !df.is_finite() || df <= 0.0 {
        return f64::NAN;
    }
    StudentsT::new(0.0, 1.0, df)
        .map(|dist| 2.0 * dist.cdf(-t.abs()))
        .unwrap_or(f64::NAN)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_spec_deserializes() {
        let json = serde_json::json!({
            "ids": ["psu"],
            "strata": ["stratum"],
            "weights": "wt",
            "nest": true
        });
        let spec: SurveyDesignSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.ids, vec!["psu"]);
        assert_eq!(spec.weights.as_deref(), Some("wt"));
        assert!(spec.nest);
        assert_eq!(spec.pps, "none");
        assert_eq!(spec.variance, "HT");
    }

    #[test]
    fn design_spec_minimal() {
        let json = serde_json::json!({"ids": ["id"]});
        let spec: SurveyDesignSpec = serde_json::from_value(json).unwrap();
        assert!(spec.strata.is_empty());
        assert!(spec.weights.is_none());
        assert!(!spec.nest);
    }

    #[test]
    fn student_t_two_sided_p_matches_reference() {
        // qt(-0.414, df=7): two-sided p is approximately 0.691.
        let p = student_t_two_sided_p(-0.414, 7.0);
        assert!((p - 0.691).abs() < 0.001, "p={p}");
    }
}
