//! Feature selection DAG nodes — variance threshold, SelectKBest, F-regression.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_feat".into(),
        msg: "no input".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_feat".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_feat".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// VarianceThreshold
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct VarianceThresholdSpec {
    pub features: Vec<String>,
    #[serde(default = "d_vt_threshold")]
    pub threshold: f64,
}
fn d_vt_threshold() -> f64 {
    0.0
}

pub struct VarianceThresholdFactory;
impl NodeFactory for VarianceThresholdFactory {
    fn kind(&self) -> &'static str {
        "ml_variance_threshold"
    }
    fn desc(&self) -> &'static str {
        "Remove features with variance below a threshold."
    }
    fn doc(&self) -> &'static str {
        "VarianceThreshold: filters out low-variance features. Outputs a summary table with feature names, variances, and selected status."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(VarianceThresholdSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: VarianceThresholdSpec = serde_json::from_value(spec)?;
        Ok(Box::new(VarianceThresholdNode {
            features: s.features,
            threshold: s.threshold,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct VarianceThresholdNode {
    features: Vec<String>,
    threshold: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for VarianceThresholdNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_variance_threshold"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_variance_threshold".into(),
                msg: e.to_string(),
            })?;
        let variances = ml::feat_select::column_variances(&data);
        let selected = ml::feat_select::variance_threshold(&data, self.threshold);

        let names: Vec<&str> = self.features.iter().map(|s| s.as_str()).collect();
        let sel_bools: Vec<bool> = (0..self.features.len())
            .map(|i| selected.contains(&i))
            .collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("variance", DataType::Float64, false),
                Field::new("selected", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(names)),
                Arc::new(Float64Array::from(variances)),
                Arc::new(arrow_array::BooleanArray::from(sel_bools)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_variance_threshold".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// SelectKBest (classification)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SelectKBestSpec {
    pub features: Vec<String>,
    pub label_column: String,
    pub k: usize,
}

pub struct SelectKBestFactory;
impl NodeFactory for SelectKBestFactory {
    fn kind(&self) -> &'static str {
        "ml_select_k_best"
    }
    fn desc(&self) -> &'static str {
        "Select top-K features by ANOVA F-value."
    }
    fn doc(&self) -> &'static str {
        "SelectKBest: ranks features by ANOVA F-statistic against class labels, returns the top K. Outputs feature, score, rank."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SelectKBestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: SelectKBestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SelectKBestNode {
            features: s.features,
            label_column: s.label_column,
            k: s.k,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct SelectKBestNode {
    features: Vec<String>,
    label_column: String,
    k: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for SelectKBestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_select_k_best"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_select_k_best".into(),
                msg: e.to_string(),
            })?;
        let labels_f64 =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_select_k_best".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f64.into_iter().map(|v| v as usize).collect();
        let (indices, scores) =
            ml::feat_select::select_k_best(&data, &labels, self.k).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_select_k_best".into(),
                    msg: e.to_string(),
                }
            })?;

        let names: Vec<String> = indices.iter().map(|&i| self.features[i].clone()).collect();
        let ranks: Vec<u32> = (1..=indices.len() as u32).collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("f_score", DataType::Float64, false),
                Field::new("rank", DataType::UInt32, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(names)),
                Arc::new(Float64Array::from(scores)),
                Arc::new(UInt32Array::from(ranks)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_select_k_best".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// CorrelationRank (regression)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CorrelationRankSpec {
    pub features: Vec<String>,
    pub target_column: String,
}

pub struct CorrelationRankFactory;
impl NodeFactory for CorrelationRankFactory {
    fn kind(&self) -> &'static str {
        "ml_correlation_rank"
    }
    fn desc(&self) -> &'static str {
        "Rank features by absolute correlation with a continuous target."
    }
    fn doc(&self) -> &'static str {
        "CorrelationRank: computes |Pearson r| between each feature and the target column, outputs feature name and correlation sorted by descending strength."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CorrelationRankSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: CorrelationRankSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CorrelationRankNode {
            features: s.features,
            target_column: s.target_column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct CorrelationRankNode {
    features: Vec<String>,
    target_column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for CorrelationRankNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_correlation_rank"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_correlation_rank".into(),
                msg: e.to_string(),
            })?;
        let target =
            common::extract_numeric_column(&batches, &self.target_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_correlation_rank".into(),
                    msg: e.to_string(),
                }
            })?;
        let corrs = ml::feat_select::f_regression(&data, &target);
        let mut indexed: Vec<(usize, f64)> = corrs.iter().copied().enumerate().collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let names: Vec<String> = indexed
            .iter()
            .map(|&(i, _)| self.features[i].clone())
            .collect();
        let values: Vec<f64> = indexed.iter().map(|&(_, c)| c).collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("abs_correlation", DataType::Float64, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(names)),
                Arc::new(Float64Array::from(values)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_correlation_rank".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
