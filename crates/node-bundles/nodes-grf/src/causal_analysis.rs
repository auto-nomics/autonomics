//! Causal analysis DAG nodes — ATE, best-linear projection, calibration, scores.
//!
//! All consume a causal-forest exchange batch (port 0) and rebuild a
//! `CausalForestOutput` (with Ŷ/Ŵ and original Y/W) from it. BLP additionally
//! takes the projection features A on port 1.

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use grf::nodes::{
    AverageTreatmentEffectSpec, BestLinearProjectionSpec, GetScoresSpec, TestCalibrationSpec,
};

use crate::common;

// ── ATE ────────────────────────────────────────────────────────────────

pub struct AverageTreatmentEffectNode {
    spec: AverageTreatmentEffectSpec,
    meta: NodePorts,
}
impl Clone for AverageTreatmentEffectNode {
    fn clone(&self) -> Self { Self { spec: self.spec.clone(), meta: self.meta.clone() } }
}
pub struct AverageTreatmentEffectNodeFactory;
impl NodeFactory for AverageTreatmentEffectNodeFactory {
    fn kind(&self) -> &'static str { "grf_average_treatment_effect" }
    fn desc(&self) -> &'static str { "Estimate the average treatment effect (ATE)." }
    fn doc(&self) -> &'static str { "grf_average_treatment_effect: takes a causal-forest exchange batch (port 0) and returns the ATE estimate, robust SE, and effective sample size." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(AverageTreatmentEffectSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _c: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(AverageTreatmentEffectNode { spec: serde_json::from_value(spec)?, meta: self.ports() }))
    }
}
#[async_trait]
impl DagNode for AverageTreatmentEffectNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "grf_average_treatment_effect" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        let fb = forest_batch(node, inputs).await?;
        let causal = common::decode_causal_forest(node, &fb)?;
        let out = self.spec.estimate(&causal).map_err(|e| dag_err(node, &e.to_string()))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("estimate", DataType::Float64, false),
                Field::new("std_err", DataType::Float64, false),
                Field::new("target_sample", DataType::Utf8, false),
                Field::new("method", DataType::Utf8, false),
                Field::new("n_effective", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![out.estimate])),
                Arc::new(Float64Array::from(vec![out.std_err])),
                Arc::new(arrow_array::StringArray::from(vec![out.target_sample])),
                Arc::new(arrow_array::StringArray::from(vec![out.method])),
                Arc::new(Int64Array::from(vec![out.n_effective as i64])),
            ],
        ).map_err(|e| dag_err(node, &format!("ate batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ── Best Linear Projection ─────────────────────────────────────────────

pub struct BestLinearProjectionNode {
    spec: BestLinearProjectionSpec,
    meta: NodePorts,
}
impl Clone for BestLinearProjectionNode {
    fn clone(&self) -> Self { Self { spec: self.spec.clone(), meta: self.meta.clone() } }
}
pub struct BestLinearProjectionNodeFactory;
impl NodeFactory for BestLinearProjectionNodeFactory {
    fn kind(&self) -> &'static str { "grf_best_linear_projection" }
    fn desc(&self) -> &'static str { "Project CATE onto covariates A (BLP)." }
    fn doc(&self) -> &'static str { "grf_best_linear_projection: takes a causal-forest exchange batch (port 0) and an optional A-feature batch (port 1); returns coefficients, robust SEs, t-stats, and p-values." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(BestLinearProjectionSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_input_port(None).add_output_port(None)
    }
    fn build(&self, spec: serde_json::Value, _c: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(BestLinearProjectionNode { spec: serde_json::from_value(spec)?, meta: self.ports() }))
    }
}
#[async_trait]
impl DagNode for BestLinearProjectionNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "grf_best_linear_projection" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        let fb = forest_batch(node, inputs).await?;
        let a_batches = {
            let ab = common::collect_port(node, inputs, 1).await?;
            if ab.is_empty() { None } else { Some(ab) }
        };
        let causal = common::decode_causal_forest(node, &fb)?;
        let out = self.spec.project(&causal, a_batches.as_deref()).map_err(|e| dag_err(node, &e.to_string()))?;
        let n = out.coefficients.len();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("index", DataType::Int64, false),
                Field::new("coefficient", DataType::Float64, false),
                Field::new("std_error", DataType::Float64, false),
                Field::new("t_stat", DataType::Float64, false),
                Field::new("p_value", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Int64Array::from((0..n).map(|i| i as i64).collect::<Vec<_>>())),
                Arc::new(Float64Array::from(out.coefficients)),
                Arc::new(Float64Array::from(out.std_errors)),
                Arc::new(Float64Array::from(out.t_stats)),
                Arc::new(Float64Array::from(out.p_values)),
            ],
        ).map_err(|e| dag_err(node, &format!("blp batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ── Test Calibration ───────────────────────────────────────────────────

pub struct TestCalibrationNode {
    spec: TestCalibrationSpec,
    meta: NodePorts,
}
impl Clone for TestCalibrationNode {
    fn clone(&self) -> Self { Self { spec: self.spec.clone(), meta: self.meta.clone() } }
}
pub struct TestCalibrationNodeFactory;
impl NodeFactory for TestCalibrationNodeFactory {
    fn kind(&self) -> &'static str { "grf_test_calibration" }
    fn desc(&self) -> &'static str { "Calibration test for a causal forest." }
    fn doc(&self) -> &'static str { "grf_test_calibration: runs the calibration test on a causal-forest exchange batch and reports mean-prediction and differential-prediction coefficients with SEs and p-values." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(TestCalibrationSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _c: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(TestCalibrationNode { spec: serde_json::from_value(spec)?, meta: self.ports() }))
    }
}
#[async_trait]
impl DagNode for TestCalibrationNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "grf_test_calibration" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        let fb = forest_batch(node, inputs).await?;
        let causal = common::decode_causal_forest(node, &fb)?;
        let out = self.spec.check_causal(&causal).map_err(|e| dag_err(node, &e.to_string()))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("mean_pred_coefficient", DataType::Float64, false),
                Field::new("mean_pred_se", DataType::Float64, false),
                Field::new("mean_pred_p_value", DataType::Float64, false),
                Field::new("differential_pred_coefficient", DataType::Float64, false),
                Field::new("differential_pred_se", DataType::Float64, false),
                Field::new("differential_pred_p_value", DataType::Float64, false),
                Field::new("n_obs", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![out.mean_pred_coefficient])),
                Arc::new(Float64Array::from(vec![out.mean_pred_se])),
                Arc::new(Float64Array::from(vec![out.mean_pred_p_value])),
                Arc::new(Float64Array::from(vec![out.differential_pred_coefficient])),
                Arc::new(Float64Array::from(vec![out.differential_pred_se])),
                Arc::new(Float64Array::from(vec![out.differential_pred_p_value])),
                Arc::new(Int64Array::from(vec![out.n_obs as i64])),
            ],
        ).map_err(|e| dag_err(node, &format!("calibration batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ── Get Scores ─────────────────────────────────────────────────────────

pub struct GetScoresNode {
    spec: GetScoresSpec,
    meta: NodePorts,
}
impl Clone for GetScoresNode {
    fn clone(&self) -> Self { Self { spec: self.spec.clone(), meta: self.meta.clone() } }
}
pub struct GetScoresNodeFactory;
impl NodeFactory for GetScoresNodeFactory {
    fn kind(&self) -> &'static str { "grf_get_scores" }
    fn desc(&self) -> &'static str { "Extract doubly-robust scores from a causal forest." }
    fn doc(&self) -> &'static str { "grf_get_scores: computes AIPW scores for a binary treatment from a causal-forest exchange batch and emits a `dr_score` column." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(GetScoresSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _c: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(GetScoresNode { spec: serde_json::from_value(spec)?, meta: self.ports() }))
    }
}
#[async_trait]
impl DagNode for GetScoresNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "grf_get_scores" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        let fb = forest_batch(node, inputs).await?;
        let causal = common::decode_causal_forest(node, &fb)?;
        let out = self.spec.compute(&causal).map_err(|e| dag_err(node, &e.to_string()))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("dr_score", DataType::Float64, false)])),
            vec![Arc::new(Float64Array::from(out.dr_scores))],
        ).map_err(|e| dag_err(node, &format!("scores batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ── shared ─────────────────────────────────────────────────────────────

async fn forest_batch(node: &str, inputs: &[NodeInput]) -> Result<RecordBatch, DagError> {
    let b = common::collect_port(node, inputs, 0).await?;
    b.into_iter().next().ok_or_else(|| dag_err(node, "input port 0 (forest) not connected"))
}

fn dag_err(node: &str, msg: &str) -> DagError {
    DagError::NodeError { node_type: node.into(), msg: msg.to_string() }
}
