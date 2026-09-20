//! Survey-weighted serial (two-mediator) mediation node.
//!
//! Wraps [`epi::mediation_serial`]. The X → M1 → M2 → Y chain of Li et
//! al. 2026 Figure 4D: three WLS models decompose the total effect into
//! the paths through M1 only, M2 only, and the serial path through both,
//! with design-aware bootstrap CIs.
//!
//! Output (single row):
//!
//! | Column                    | Type    | Description                        |
//! |---------------------------|---------|------------------------------------|
//! | `ie_m1` / `_ci_*`         | Float64 | Indirect via M1 only + 95% CI      |
//! | `ie_m2` / `_ci_*`         | Float64 | Indirect via M2 only + 95% CI      |
//! | `ie_serial` / `_ci_*`     | Float64 | Serial indirect (M1→M2) + 95% CI   |
//! | `total_indirect` / `_ci_*`| Float64 | Sum of the three indirects + CI    |
//! | `direct` / `_ci_*`        | Float64 | Direct effect c′·Δx + 95% CI       |
//! | `te` / `_ci_*`            | Float64 | Total effect + 95% CI              |
//! | `prop_mediated`           | Float64 | total_indirect / te                |
//! | `prop_serial`             | Float64 | ie_serial / te                     |
//! | `a1`/`a2`/`d21`/`b1`/`b2`/`c_prime` | Float64 | Path coefficients       |
//! | `n_bootstrap`             | Int32   | Successful bootstrap replicates    |
//! | `n_obs`                   | Int32   | Complete-case observations          |

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

use crate::mediation_weighted::{parse_ci_method, to_u64_codes};

#[derive(Debug, Error)]
pub enum MediationSerialError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for MediationSerialError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for MediationSerialError {
    fn node_type(&self) -> &str {
        "mediation_serial"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MediationSerialNodeSpec {
    /// Exposure column name.
    pub exposure_column: String,
    /// First mediator column name (upstream in the chain).
    pub mediator1_column: String,
    /// Second mediator column name (downstream in the chain).
    pub mediator2_column: String,
    /// Outcome column name.
    pub outcome_column: String,
    /// Optional confounder column names.
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Sampling-weight column name; `null` → unit weights.
    #[serde(default)]
    pub weight_column: Option<String>,
    /// Stratum column name (e.g. SDMVSTRA); enables stratified bootstrap.
    #[serde(default)]
    pub strata_column: Option<String>,
    /// PSU column name (e.g. SDMVPSU); requires strata.
    #[serde(default)]
    pub psu_column: Option<String>,
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
pub struct MediationSerialNode {
    meta: NodePorts,
    exposure_column: String,
    mediator1_column: String,
    mediator2_column: String,
    outcome_column: String,
    covariates: Vec<String>,
    weight_column: Option<String>,
    strata_column: Option<String>,
    psu_column: Option<String>,
    ci_method: CiMethod,
    n_bootstrap: usize,
    seed: u64,
}

pub struct MediationSerialNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for MediationSerialNodeFactory {
    fn kind(&self) -> &'static str {
        "mediation_serial"
    }
    fn desc(&self) -> &'static str {
        "Serial two-mediator mediation X → M1 → M2 → Y (weighted, bootstrap CI)."
    }
    fn doc(&self) -> &'static str {
        "Three-model serial mediation chain: M1 ~ X + C, M2 ~ X + M1 + C, \
        Y ~ X + M1 + M2 + C, all fitted by WLS under sampling weights. \
        Decomposes the total effect into ie_m1 (via M1 only), ie_m2 (via \
        M2 only) and ie_serial (through both mediators). Bias-corrected \
        bootstrap CIs from row-level or PSU-within-stratum resampling."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MediationSerialNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MediationSerialNodeSpec = serde_json::from_value(spec)?;
        let ci_method = parse_ci_method(&s.ci_method)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(Box::new(MediationSerialNode {
            meta: port_layout(),
            exposure_column: s.exposure_column,
            mediator1_column: s.mediator1_column,
            mediator2_column: s.mediator2_column,
            outcome_column: s.outcome_column,
            covariates: s.covariates,
            weight_column: s.weight_column,
            strata_column: s.strata_column,
            psu_column: s.psu_column,
            ci_method,
            n_bootstrap: s.n_bootstrap,
            seed: s.seed,
        }))
    }
}

#[async_trait]
impl DagNode for MediationSerialNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "mediation_serial"
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
        let input = inputs.first().ok_or(MediationSerialError::Column(
            "no input connected".to_string(),
        ))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| MediationSerialError::Collect(e.to_string()))?;

        let x_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.exposure_column)?;
        let m1_raw =
            dag_core::arrow_util::extract_numeric_lenient(&batches, &self.mediator1_column)?;
        let m2_raw =
            dag_core::arrow_util::extract_numeric_lenient(&batches, &self.mediator2_column)?;
        let y_raw = dag_core::arrow_util::extract_numeric_lenient(&batches, &self.outcome_column)?;
        let w_raw = match &self.weight_column {
            Some(c) => Some(dag_core::arrow_util::extract_numeric_lenient(&batches, c)?),
            None => None,
        };
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
        let mut m1 = Vec::with_capacity(n);
        let mut m2 = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut w: Vec<f64> = Vec::with_capacity(n);
        let mut s_kept: Vec<f64> = Vec::with_capacity(n);
        let mut p_kept: Vec<f64> = Vec::with_capacity(n);
        let mut cov_filtered: Vec<Vec<f64>> = vec![Vec::with_capacity(n); self.covariates.len()];
        for i in 0..n {
            if x_raw[i].is_nan()
                || m1_raw[i].is_nan()
                || m2_raw[i].is_nan()
                || y_raw[i].is_nan()
                || w_raw.as_ref().is_some_and(|w| w[i].is_nan())
                || s_raw.as_ref().is_some_and(|s| s[i].is_nan())
                || p_raw.as_ref().is_some_and(|p| p[i].is_nan())
                || cov_raw.iter().any(|c| c[i].is_nan())
            {
                continue;
            }
            x.push(x_raw[i]);
            m1.push(m1_raw[i]);
            m2.push(m2_raw[i]);
            y.push(y_raw[i]);
            if let Some(wr) = &w_raw {
                w.push(wr[i]);
            }
            if let Some(s) = &s_raw {
                s_kept.push(s[i]);
            }
            if let Some(p) = &p_raw {
                p_kept.push(p[i]);
            }
            for (j, c) in cov_raw.iter().enumerate() {
                cov_filtered[j].push(c[i]);
            }
        }
        if x.is_empty() {
            return Err(MediationSerialError::Column("no complete-case rows".to_string()).into());
        }

        if w.is_empty() {
            w = vec![1.0; x.len()];
        }
        let strata = if s_raw.is_some() {
            Some(to_u64_codes(
                &s_kept,
                self.strata_column.as_deref().unwrap_or(""),
            )?)
        } else {
            None
        };
        let psu = if p_raw.is_some() {
            Some(to_u64_codes(
                &p_kept,
                self.psu_column.as_deref().unwrap_or(""),
            )?)
        } else {
            None
        };

        let cov_slices: Vec<&[f64]> = cov_filtered.iter().map(|v| v.as_slice()).collect();
        let design = BootstrapDesign::with_clusters(&w, strata.as_deref(), psu.as_deref())
            .map_err(|e| MediationSerialError::Fit(e.to_string()))?;
        let opts = epi::mediation_serial::SerialMediationOptions {
            ci_method: self.ci_method,
            n_bootstrap: self.n_bootstrap,
            seed: self.seed,
            ..Default::default()
        };
        let result =
            epi::mediation_serial::mediation_serial(&x, &m1, &m2, &y, &cov_slices, &design, &opts)
                .map_err(|e| MediationSerialError::Fit(e.to_string()))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("ie_m1", DataType::Float64, false),
                Field::new("ie_m1_ci_lower", DataType::Float64, false),
                Field::new("ie_m1_ci_upper", DataType::Float64, false),
                Field::new("ie_m2", DataType::Float64, false),
                Field::new("ie_m2_ci_lower", DataType::Float64, false),
                Field::new("ie_m2_ci_upper", DataType::Float64, false),
                Field::new("ie_serial", DataType::Float64, false),
                Field::new("ie_serial_ci_lower", DataType::Float64, false),
                Field::new("ie_serial_ci_upper", DataType::Float64, false),
                Field::new("total_indirect", DataType::Float64, false),
                Field::new("total_indirect_ci_lower", DataType::Float64, false),
                Field::new("total_indirect_ci_upper", DataType::Float64, false),
                Field::new("direct", DataType::Float64, false),
                Field::new("direct_ci_lower", DataType::Float64, false),
                Field::new("direct_ci_upper", DataType::Float64, false),
                Field::new("te", DataType::Float64, false),
                Field::new("te_ci_lower", DataType::Float64, false),
                Field::new("te_ci_upper", DataType::Float64, false),
                Field::new("prop_mediated", DataType::Float64, true),
                Field::new("prop_serial", DataType::Float64, true),
                Field::new("a1", DataType::Float64, false),
                Field::new("a2", DataType::Float64, false),
                Field::new("d21", DataType::Float64, false),
                Field::new("b1", DataType::Float64, false),
                Field::new("b2", DataType::Float64, false),
                Field::new("c_prime", DataType::Float64, false),
                Field::new("n_bootstrap", DataType::Int32, false),
                Field::new("n_obs", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.ie_m1])),
                Arc::new(Float64Array::from(vec![result.ie_m1_ci.0])),
                Arc::new(Float64Array::from(vec![result.ie_m1_ci.1])),
                Arc::new(Float64Array::from(vec![result.ie_m2])),
                Arc::new(Float64Array::from(vec![result.ie_m2_ci.0])),
                Arc::new(Float64Array::from(vec![result.ie_m2_ci.1])),
                Arc::new(Float64Array::from(vec![result.ie_serial])),
                Arc::new(Float64Array::from(vec![result.ie_serial_ci.0])),
                Arc::new(Float64Array::from(vec![result.ie_serial_ci.1])),
                Arc::new(Float64Array::from(vec![result.total_indirect])),
                Arc::new(Float64Array::from(vec![result.total_indirect_ci.0])),
                Arc::new(Float64Array::from(vec![result.total_indirect_ci.1])),
                Arc::new(Float64Array::from(vec![result.direct])),
                Arc::new(Float64Array::from(vec![result.direct_ci.0])),
                Arc::new(Float64Array::from(vec![result.direct_ci.1])),
                Arc::new(Float64Array::from(vec![result.te])),
                Arc::new(Float64Array::from(vec![result.te_ci.0])),
                Arc::new(Float64Array::from(vec![result.te_ci.1])),
                Arc::new(Float64Array::from(vec![result.prop_mediated])),
                Arc::new(Float64Array::from(vec![result.prop_serial])),
                Arc::new(Float64Array::from(vec![result.a1])),
                Arc::new(Float64Array::from(vec![result.a2])),
                Arc::new(Float64Array::from(vec![result.d21])),
                Arc::new(Float64Array::from(vec![result.b1])),
                Arc::new(Float64Array::from(vec![result.b2])),
                Arc::new(Float64Array::from(vec![result.c_prime])),
                Arc::new(Int32Array::from(vec![result.n_bootstrap as i32])),
                Arc::new(Int32Array::from(vec![result.n_obs as i32])),
            ],
        )
        .expect("mediation_serial schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| MediationSerialError::ReadBatch(e.to_string()))?;
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

    /// Serial chain fixture: a₁=0.7, a₂=0.2, d₂₁=0.5, c′=0.3, b₁=0.6, b₂=0.8.
    fn input_batch() -> RecordBatch {
        let n = 80;
        let x: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let m1: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.7 * x[i] + 0.2 * ((i % 7) as f64 / 7.0))
            .collect();
        let m2: Vec<f64> = (0..n)
            .map(|i| 3.0 + 0.2 * x[i] + 0.5 * m1[i] + 0.15 * ((i % 5) as f64 / 5.0))
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 2.0 + 0.3 * x[i] + 0.6 * m1[i] + 0.8 * m2[i] + (i % 3) as f64 / 3.0)
            .collect();
        let wt: Vec<f64> = (0..n).map(|i| 5000.0 + 100.0 * (i % 5) as f64).collect();
        let st: Vec<f64> = (0..n).map(|i| 1.0 + (i % 4) as f64).collect();
        make_batch(vec![
            ("x", x),
            ("m1", m1),
            ("m2", m2),
            ("y", y),
            ("wt", wt),
            ("st", st),
        ])
    }

    async fn run_node(spec: serde_json::Value) -> Vec<RecordBatch> {
        let mut node = MediationSerialNodeFactory {}
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
    async fn unit_weight_smoke() {
        let rows = run_node(serde_json::json!({
            "exposure_column": "x",
            "mediator1_column": "m1",
            "mediator2_column": "m2",
            "outcome_column": "y",
            "n_bootstrap": 50,
        }))
        .await;
        assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
        assert_eq!(cell(&rows, "n_obs") as usize, 80);
        assert_eq!(cell(&rows, "n_bootstrap") as usize, 50);
        // te = direct + total_indirect, and total = ie_m1 + ie_m2 + ie_serial.
        let (direct, total, te) = (
            cell(&rows, "direct"),
            cell(&rows, "total_indirect"),
            cell(&rows, "te"),
        );
        assert!((te - direct - total).abs() < 1e-9);
        assert!(
            (total - cell(&rows, "ie_m1") - cell(&rows, "ie_m2") - cell(&rows, "ie_serial")).abs()
                < 1e-9
        );
        assert!(cell(&rows, "ie_serial_ci_lower") < cell(&rows, "ie_serial_ci_upper"));
    }

    #[tokio::test]
    async fn weighted_with_strata_smoke() {
        let rows = run_node(serde_json::json!({
            "exposure_column": "x",
            "mediator1_column": "m1",
            "mediator2_column": "m2",
            "outcome_column": "y",
            "weight_column": "wt",
            "strata_column": "st",
            "n_bootstrap": 50,
        }))
        .await;
        assert_eq!(cell(&rows, "n_obs") as usize, 80);
        assert_eq!(cell(&rows, "n_bootstrap") as usize, 50);
    }
}
