//! ROC / AUC / DeLong test node.
//!
//! Wraps [`epi::roc`]. Computes AUC for one or two models, optionally
//! performing a DeLong test to compare them.
//!
//! Output schema (single row):
//!
//! | Column         | Type    | Description                              |
//! |----------------|---------|------------------------------------------|
//! | `auc1`         | Float64 | AUC of model 1                            |
//! | `auc1_ci_lower`| Float64 | 95% CI lower for AUC 1                    |
//! | `auc1_ci_upper`| Float64 | 95% CI upper for AUC 1                    |
//! | `auc2`         | Float64 | AUC of model 2 (null if single-model)     |
//! | `delong_z`     | Float64 | DeLong z-statistic (null if single-model) |
//! | `delong_p`     | Float64 | DeLong p-value (null if single-model)      |
//! | `n_pos`        | Int32   | Number of positive cases                   |
//! | `n_neg`        | Int32   | Number of negative cases                   |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum EpiRocError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiRocError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for EpiRocError {
    fn node_type(&self) -> &str {
        "epi_roc"
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiRocNodeSpec {
    /// Score column for model 1 (predicted probability or any continuous predictor).
    pub score1_column: String,
    /// Binary label column (0/1).
    pub label_column: String,
    /// Optional score column for model 2. When provided, a DeLong test is
    /// performed comparing AUC 1 vs AUC 2.
    #[serde(default)]
    pub score2_column: Option<String>,
    /// Bootstrap iterations for the AUC confidence interval (default 1000).
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    /// Random seed for bootstrap (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

#[derive(Clone)]
pub struct EpiRocNode {
    meta: NodePorts,
    score1_column: String,
    label_column: String,
    score2_column: Option<String>,
    n_bootstrap: usize,
    seed: u64,
}

pub struct EpiRocNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for EpiRocNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_roc"
    }
    fn desc(&self) -> &'static str {
        "ROC AUC with bootstrap CI and optional DeLong test for two-model comparison."
    }
    fn doc(&self) -> &'static str {
        "Computes the area under the ROC curve (AUC) for one or two score \
        columns against a binary label. Provides bootstrap 95% CI and, when \
        two score columns are specified, a DeLong test for the difference \
        in AUC."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiRocNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: EpiRocNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EpiRocNode {
            meta: port_layout(),
            score1_column: s.score1_column,
            label_column: s.label_column,
            score2_column: s.score2_column,
            n_bootstrap: s.n_bootstrap,
            seed: s.seed,
        }))
    }
}

#[async_trait]
impl DagNode for EpiRocNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "epi_roc"
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
        let input = inputs
            .first()
            .ok_or(EpiRocError::Column("no input connected".to_string()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| EpiRocError::Collect(e.to_string()))?;

        let s1_raw = extract_numeric_lenient(&batches, &self.score1_column)?;
        let label_raw = extract_numeric_lenient(&batches, &self.label_column)?;
        let s2_raw = match &self.score2_column {
            Some(col) => Some(extract_numeric_lenient(&batches, col)?),
            None => None,
        };

        // Validate binary labels.
        for &v in &label_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(EpiRocError::Column(format!(
                    "label '{}' must be binary (0/1), found {v}",
                    self.label_column
                ))
                .into());
            }
        }

        // Complete-case filter.
        let n = label_raw.len();
        let mut s1 = Vec::with_capacity(n);
        let mut labels = Vec::with_capacity(n);
        let mut s2 = Vec::with_capacity(n);
        let has_s2 = s2_raw.is_some();
        let s2_data = s2_raw.unwrap_or_default();
        for i in 0..n {
            if label_raw[i].is_nan() || s1_raw[i].is_nan() {
                continue;
            }
            if has_s2 && s2_data[i].is_nan() {
                continue;
            }
            s1.push(s1_raw[i]);
            labels.push(label_raw[i] as u64);
            if has_s2 {
                s2.push(s2_data[i]);
            }
        }

        if labels.is_empty() {
            return Err(EpiRocError::Column("no complete-case rows".to_string()).into());
        }

        // AUC + bootstrap CI for model 1.
        let roc1 = epi::roc::bootstrap_auc_ci(&s1, &labels, self.n_bootstrap, self.seed)
            .map_err(|e| EpiRocError::Computation(e.to_string()))?;

        // Optional model 2 + DeLong.
        let (auc2, delong_z, delong_p) = if has_s2 {
            let roc2 =
                epi::roc::auc(&s2, &labels).map_err(|e| EpiRocError::Computation(e.to_string()))?;
            let dl = epi::roc::delong_test(&s1, &s2, &labels)
                .map_err(|e| EpiRocError::Computation(e.to_string()))?;
            (Some(roc2.auc), Some(dl.z), Some(dl.p_value))
        } else {
            (None, None, None)
        };

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("auc1", DataType::Float64, false),
                Field::new("auc1_ci_lower", DataType::Float64, false),
                Field::new("auc1_ci_upper", DataType::Float64, false),
                Field::new("auc2", DataType::Float64, true),
                Field::new("delong_z", DataType::Float64, true),
                Field::new("delong_p", DataType::Float64, true),
                Field::new("n_pos", DataType::Int32, false),
                Field::new("n_neg", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![roc1.auc])),
                Arc::new(Float64Array::from(vec![roc1.ci_lower])),
                Arc::new(Float64Array::from(vec![roc1.ci_upper])),
                Arc::new(Float64Array::from(vec![auc2.unwrap_or(f64::NAN)])),
                Arc::new(Float64Array::from(vec![delong_z.unwrap_or(f64::NAN)])),
                Arc::new(Float64Array::from(vec![delong_p.unwrap_or(f64::NAN)])),
                Arc::new(Int32Array::from(vec![roc1.n_pos as i32])),
                Arc::new(Int32Array::from(vec![roc1.n_neg as i32])),
            ],
        )
        .expect("roc schema");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| EpiRocError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
