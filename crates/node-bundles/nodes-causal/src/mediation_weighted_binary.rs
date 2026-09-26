//! Survey-weighted causal mediation with a **binary outcome**.
//!
//! Wraps [`epi::mediation_weighted_binary`]: the mediator model stays WLS
//! (`M ~ X + C`) while the outcome model is a weighted logistic regression
//! (`Y ~ X + M [+ X:M] + C`, IRLS — the same estimating equations as
//! `svyglm(family = quasibinomial)`), so every effect in the output is on
//! the **log-odds scale** (`exp(effect)` = odds ratio). Intervals come from
//! the same design-aware (strata/PSU) bootstrap as `mediation_weighted`.
//!
//! Output (single row) — identical schema to `mediation_weighted`:
//!
//! | Column              | Type    | Description                          |
//! |---------------------|---------|--------------------------------------|
//! | `cde` / `_ci_*`     | Float64 | Controlled direct effect (log-odds) |
//! | `nde` / `_ci_*`     | Float64 | Natural direct effect (log-odds)     |
//! | `nie` / `_ci_*`     | Float64 | Natural indirect effect (log-odds)   |
//! | `te` / `_ci_*`      | Float64 | Total effect (log-odds)              |
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

use dag_core::arrow_util::ColumnError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};
use epi::bootstrap::BootstrapDesign;

use crate::mediation_weighted::{parse_ci_method, to_u64_codes};

#[derive(Debug, thiserror::Error)]
pub enum MediationWeightedBinaryError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for MediationWeightedBinaryError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for MediationWeightedBinaryError {
    fn node_type(&self) -> &str {
        "mediation_weighted_binary"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MediationWeightedBinaryNodeSpec {
    /// Exposure column name.
    pub exposure_column: String,
    /// Mediator column name (continuous).
    pub mediator_column: String,
    /// Outcome column name (binary 0/1).
    pub outcome_column: String,
    /// Optional confounder column names.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Sampling-weight column name (required, e.g. NHANES WTMEC2YR).
    pub weight_column: String,
    /// Stratum column name; enables stratified bootstrap.
    #[serde(default)]
    pub strata_column: Option<String>,
    /// PSU column name; requires strata — PSUs are resampled with
    /// replacement within each stratum.
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

#[derive(Clone)]
pub struct MediationWeightedBinaryNode {
    meta: NodePorts,
    exposure_column: String,
    mediator_column: String,
    outcome_column: String,
    covariates: Vec<String>,
    weight_column: String,
    strata_column: Option<String>,
    psu_column: Option<String>,
    interaction: bool,
    ci_method: epi::bootstrap::CiMethod,
    n_bootstrap: usize,
    seed: u64,
}

pub struct MediationWeightedBinaryNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for MediationWeightedBinaryNodeFactory {
    fn kind(&self) -> &'static str {
        "mediation_weighted_binary"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted causal mediation, binary outcome (logistic path model + bootstrap CI)."
    }
    fn doc(&self) -> &'static str {
        "Two-model causal mediation under sampling weights with a binary \
        outcome: mediator (M ~ X + C) fitted by WLS and outcome \
        (Y ~ X + M [+ X:M] + C) fitted by weighted logistic regression — \
        the svyglm(quasibinomial) path model. CDE/NDE/NIE/TE are decomposed \
        on the log-odds scale at the weighted covariate means; exp() gives \
        odds ratios. Bias-corrected bootstrap CIs from PSU-within-stratum \
        resampling when strata/PSU columns are given."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MediationWeightedBinaryNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MediationWeightedBinaryNodeSpec = serde_json::from_value(spec)?;
        let ci_method = parse_ci_method(&s.ci_method)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(Box::new(MediationWeightedBinaryNode {
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
impl DagNode for MediationWeightedBinaryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "mediation_weighted_binary"
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
        let input = inputs.first().ok_or(MediationWeightedBinaryError::Column(
            "no input connected".to_string(),
        ))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| MediationWeightedBinaryError::Collect(e.to_string()))?;

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

        // Complete-case filter across every column in play, plus the 0/1
        // outcome validation.
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
            if y_raw[i] != 0.0 && y_raw[i] != 1.0 {
                return Err(MediationWeightedBinaryError::Column(format!(
                    "outcome column '{}' must be binary 0/1, found {} at row {i}",
                    self.outcome_column, y_raw[i]
                ))
                .into());
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
            return Err(
                MediationWeightedBinaryError::Column("no complete-case rows".to_string()).into(),
            );
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
            .map_err(|e| MediationWeightedBinaryError::Fit(e.to_string()))?;
        let opts = epi::mediation_weighted::MediationWeightedOptions {
            interaction: self.interaction,
            ci_method: self.ci_method,
            n_bootstrap: self.n_bootstrap,
            seed: self.seed,
            ..Default::default()
        };
        let result = epi::mediation_weighted::mediation_weighted_binary(
            &x,
            &m,
            &y,
            &cov_slices,
            &design,
            &opts,
        )
        .map_err(|e| MediationWeightedBinaryError::Fit(e.to_string()))?;

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
        .expect("mediation_weighted_binary schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| MediationWeightedBinaryError::ReadBatch(e.to_string()))?;
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

    /// Logistic-generating NHANES-shaped system with strata + PSU codes.
    fn input_batch() -> RecordBatch {
        let n = 160;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let c1: Vec<f64> = (0..n).map(|i| (i % 13) as f64 / 13.0 - 0.5).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| 0.5 + 0.8 * x[i] + 0.4 * c1[i] + ((i % 7) as f64 / 7.0 - 0.5))
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| {
                let eta = -1.2 + 0.6 * x[i] + 0.5 * m[i] + 0.3 * c1[i];
                let u = (i as f64 * 0.6180339887498949).fract();
                if u < 1.0 / (1.0 + (-eta).exp()) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let wt: Vec<f64> = (0..n).map(|i| 5000.0 + 100.0 * (i % 5) as f64).collect();
        let st: Vec<f64> = (0..n).map(|i| 1.0 + (i % 4) as f64).collect();
        // 2 PSUs per stratum.
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
        let mut node = MediationWeightedBinaryNodeFactory {}
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
    async fn psu_bootstrap_smoke_binary() {
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
        assert_eq!(cell(&rows, "n_obs") as usize, 160);
        assert_eq!(cell(&rows, "n_bootstrap") as usize, 50);
        // Log-odds scale effects.
        let nie = cell(&rows, "nie");
        let cde = cell(&rows, "cde");
        assert!(nie > 0.0 && nie < 1.5, "nie {nie}");
        assert!(cde > -0.5 && cde < 1.5, "cde {cde}");
        assert!((cell(&rows, "te") - cde - nie).abs() < 1e-9);
        assert!(cell(&rows, "nie_ci_lower") < cell(&rows, "nie_ci_upper"));
        assert_eq!(cell(&rows, "nde"), cde);
    }

    #[tokio::test]
    async fn non_binary_outcome_rejected() {
        let mut node = MediationWeightedBinaryNodeFactory {}
            .build(
                serde_json::json!({
                    "exposure_column": "x",
                    "mediator_column": "m",
                    "outcome_column": "y",
                    "weight_column": "wt",
                }),
                node_ctx(),
            )
            .unwrap();
        let n = 12;
        let mut y = vec![1.0; n];
        y[3] = 0.5; // invalid outcome value
        let batch = make_batch(vec![
            ("x", vec![1.0; n]),
            ("m", vec![1.0; n]),
            ("y", y),
            ("wt", vec![1.0; n]),
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

    #[tokio::test]
    async fn bad_ci_method_is_rejected_at_build() {
        let err = MediationWeightedBinaryNodeFactory {}.build(
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
}
