//! SVM + Anomaly detection DAG nodes.

use std::sync::Arc;

use arrow_array::{Array, BooleanArray, Float64Array, RecordBatch, UInt32Array};
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
        node_type: "ml_svm_anomaly".into(),
        msg: "no input".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_svm_anomaly".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_svm_anomaly".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// SVM Classifier
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvmSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_svm_kernel")]
    pub kernel: String,
    #[serde(default = "d_svm_c")]
    pub c: f64,
}
fn d_svm_kernel() -> String {
    "rbf".into()
}
fn d_svm_c() -> f64 {
    1.0
}

pub struct SvmFactory;
impl NodeFactory for SvmFactory {
    fn kind(&self) -> &'static str {
        "ml_svm"
    }
    fn desc(&self) -> &'static str {
        "Support Vector Machine classifier."
    }
    fn doc(&self) -> &'static str {
        "SVM: binary classification via SMO solver. Supports linear, RBF, and polynomial kernels."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvmSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: SvmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvmNode {
            features: s.features,
            label_column: s.label_column,
            kernel: s.kernel,
            c: s.c,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct SvmNode {
    features: Vec<String>,
    label_column: String,
    kernel: String,
    c: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for SvmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_svm"
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
                node_type: "ml_svm".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_svm".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result =
            ml::svm_ensemble::svm_classify(&data, &labels, &self.kernel, self.c).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_svm".into(),
                    msg: e.to_string(),
                }
            })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        fields.push(Arc::new(Field::new(
            "probability",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.probabilities)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_svm".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// AdaBoost
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdaBoostSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_ab_n")]
    pub n_estimators: usize,
    #[serde(default = "d_ab_lr")]
    pub learning_rate: f64,
}
fn d_ab_n() -> usize {
    50
}
fn d_ab_lr() -> f64 {
    1.0
}

pub struct AdaBoostFactory;
impl NodeFactory for AdaBoostFactory {
    fn kind(&self) -> &'static str {
        "ml_adaboost"
    }
    fn desc(&self) -> &'static str {
        "AdaBoost ensemble classifier."
    }
    fn doc(&self) -> &'static str {
        "AdaBoost: boosted ensemble of decision trees with adaptive sample weighting."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AdaBoostSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AdaBoostSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AdaBoostNode {
            features: s.features,
            label_column: s.label_column,
            n_estimators: s.n_estimators,
            learning_rate: s.learning_rate,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct AdaBoostNode {
    features: Vec<String>,
    label_column: String,
    n_estimators: usize,
    learning_rate: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for AdaBoostNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_adaboost"
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
                node_type: "ml_adaboost".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_adaboost".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result =
            ml::svm_ensemble::adaboost(&data, &labels, self.n_estimators, self.learning_rate)
                .map_err(|e| DagError::NodeError {
                    node_type: "ml_adaboost".into(),
                    msg: e.to_string(),
                })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        fields.push(Arc::new(Field::new(
            "probability",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.probabilities)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_adaboost".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Isolation Forest
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct IsolationForestSpec {
    pub features: Vec<String>,
    #[serde(default = "d_if_trees")]
    pub n_trees: usize,
    #[serde(default = "d_if_samples")]
    pub max_samples: usize,
    #[serde(default = "d_if_seed")]
    pub seed: u64,
}
fn d_if_trees() -> usize {
    100
}
fn d_if_samples() -> usize {
    256
}
fn d_if_seed() -> u64 {
    42
}

pub struct IsolationForestFactory;
impl NodeFactory for IsolationForestFactory {
    fn kind(&self) -> &'static str {
        "ml_isolation_forest"
    }
    fn desc(&self) -> &'static str {
        "Isolation Forest anomaly detection."
    }
    fn doc(&self) -> &'static str {
        "IsolationForest: detects anomalies via random partition trees. Points with shorter average path lengths are more anomalous. Outputs anomaly_score + is_outlier."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(IsolationForestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: IsolationForestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(IsolationForestNode {
            features: s.features,
            n_trees: s.n_trees,
            max_samples: s.max_samples,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct IsolationForestNode {
    features: Vec<String>,
    n_trees: usize,
    max_samples: usize,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for IsolationForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_isolation_forest"
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
                node_type: "ml_isolation_forest".into(),
                msg: e.to_string(),
            })?;
        let result =
            ml::anomaly::isolation_forest(&data, self.n_trees, self.max_samples, self.seed)
                .map_err(|e| DagError::NodeError {
                    node_type: "ml_isolation_forest".into(),
                    msg: e.to_string(),
                })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new(
            "anomaly_score",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.scores)));
        fields.push(Arc::new(Field::new("is_outlier", DataType::Boolean, false)));
        arrays.push(Arc::new(BooleanArray::from(result.is_outlier)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_isolation_forest".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Z-score outlier
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ZscoreOutlierSpec {
    pub features: Vec<String>,
    #[serde(default = "d_zs_threshold")]
    pub threshold: f64,
}
fn d_zs_threshold() -> f64 {
    3.0
}

pub struct ZscoreOutlierFactory;
impl NodeFactory for ZscoreOutlierFactory {
    fn kind(&self) -> &'static str {
        "ml_zscore_outlier"
    }
    fn desc(&self) -> &'static str {
        "Z-score outlier detection."
    }
    fn doc(&self) -> &'static str {
        "Z-score: flags rows where any feature's z-score exceeds threshold. Simple but effective for Gaussian-distributed features."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ZscoreOutlierSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ZscoreOutlierSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ZscoreOutlierNode {
            features: s.features,
            threshold: s.threshold,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ZscoreOutlierNode {
    features: Vec<String>,
    threshold: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ZscoreOutlierNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_zscore_outlier"
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
                node_type: "ml_zscore_outlier".into(),
                msg: e.to_string(),
            })?;
        let result = ml::anomaly::zscore_outliers(&data, self.threshold).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_zscore_outlier".into(),
                msg: e.to_string(),
            }
        })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("max_zscore", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.scores)));
        fields.push(Arc::new(Field::new("is_outlier", DataType::Boolean, false)));
        arrays.push(Arc::new(BooleanArray::from(result.is_outlier)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_zscore_outlier".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// LOF
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LofSpec {
    pub features: Vec<String>,
    #[serde(default = "d_lof_k")]
    pub k: usize,
}
fn d_lof_k() -> usize {
    20
}

pub struct LofFactory;
impl NodeFactory for LofFactory {
    fn kind(&self) -> &'static str {
        "ml_lof"
    }
    fn desc(&self) -> &'static str {
        "Local Outlier Factor anomaly detection."
    }
    fn doc(&self) -> &'static str {
        "LOF: density-based anomaly detection. LOF > 1 indicates a point is sparser than its neighbors."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LofSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LofSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LofNode {
            features: s.features,
            k: s.k,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LofNode {
    features: Vec<String>,
    k: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LofNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_lof"
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
                node_type: "ml_lof".into(),
                msg: e.to_string(),
            })?;
        let result =
            ml::anomaly::local_outlier_factor(&data, self.k).map_err(|e| DagError::NodeError {
                node_type: "ml_lof".into(),
                msg: e.to_string(),
            })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("lof_score", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.scores)));
        fields.push(Arc::new(Field::new("is_outlier", DataType::Boolean, false)));
        arrays.push(Arc::new(BooleanArray::from(result.is_outlier)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_lof".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
