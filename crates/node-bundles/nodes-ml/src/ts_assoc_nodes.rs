//! Time series + Association rule DAG nodes.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
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
        node_type: "ml_ts_assoc".into(),
        msg: "no input".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_ts_assoc".into(),
            msg: format!("collect: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_ts_assoc".into(),
            msg: format!("read_batch: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// Exponential Smoothing
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExpSmoothingSpec {
    pub value_column: String,
    #[serde(default = "d_es_alpha")]
    pub alpha: f64,
    #[serde(default = "d_es_forecast")]
    pub n_forecast: usize,
}
fn d_es_alpha() -> f64 {
    0.3
}
fn d_es_forecast() -> usize {
    5
}

pub struct ExpSmoothingFactory;
impl NodeFactory for ExpSmoothingFactory {
    fn kind(&self) -> &'static str {
        "ml_exp_smoothing"
    }
    fn desc(&self) -> &'static str {
        "Simple exponential smoothing + forecast."
    }
    fn doc(&self) -> &'static str {
        "ExpSmoothing: single exponential smoothing for time series without trend. Outputs fitted values + h-step forecast."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ExpSmoothingSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ExpSmoothingSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ExpSmoothingNode {
            value_column: s.value_column,
            alpha: s.alpha,
            n_forecast: s.n_forecast,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ExpSmoothingNode {
    value_column: String,
    alpha: f64,
    n_forecast: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ExpSmoothingNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_exp_smoothing"
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
        let data = common::extract_numeric_column(&batches, &self.value_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_exp_smoothing".into(),
                msg: e.to_string(),
            }
        })?;
        let result = ml::timeseries::exponential_smoothing(&data, self.alpha, self.n_forecast)
            .map_err(|e| DagError::NodeError {
                node_type: "ml_exp_smoothing".into(),
                msg: e.to_string(),
            })?;
        let n = data.len();
        let total = n + self.n_forecast;
        let fitted_padded: Vec<f64> = result
            .fitted
            .into_iter()
            .chain(std::iter::repeat_n(f64::NAN, self.n_forecast))
            .collect();
        let forecast_padded: Vec<f64> = std::iter::repeat_n(f64::NAN, n)
            .chain(result.forecast)
            .collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("index", DataType::UInt32, false),
                Field::new("fitted", DataType::Float64, true),
                Field::new("forecast", DataType::Float64, true),
            ])),
            vec![
                Arc::new(arrow_array::UInt32Array::from(
                    (0..total as u32).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(fitted_padded)),
                Arc::new(Float64Array::from(forecast_padded)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_exp_smoothing".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// STL Decomposition
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StlSpec {
    pub value_column: String,
    pub period: usize,
}

pub struct StlFactory;
impl NodeFactory for StlFactory {
    fn kind(&self) -> &'static str {
        "ml_stl_decompose"
    }
    fn desc(&self) -> &'static str {
        "STL decomposition (trend + seasonal + residual)."
    }
    fn doc(&self) -> &'static str {
        "STL: decomposes a time series into trend, seasonal, and residual components using moving-average trend extraction."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StlSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: StlSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StlNode {
            value_column: s.value_column,
            period: s.period,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct StlNode {
    value_column: String,
    period: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for StlNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_stl_decompose"
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
        let data = common::extract_numeric_column(&batches, &self.value_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_stl_decompose".into(),
                msg: e.to_string(),
            }
        })?;
        let result =
            ml::timeseries::stl_decompose(&data, self.period).map_err(|e| DagError::NodeError {
                node_type: "ml_stl_decompose".into(),
                msg: e.to_string(),
            })?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("trend", DataType::Float64, false),
                Field::new("seasonal", DataType::Float64, false),
                Field::new("residual", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Float64Array::from(result.trend)),
                Arc::new(Float64Array::from(result.seasonal)),
                Arc::new(Float64Array::from(result.residual)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_stl_decompose".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Change-point detection (PELT)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PeltSpec {
    pub value_column: String,
    #[serde(default = "d_pelt_penalty")]
    pub penalty: f64,
}
fn d_pelt_penalty() -> f64 {
    10.0
}

pub struct PeltFactory;
impl NodeFactory for PeltFactory {
    fn kind(&self) -> &'static str {
        "ml_changepoint"
    }
    fn desc(&self) -> &'static str {
        "Change-point detection (PELT algorithm)."
    }
    fn doc(&self) -> &'static str {
        "PELT: detects change-points in a time series by minimizing segment cost + penalty. Returns detected change-point indices."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PeltSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PeltSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PeltNode {
            value_column: s.value_column,
            penalty: s.penalty,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PeltNode {
    value_column: String,
    penalty: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PeltNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_changepoint"
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
        let data = common::extract_numeric_column(&batches, &self.value_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_changepoint".into(),
                msg: e.to_string(),
            }
        })?;
        let cps = ml::timeseries::pelt(&data, self.penalty).map_err(|e| DagError::NodeError {
            node_type: "ml_changepoint".into(),
            msg: e.to_string(),
        })?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "changepoint_index",
                DataType::UInt32,
                false,
            )])),
            vec![Arc::new(arrow_array::UInt32Array::from(
                cps.iter().map(|&c| c as u32).collect::<Vec<_>>(),
            ))],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_changepoint".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Apriori Association Rules
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AprioriSpec {
    pub item_column: String,
    pub transaction_column: String,
    #[serde(default = "d_min_sup")]
    pub min_support: f64,
    #[serde(default = "d_min_conf")]
    pub min_confidence: f64,
}
fn d_min_sup() -> f64 {
    0.1
}
fn d_min_conf() -> f64 {
    0.5
}

pub struct AprioriFactory;
impl NodeFactory for AprioriFactory {
    fn kind(&self) -> &'static str {
        "ml_apriori"
    }
    fn desc(&self) -> &'static str {
        "Apriori frequent itemset + association rule mining."
    }
    fn doc(&self) -> &'static str {
        "Apriori: discovers frequent itemsets and association rules from transaction data. Requires item + transaction ID columns."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AprioriSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AprioriSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AprioriNode {
            item_column: s.item_column,
            transaction_column: s.transaction_column,
            min_support: s.min_support,
            min_confidence: s.min_confidence,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct AprioriNode {
    item_column: String,
    transaction_column: String,
    min_support: f64,
    min_confidence: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for AprioriNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_apriori"
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
        let items = common::extract_string_column(&batches, &self.item_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_apriori".into(),
                msg: e.to_string(),
            }
        })?;
        let txn_ids =
            common::extract_string_column(&batches, &self.transaction_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_apriori".into(),
                    msg: e.to_string(),
                }
            })?;

        // Group items by transaction
        let mut txn_map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (item, txn) in items.iter().zip(txn_ids.iter()) {
            txn_map.entry(txn.clone()).or_default().push(item.clone());
        }
        let transactions: Vec<Vec<String>> = txn_map.into_values().collect();

        let (_frequent, rules) =
            ml::assoc_recsys::apriori(&transactions, self.min_support, self.min_confidence)
                .map_err(|e| DagError::NodeError {
                    node_type: "ml_apriori".into(),
                    msg: e.to_string(),
                })?;

        let antecedents: Vec<String> = rules.iter().map(|r| r.antecedent.join(", ")).collect();
        let consequents: Vec<String> = rules.iter().map(|r| r.consequent.join(", ")).collect();
        let supports: Vec<f64> = rules.iter().map(|r| r.support).collect();
        let confidences: Vec<f64> = rules.iter().map(|r| r.confidence).collect();
        let lifts: Vec<f64> = rules.iter().map(|r| r.lift).collect();

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("antecedent", DataType::Utf8, false),
                Field::new("consequent", DataType::Utf8, false),
                Field::new("support", DataType::Float64, false),
                Field::new("confidence", DataType::Float64, false),
                Field::new("lift", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(antecedents)),
                Arc::new(StringArray::from(consequents)),
                Arc::new(Float64Array::from(supports)),
                Arc::new(Float64Array::from(confidences)),
                Arc::new(Float64Array::from(lifts)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_apriori".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
