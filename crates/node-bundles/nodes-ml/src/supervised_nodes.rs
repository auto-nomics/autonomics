//! Supervised learning DAG nodes — classification + regression.
//!
//! Classification: ml_logistic, ml_gaussian_nb, ml_knn, ml_decision_tree
//! Regression: ml_linear_regress, ml_elastic_net, ml_lars, ml_pls

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, UInt32Array};
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
        node_type: "ml_supervised".into(),
        msg: "no input".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_supervised".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_supervised".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// Logistic Regression
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LogisticSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_alpha")]
    pub alpha: f64,
    #[serde(default = "d_max_iter")]
    pub max_iter: usize,
}
fn d_alpha() -> f64 {
    0.1
}
fn d_max_iter() -> usize {
    200
}

pub struct LogisticFactory;
impl NodeFactory for LogisticFactory {
    fn kind(&self) -> &'static str {
        "ml_logistic"
    }
    fn desc(&self) -> &'static str {
        "Binary logistic regression classifier."
    }
    fn doc(&self) -> &'static str {
        "LogisticRegression: L2-regularised binary logistic regression via gradient descent. Outputs predictions + probabilities."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LogisticSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LogisticSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LogisticNode {
            features: s.features,
            label_column: s.label_column,
            alpha: s.alpha,
            max_iter: s.max_iter,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LogisticNode {
    features: Vec<String>,
    label_column: String,
    alpha: f64,
    max_iter: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LogisticNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_logistic"
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
                node_type: "ml_logistic".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_logistic".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result = ml::classify::logistic_regression(&data, &labels, self.alpha, self.max_iter)
            .map_err(|e| DagError::NodeError {
            node_type: "ml_logistic".into(),
            msg: e.to_string(),
        })?;
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
            .map(|i| batches.first().unwrap().column(i).clone())
            .collect();
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
                    node_type: "ml_logistic".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Gaussian Naive Bayes
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GaussianNbSpec {
    pub features: Vec<String>,
    pub label_column: String,
}

pub struct GaussianNbFactory;
impl NodeFactory for GaussianNbFactory {
    fn kind(&self) -> &'static str {
        "ml_gaussian_nb"
    }
    fn desc(&self) -> &'static str {
        "Gaussian Naive Bayes classifier."
    }
    fn doc(&self) -> &'static str {
        "GaussianNB: assumes features are conditionally independent given class, each following a Gaussian distribution."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GaussianNbSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: GaussianNbSpec = serde_json::from_value(spec)?;
        Ok(Box::new(GaussianNbNode {
            features: s.features,
            label_column: s.label_column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct GaussianNbNode {
    features: Vec<String>,
    label_column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for GaussianNbNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_gaussian_nb"
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
                node_type: "ml_gaussian_nb".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_gaussian_nb".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result =
            ml::classify::gaussian_nb(&data, &labels).map_err(|e| DagError::NodeError {
                node_type: "ml_gaussian_nb".into(),
                msg: e.to_string(),
            })?;
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
            .map(|i| batches.first().unwrap().column(i).clone())
            .collect();
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_gaussian_nb".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// KNN Classifier
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KnnSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_k")]
    pub k: usize,
}
fn d_k() -> usize {
    5
}

pub struct KnnFactory;
impl NodeFactory for KnnFactory {
    fn kind(&self) -> &'static str {
        "ml_knn"
    }
    fn desc(&self) -> &'static str {
        "k-Nearest Neighbors classifier."
    }
    fn doc(&self) -> &'static str {
        "KNN: classifies each sample by majority vote among its k nearest neighbors. Uses Euclidean distance."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KnnSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: KnnSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KnnNode {
            features: s.features,
            label_column: s.label_column,
            k: s.k,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct KnnNode {
    features: Vec<String>,
    label_column: String,
    k: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for KnnNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_knn"
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
                node_type: "ml_knn".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_knn".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        // Train = Test = all data (in-sample evaluation)
        let result = ml::classify::knn_classify(&data, &labels, &data, self.k).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_knn".into(),
                msg: e.to_string(),
            }
        })?;
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
            .map(|i| batches.first().unwrap().column(i).clone())
            .collect();
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_knn".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Decision Tree
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DecisionTreeSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_dt_depth")]
    pub max_depth: usize,
    #[serde(default = "d_dt_split")]
    pub min_samples_split: usize,
    #[serde(default = "d_dt_leaf")]
    pub min_samples_leaf: usize,
}
fn d_dt_depth() -> usize {
    10
}
fn d_dt_split() -> usize {
    2
}
fn d_dt_leaf() -> usize {
    1
}

pub struct DecisionTreeFactory;
impl NodeFactory for DecisionTreeFactory {
    fn kind(&self) -> &'static str {
        "ml_decision_tree"
    }
    fn desc(&self) -> &'static str {
        "Decision tree (CART) classifier."
    }
    fn doc(&self) -> &'static str {
        "DecisionTree: CART classification tree with configurable depth, split, and leaf constraints."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DecisionTreeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: DecisionTreeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DecisionTreeNode {
            features: s.features,
            label_column: s.label_column,
            max_depth: s.max_depth,
            min_samples_split: s.min_samples_split,
            min_samples_leaf: s.min_samples_leaf,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct DecisionTreeNode {
    features: Vec<String>,
    label_column: String,
    max_depth: usize,
    min_samples_split: usize,
    min_samples_leaf: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for DecisionTreeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_decision_tree"
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
                node_type: "ml_decision_tree".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_decision_tree".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result = ml::classify::decision_tree(
            &data,
            &labels,
            self.max_depth,
            self.min_samples_split,
            self.min_samples_leaf,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_decision_tree".into(),
            msg: e.to_string(),
        })?;
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
            .map(|i| batches.first().unwrap().column(i).clone())
            .collect();
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_decision_tree".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Linear Regression
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LinearRegressSpec {
    pub features: Vec<String>,
    pub target_column: String,
}

pub struct LinearRegressFactory;
impl NodeFactory for LinearRegressFactory {
    fn kind(&self) -> &'static str {
        "ml_linear_regress"
    }
    fn desc(&self) -> &'static str {
        "Ordinary least squares linear regression."
    }
    fn doc(&self) -> &'static str {
        "LinearRegression: OLS via linfa-linear. Outputs predictions + coefficients."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LinearRegressSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LinearRegressSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LinearRegressNode {
            features: s.features,
            target_column: s.target_column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LinearRegressNode {
    features: Vec<String>,
    target_column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LinearRegressNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_linear_regress"
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
                node_type: "ml_linear_regress".into(),
                msg: e.to_string(),
            })?;
        let target =
            common::extract_numeric_column(&batches, &self.target_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_linear_regress".into(),
                    msg: e.to_string(),
                }
            })?;
        let result =
            ml::regress::linear_regression(&data, &target).map_err(|e| DagError::NodeError {
                node_type: "ml_linear_regress".into(),
                msg: e.to_string(),
            })?;
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
            .map(|i| batches.first().unwrap().column(i).clone())
            .collect();
        fields.push(Arc::new(Field::new("prediction", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.predictions)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_linear_regress".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ElasticNet (covers Ridge/Lasso)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ElasticNetSpec {
    pub features: Vec<String>,
    pub target_column: String,
    #[serde(default = "d_en_penalty")]
    pub penalty: f64,
    #[serde(default = "d_en_l1")]
    pub l1_ratio: f64,
    #[serde(default = "d_en_iter")]
    pub max_iter: usize,
    #[serde(default = "d_en_tol")]
    pub tol: f64,
}
fn d_en_penalty() -> f64 {
    0.01
}
fn d_en_l1() -> f64 {
    0.5
}
fn d_en_iter() -> usize {
    1000
}
fn d_en_tol() -> f64 {
    1e-4
}

pub struct ElasticNetFactory;
impl NodeFactory for ElasticNetFactory {
    fn kind(&self) -> &'static str {
        "ml_elastic_net"
    }
    fn desc(&self) -> &'static str {
        "Elastic Net regression (covers Ridge/Lasso via l1_ratio)."
    }
    fn doc(&self) -> &'static str {
        "ElasticNet: L1+L2 regularised regression. l1_ratio=0 → Ridge, l1_ratio=1 → Lasso, 0<l1_ratio<1 → Elastic Net."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ElasticNetSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ElasticNetSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ElasticNetNode {
            features: s.features,
            target_column: s.target_column,
            penalty: s.penalty,
            l1_ratio: s.l1_ratio,
            max_iter: s.max_iter,
            tol: s.tol,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ElasticNetNode {
    features: Vec<String>,
    target_column: String,
    penalty: f64,
    l1_ratio: f64,
    max_iter: usize,
    tol: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ElasticNetNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_elastic_net"
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
                node_type: "ml_elastic_net".into(),
                msg: e.to_string(),
            })?;
        let target =
            common::extract_numeric_column(&batches, &self.target_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_elastic_net".into(),
                    msg: e.to_string(),
                }
            })?;
        let result = ml::regress::elastic_net(
            &data,
            &target,
            self.penalty,
            self.l1_ratio,
            self.max_iter,
            self.tol,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_elastic_net".into(),
            msg: e.to_string(),
        })?;
        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
        let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
            .map(|i| batches.first().unwrap().column(i).clone())
            .collect();
        fields.push(Arc::new(Field::new("prediction", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.predictions)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_elastic_net".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
