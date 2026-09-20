//! Survey-weighted causal mediation node.
//!
//! Wraps [`epi::mediation_weighted`]. Two-model VanderWeele decomposition
//! fitted by WLS under sampling weights, with design-aware (strata/PSU)
//! bias-corrected bootstrap CIs — the Figure 4A–C estimator of Li et al.
//! 2026.
//!
//! Output (single row):
//!
//! | Column              | Type    | Description                          |
//! |---------------------|---------|--------------------------------------|
//! | `cde` / `_ci_*`     | Float64 | Controlled direct effect + 95% CI    |
//! | `nde` / `_ci_*`     | Float64 | Natural direct effect + 95% CI       |
//! | `nie` / `_ci_*`     | Float64 | Natural indirect effect + 95% CI     |
//! | `te` / `_ci_*`      | Float64 | Total effect + 95% CI                |
//! | `prop_mediated` ... | Float64 | Proportion mediated + 95% CI         |
//! | `m_under_control`   | Float64 | Mediator level under control         |
//! | `alpha_x`           | Float64 | Mediator model α₁ (X → M)            |
//! | `beta_x`            | Float64 | Outcome model β₁ (direct X → Y)      |
//! | `beta_m`            | Float64 | Outcome model β₂ (M → Y)             |
//! | `beta_xm`           | Float64 | Outcome model β₃ (X×M), 0 if absent  |
//! | `n_bootstrap`       | Int32   | Successful bootstrap replicates      |
//! | `n_obs`             | Int32   | Complete-case observations            |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::arrow_util::ColumnError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};
use epi::bootstrap::{BootstrapDesign, CiMethod};

#[derive(Debug, Error)]
pub enum MediationWeightedError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for MediationWeightedError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for MediationWeightedError {
    fn node_type(&self) -> &str {
        "mediation_weighted"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MediationWeightedNodeSpec {
    /// Exposure column name.
    pub exposure_column: String,
    /// Mediator column name.
    pub mediator_column: String,
    /// Outcome column name.
    pub outcome_column: String,
    /// Optional confounder column names.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Sampling-weight column name (required, e.g. NHANES WTMEC2YR).
    pub weight_column: String,
    /// Stratum column name (e.g. SDMVSTRA); enables stratified bootstrap.
    #[serde(default)]
    pub strata_column: Option<String>,
    /// PSU column name (e.g. SDMVPSU); requires strata — PSUs are resampled
    /// with replacement within each stratum.
    #[serde(default)]
    pub psu_column: Option<String>,
    /// Include X×M interaction in the outcome model (default false).
    #[serde(default)]
    pub interaction: bool,
    /// CI construction: "bc" (bias-corrected, default) or "percentile".
    #[serde(default = "default_ci_method")]
    pub ci_method: String,
    /// Bootstrap iterations (default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_ci_method() -> String {
    "bc".to_string()
}
fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

/// Parse the CI-method string shared by the weighted mediation nodes.
pub(crate) fn parse_ci_method(s: &str) -> Result<CiMethod, MediationWeightedError> {
    match s {
        "bc" => Ok(CiMethod::BiasCorrected),
        "percentile" => Ok(CiMethod::Percentile),
        other => Err(MediationWeightedError::Column(format!(
            "ci_method must be \"bc\" or \"percentile\", got \"{other}\""
        ))),
    }
}

/// Cast a filtered f64 stratum/PSU column to u64 codes.
pub(crate) fn to_u64_codes(v: &[f64], name: &str) -> Result<Vec<u64>, MediationWeightedError> {
    v.iter()
        .enumerate()
        .map(|(i, &x)| {
            if !x.is_finite() || x < 0.0 || x.fract() != 0.0 {
                Err(MediationWeightedError::Column(format!(
                    "column '{name}' row {i} is not a non-negative integer code (got {x})"
                )))
            } else {
                Ok(x as u64)
            }
        })
        .collect()
}

#[derive(Clone)]
pub struct MediationWeightedNode {
    meta: NodePorts,
    exposure_column: String,
    mediator_column: String,
    outcome_column: String,
    covariates: Vec<String>,
    weight_column: String,
    strata_column: Option<String>,
    psu_column: Option<String>,
    interaction: bool,
    ci_method: CiMethod,
    n_bootstrap: usize,
    seed: u64,
}

pub struct MediationWeightedNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for MediationWeightedNodeFactory {
    fn kind(&self) -> &'static str {
        "mediation_weighted"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted causal mediation (WLS + design-aware bootstrap CI)."
    }
    fn doc(&self) -> &'static str {
        "Two-model causal mediation under sampling weights: mediator \
        (M ~ X + C) and outcome (Y ~ X + M [+ X:M] + C) models fitted by \
        WLS, CDE/NDE/NIE/TE decomposed at the weighted covariate means. \
        Bias-corrected bootstrap CIs from row-level resampling or \
        PSU-within-stratum resampling when strata/PSU columns are given."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MediationWeightedNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MediationWeightedNodeSpec = serde_json::from_value(spec)?;
        let ci_method = parse_ci_method(&s.ci_method)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(Box::new(MediationWeightedNode {
            meta: port_layout(),
            exposure_column: s.exposure_column,
            mediator_column: s.mediator_column,
            outcome_column: s.outcome_column,
            covariates: s.covariates,
            weight_column: s.weight_column,
            strata_column: s.strata_column,
            psu_column: s.psu_column,
            interaction: s.interaction,
            ci_method,
            n_bootstrap: s.n_bootstrap,
            seed: s.seed,
        }))
    }
}

#[async_trait]
impl DagNode for MediationWeightedNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "mediation_weighted"
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
        let input = inputs.first().ok_or(MediationWeightedError::Column(
            "no input connected".to_string(),
        ))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| MediationWeightedError::Collect(e.to_string()))?;

        let x_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.exposure_column)?;
        let m_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.mediator_column)?;
        let y_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.outcome_column)?;
        let w_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.weight_column)?;
        let s_raw = match &self.strata_column {
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
            None => None,
        };
        let p_raw = match &self.psu_column {
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
            None => None,
        };
        let cov_raw: Vec<Vec<f64>> = self
            .covariates
            .iter()
            .map(|c| dag_core::arrow_util::extract_numeric_lenient(&batches, c))
            .collect::<Result<_, _>>()?;

        // Complete-case filter across every column in play.
        let n = y_raw.len();
        let mut x = Vec::with_capacity(n);
        let mut m = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut w = Vec::with_capacity(n);
        let mut cov_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.covariates.len()];
        for i in 0..n {
            if x_raw[i].is_nan()
                || m_raw[i].is_nan()
                || y_raw[i].is_nan()
                || w_raw[i].is_nan()
                || s_raw.as_ref().is_some_and(|s| s[i].is_nan())
                || p_raw.as_ref().is_some_and(|p| p[i].is_nan())
                || cov_raw.iter().any(|c| c[i].is_nan())
            {
                continue;
            }
            x.push(x_raw[i]);
            m.push(m_raw[i]);
            y.push(y_raw[i]);
            w.push(w_raw[i]);
            for (j, c) in cov_raw.iter().enumerate() {
                cov_filtered[j].push(c[i]);
            }
        }
        if x.is_empty() {
            return Err(MediationWeightedError::Column("no complete-case rows".to_string()).into());
        }

        let strata = match &s_raw {
            Some(s) => Some(to_u64_codes(
                s,
                self.strata_column.as_deref().unwrap_or(""),
            )?),
            None => None,
        };
        let psu = match &p_raw {
            Some(p) => Some(to_u64_codes(p, self.psu_column.as_deref().unwrap_or(""))?),
            None => None,
        };

        let cov_slices: Vec<&[f64]> = cov_filtered.iter().map(|v| v.as_slice()).collect();
        let design = BootstrapDesign::with_clusters(&w, strata.as_deref(), psu.as_deref())
            .map_err(|e| MediationWeightedError::Fit(e.to_string()))?;
        let opts = epi::mediation_weighted::MediationWeightedOptions {
            interaction: self.interaction,
            ci_method: self.ci_method,
            n_bootstrap: self.n_bootstrap,
            seed: self.seed,
            ..Default::default()
        };
        let result =
            epi::mediation_weighted::mediation_weighted(&x, &m, &y, &cov_slices, &design, &opts)
                .map_err(|e| MediationWeightedError::Fit(e.to_string()))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("cde", DataType::Float64, false),
                Field::new("cde_ci_lower", DataType::Float64, false),
                Field::new("cde_ci_upper", DataType::Float64, false),
                Field::new("nde", DataType::Float64, false),
                Field::new("nde_ci_lower", DataType::Float64, false),
                Field::new("nde_ci_upper", DataType::Float64, false),
                Field::new("nie", DataType::Float64, false),
                Field::new("nie_ci_lower", DataType::Float64, false),
                Field::new("nie_ci_upper", DataType::Float64, false),
                Field::new("te", DataType::Float64, false),
                Field::new("te_ci_lower", DataType::Float64, false),
                Field::new("te_ci_upper", DataType::Float64, false),
                Field::new("prop_mediated", DataType::Float64, true),
                Field::new("prop_mediated_ci_lower", DataType::Float64, false),
                Field::new("prop_mediated_ci_upper", DataType::Float64, false),
                Field::new("m_under_control", DataType::Float64, false),
                Field::new("alpha_x", DataType::Float64, false),
                Field::new("beta_x", DataType::Float64, false),
                Field::new("beta_m", DataType::Float64, false),
                Field::new("beta_xm", DataType::Float64, false),
                Field::new("n_bootstrap", DataType::Int32, false),
                Field::new("n_obs", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.cde])),
                Arc::new(Float64Array::from(vec![result.cde_ci.0])),
                Arc::new(Float64Array::from(vec![result.cde_ci.1])),
                Arc::new(Float64Array::from(vec![result.nde])),
                Arc::new(Float64Array::from(vec![result.nde_ci.0])),
                Arc::new(Float64Array::from(vec![result.nde_ci.1])),
                Arc::new(Float64Array::from(vec![result.nie])),
                Arc::new(Float64Array::from(vec![result.nie_ci.0])),
                Arc::new(Float64Array::from(vec![result.nie_ci.1])),
                Arc::new(Float64Array::from(vec![result.te])),
                Arc::new(Float64Array::from(vec![result.te_ci.0])),
                Arc::new(Float64Array::from(vec![result.te_ci.1])),
                Arc::new(Float64Array::from(vec![result.prop_mediated])),
                Arc::new(Float64Array::from(vec![result.pm_ci.0])),
                Arc::new(Float64Array::from(vec![result.pm_ci.1])),
                Arc::new(Float64Array::from(vec![result.m_under_control])),
                Arc::new(Float64Array::from(vec![result.alpha_x])),
                Arc::new(Float64Array::from(vec![result.beta_x])),
                Arc::new(Float64Array::from(vec![result.beta_m])),
                Arc::new(Float64Array::from(vec![result.beta_xm])),
                Arc::new(Int32Array::from(vec![result.n_bootstrap as i32])),
                Arc::new(Int32Array::from(vec![result.n_obs as i32])),
            ],
        )
        .expect("mediation_weighted schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| MediationWeightedError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn make_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect();
        let arrays: Vec<Arc<dyn arrow_array::Array>> = columns
            .iter()
            .map(|(_, vals)| {
                Arc::new(Float64Array::from(vals.clone())) as Arc<dyn arrow_array::Array>
            })
            .collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    /// Small NHANES-shaped weighted system with strata + PSU codes.
    fn input_batch() -> RecordBatch {
        let n = 80;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.8 * x[i] + 0.3 * ((i % 7) as f64 / 7.0))
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 5.0 + 0.4 * x[i] + 0.9 * m[i] + (i % 5) as f64 / 5.0)
            .collect();
        let c1: Vec<f64> = (0..n).map(|i| 40.0 + (i % 9) as f64).collect();
        let wt: Vec<f64> = (0..n).map(|i| 5000.0 + 100.0 * (i % 5) as f64).collect();
        let st: Vec<f64> = (0..n).map(|i| 1.0 + (i % 4) as f64).collect();
        // PSU must vary WITHIN each stratum (2 PSUs per stratum), otherwise
        // every stratum is a lonely-PSU stratum and the bootstrap degenerates.
        let ps: Vec<f64> = (0..n).map(|i| ((i / 4) % 2) as f64).collect();
        make_batch(vec![
            ("x", x),
            ("m", m),
            ("y", y),
            ("c1", c1),
            ("wt", wt),
            ("st", st),
            ("ps", ps),
        ])
    }

    async fn run_node(spec: serde_json::Value) -> Vec<RecordBatch> {
        let mut node = MediationWeightedNodeFactory {}
            .build(spec, node_ctx())
            .unwrap();
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(input_batch())
                .unwrap(),
        );
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        outs.dataframe(0).unwrap().clone().collect().await.unwrap()
    }

    fn cell(rows: &[RecordBatch], name: &str) -> f64 {
        let batch = &rows[0];
        let idx = batch.schema().index_of(name).unwrap();
        let col = batch.column(idx);
        if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
            a.value(0)
        } else {
            col.as_any().downcast_ref::<Int32Array>().unwrap().value(0) as f64
        }
    }

    #[tokio::test]
    async fn psu_bootstrap_smoke() {
        let rows = run_node(serde_json::json!({
            "exposure_column": "x",
            "mediator_column": "m",
            "outcome_column": "y",
            "covariates": ["c1"],
            "weight_column": "wt",
            "strata_column": "st",
            "psu_column": "ps",
            "n_bootstrap": 50,
        }))
        .await;
        assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
        assert_eq!(cell(&rows, "n_obs") as usize, 80);
        assert_eq!(cell(&rows, "n_bootstrap") as usize, 50);
        // Generating system: nie ≈ 0.9·0.8 = 0.72, cde ≈ 0.4.
        let nie = cell(&rows, "nie");
        assert!((0.4..1.0).contains(&nie), "nie {nie}");
        let cde = cell(&rows, "cde");
        assert!((0.1..0.7).contains(&cde), "cde {cde}");
        // te = cde + nie exactly, and CIs are ordered.
        assert!((cell(&rows, "te") - cde - nie).abs() < 1e-9);
        assert!(cell(&rows, "nie_ci_lower") < cell(&rows, "nie_ci_upper"));
        assert_eq!(cell(&rows, "nde"), cde);
    }

    #[tokio::test]
    async fn bad_ci_method_is_rejected_at_build() {
        let err = MediationWeightedNodeFactory {}.build(
            serde_json::json!({
                "exposure_column": "x",
                "mediator_column": "m",
                "outcome_column": "y",
                "weight_column": "wt",
                "ci_method": "bootstrap",
            }),
            node_ctx(),
        );
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn fractional_psu_code_is_rejected() {
        let mut node = MediationWeightedNodeFactory {}
            .build(
                serde_json::json!({
                    "exposure_column": "x",
                    "mediator_column": "m",
                    "outcome_column": "y",
                    "weight_column": "wt",
                    "psu_column": "ps",
                    "strata_column": "st",
                }),
                node_ctx(),
            )
            .unwrap();
        let n = 12;
        let batch = make_batch(vec![
            ("x", vec![1.0; n]),
            ("m", vec![1.0; n]),
            ("y", vec![1.0; n]),
            ("wt", vec![1.0; n]),
            ("st", vec![1.0; n]),
            ("ps", vec![1.5; n]), // fractional → cast error
        ]);
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
        );
        let err = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await;
        assert!(err.is_err());
    }
}
