//! Random forest + SHAP node.
//!
//! Wraps [`epi::ensemble::random_forest`] and [`epi::shap::shap_values`].
//! Fits an RF classifier on feature columns against a label column, then
//! explains predictions with TreeSHAP-style attributions.

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::dag::DagError;
use dag_core::dag::graph::PortOutputs;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum EpiRfShapError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiRfShapError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl dag_core::dag::NodeError for EpiRfShapError {
    fn node_type(&self) -> &str {
        "epi_rf_shap"
    }
}

fn default_n_trees() -> usize {
    100
}
fn default_min_samples_leaf() -> usize {
    5
}
fn default_seed() -> u64 {
    42
}

/// Spec for [`EpiRfShapNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiRfShapSpec {
    /// Numeric feature columns (order defines feature indices).
    pub feature_columns: Vec<String>,
    /// Numeric label column (class labels; any distinct numeric values).
    pub label_column: String,
    /// Number of trees (default 100).
    #[serde(default = "default_n_trees")]
    pub n_trees: usize,
    /// Features sampled per split; omitted means √(p).
    #[serde(default)]
    pub mtry: Option<usize>,
    /// Minimum samples per leaf (default 5).
    #[serde(default = "default_min_samples_leaf")]
    pub min_samples_leaf: usize,
    /// Maximum tree depth; omitted means unlimited.
    #[serde(default)]
    pub max_depth: Option<usize>,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

/// Node fitting a random forest and emitting SHAP attributions.
#[derive(Clone)]
pub struct EpiRfShapNode {
    meta: NodePorts,
    spec: EpiRfShapSpec,
}

pub struct EpiRfShapNodeFactory;

impl NodeFactory for EpiRfShapNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_rf_shap"
    }

    fn desc(&self) -> &'static str {
        "Random forest classifier with SHAP feature attributions."
    }

    fn doc(&self) -> &'static str {
        "Fits a random forest classifier over `feature_columns` against \
         `label_column`, then computes TreeSHAP-style attributions for the \
         training rows. Complete-case rows only (any NaN feature or label \
         drops the row).\n\n\
         Port 0 — global importance: `feature, mean_abs_shap, rank` \
         (rank 1 = most important).\n\
         Port 1 — per-sample attributions in long format: `row_index, \
         feature, shap_value` — join with the input on row position if \
         per-sample explanation is needed.\n\
         Port 2 — fit summary, one row: `oob_accuracy, n_obs, n_features, \
         n_trees, mtry, baseline`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiRfShapSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EpiRfShapSpec = serde_json::from_value(spec)?;
        if spec.feature_columns.is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "epi_rf_shap requires at least one feature column".into(),
            ));
        }
        Ok(Box::new(EpiRfShapNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(None)
                .add_output_port(None)
                .add_output_port(None),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EpiRfShapNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "epi_rf_shap"
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
            .ok_or_else(|| EpiRfShapError::Column("no input connected".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| EpiRfShapError::Collect(e.to_string()))?;

        let n_rows = batches.first().map(|b| b.num_rows()).unwrap_or(0);
        let feature_cols: Vec<Vec<f64>> = self
            .spec
            .feature_columns
            .iter()
            .map(|col| extract_numeric_lenient(&batches, col))
            .collect::<Result<_, _>>()?;
        let labels_raw = extract_numeric_lenient(&batches, &self.spec.label_column)?;

        // Complete-case filter preserving row indices.
        let mut features: Vec<Vec<f64>> = Vec::new();
        let mut labels: Vec<f64> = Vec::new();
        let mut kept: Vec<usize> = Vec::new();
        for i in 0..n_rows {
            let mut row = Vec::with_capacity(feature_cols.len());
            let mut complete = !labels_raw[i].is_nan();
            for col in &feature_cols {
                let v = col[i];
                if !v.is_nan() {
                    row.push(v);
                } else {
                    complete = false;
                    break;
                }
            }
            if complete {
                features.push(row);
                labels.push(labels_raw[i]);
                kept.push(i);
            }
        }
        if features.is_empty() {
            return Err(EpiRfShapError::Column("no complete-case rows".into()).into());
        }

        let opts = epi::ensemble::RfOptions {
            n_trees: self.spec.n_trees,
            mtry: self.spec.mtry,
            min_samples_leaf: self.spec.min_samples_leaf,
            max_depth: self.spec.max_depth,
            seed: self.spec.seed,
        };
        let rf = epi::ensemble::random_forest(&features, &labels, &opts)
            .map_err(|e| EpiRfShapError::Computation(e.to_string()))?;
        let feature_names: Vec<String> = self.spec.feature_columns.clone();
        let shap = epi::shap::shap_values(&rf, &features, feature_names)
            .map_err(|e| EpiRfShapError::Computation(e.to_string()))?;

        let session = node_ctx.session();
        let mut outputs = PortOutputs::new();

        // Port 0: global importance ranked by mean |SHAP|.
        let mut order: Vec<usize> = (0..shap.mean_abs.len()).collect();
        order.sort_by(|a, b| shap.mean_abs[*b].total_cmp(&shap.mean_abs[*a]));
        let importance_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, true),
                Field::new("mean_abs_shap", DataType::Float64, true),
                Field::new("rank", DataType::UInt64, true),
            ])),
            vec![
                Arc::new(StringArray::from(
                    order
                        .iter()
                        .map(|&j| Some(shap.feature_names[j].clone()))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    order
                        .iter()
                        .map(|&j| Some(shap.mean_abs[j]))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(UInt64Array::from(
                    (0..order.len())
                        .map(|r| Some(r as u64 + 1))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|e| EpiRfShapError::Computation(format!("importance batch: {e}")))?;
        outputs.insert(
            0,
            session
                .read_batch(importance_batch)
                .map_err(|e| EpiRfShapError::ReadBatch(e.to_string()))?,
        );

        // Port 1: per-sample attributions (long), row indices preserved.
        let mut row_col: Vec<Option<i64>> = Vec::new();
        let mut feat_col: Vec<Option<u64>> = Vec::new();
        let mut val_col: Vec<Option<f64>> = Vec::new();
        for (i, sample) in shap.values.iter().enumerate() {
            for (j, v) in sample.iter().enumerate() {
                row_col.push(Some(kept[i] as i64));
                feat_col.push(Some(j as u64));
                val_col.push(Some(*v));
            }
        }
        let sample_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("row_index", DataType::Int64, true),
                Field::new("feature", DataType::UInt64, true),
                Field::new("shap_value", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(row_col)),
                Arc::new(UInt64Array::from(feat_col)),
                Arc::new(Float64Array::from(val_col)),
            ],
        )
        .map_err(|e| EpiRfShapError::Computation(format!("sample batch: {e}")))?;
        outputs.insert(
            1,
            session
                .read_batch(sample_batch)
                .map_err(|e| EpiRfShapError::ReadBatch(e.to_string()))?,
        );

        // Port 2: fit summary.
        let summary_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("oob_accuracy", DataType::Float64, true),
                Field::new("n_obs", DataType::Int64, true),
                Field::new("n_features", DataType::Int64, true),
                Field::new("n_trees", DataType::Int64, true),
                Field::new("mtry", DataType::Int64, true),
                Field::new("baseline", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![rf.oob_accuracy])),
                Arc::new(Int64Array::from(vec![rf.n_obs as i64])),
                Arc::new(Int64Array::from(vec![rf.n_features as i64])),
                Arc::new(Int64Array::from(vec![rf.n_trees as i64])),
                Arc::new(Int64Array::from(vec![rf.mtry as i64])),
                Arc::new(Float64Array::from(vec![shap.baseline])),
            ],
        )
        .map_err(|e| EpiRfShapError::Computation(format!("summary batch: {e}")))?;
        outputs.insert(
            2,
            session
                .read_batch(summary_batch)
                .map_err(|e| EpiRfShapError::ReadBatch(e.to_string()))?,
        );

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    /// Linearly separable two-class blobs.
    fn rf_input() -> Vec<NodeInput> {
        let mut f0: Vec<Option<f64>> = Vec::new();
        let mut f1: Vec<Option<f64>> = Vec::new();
        let mut y: Vec<Option<f64>> = Vec::new();
        for i in 0..20 {
            let class = (i % 2) as f64;
            f0.push(Some(class * 4.0 + (i % 3) as f64 * 0.1));
            f1.push(Some(class * -2.0 + (i % 5) as f64 * 0.2));
            y.push(Some(class));
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("x0", DataType::Float64, true),
                Field::new("x1", DataType::Float64, true),
                Field::new("label", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(f0)),
                Arc::new(Float64Array::from(f1)),
                Arc::new(Float64Array::from(y)),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        vec![NodeInput::new_dataframe(0, ctx.read_batch(batch).unwrap())]
    }

    #[tokio::test]
    async fn three_ports_carry_importance_samples_and_summary() {
        use arrow_array::Array;
        let inputs = rf_input();
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let mut node = EpiRfShapNodeFactory
            .build(
                serde_json::json!({
                    "feature_columns": ["x0", "x1"],
                    "label_column": "label",
                    "n_trees": 20,
                    "seed": 7
                }),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(
                &ctx,
                &inputs,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        for port in 0..3 {
            assert!(outputs.get(&port).is_some(), "port {port} missing");
        }
        // Per-sample long table: 20 rows × 2 features.
        let samples = outputs
            .get(&1)
            .and_then(|v| v.as_dataframe().ok())
            .expect("sample table");
        let batches = samples.clone().collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 40);

        // Summary row is exactly one.
        let summary = outputs
            .get(&2)
            .and_then(|v| v.as_dataframe().ok())
            .expect("summary table");
        let rows = summary.clone().collect().await.unwrap();
        assert_eq!(rows[0].num_rows(), 1);
        // SHAP baseline must be finite.
        let baseline = rows[0]
            .column(5)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(baseline.value(0).is_finite());
    }

    #[tokio::test]
    async fn nan_feature_rows_are_dropped() {
        let mut f0: Vec<Option<f64>> = vec![];
        let mut y: Vec<Option<f64>> = vec![];
        for i in 0..10 {
            f0.push(if i == 3 { None } else { Some(i as f64) });
            y.push(Some((i % 2) as f64));
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("x0", DataType::Float64, true),
                Field::new("label", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(f0)),
                Arc::new(Float64Array::from(y)),
            ],
        )
        .unwrap();
        let session = SessionContext::new();
        let inputs = vec![NodeInput::new_dataframe(
            0,
            session.read_batch(batch).unwrap(),
        )];
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EpiRfShapNodeFactory
            .build(
                serde_json::json!({
                    "feature_columns": ["x0"],
                    "label_column": "label",
                    "n_trees": 5
                }),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(
                &ctx,
                &inputs,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let samples = outputs
            .get(&1)
            .and_then(|v| v.as_dataframe().ok())
            .expect("sample table");
        let batches = samples.clone().collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 9, "row with the NaN feature must be dropped");
    }
}
