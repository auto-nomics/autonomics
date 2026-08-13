//! Forest trainer DAG nodes.
//!
//! Every trainer consumes the training data on input port 0, runs the
//! corresponding `grf` spec's `fit()`, and emits a forest-exchange batch on
//! output port 0 plus (where available) the OOB predictions on port 1.

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use grf::forest::OobPredictions;
use grf::nodes::{
    BoostedRegressionForestSpec, CausalForestSpec, CausalSurvivalForestSpec,
    InstrumentalForestSpec, LlRegressionForestSpec, LmForestSpec, MultiArmCausalForestSpec,
    MultiRegressionForestSpec, ProbabilityForestSpec, QuantileForestSpec, RegressionForestSpec,
    SurvivalForestSpec,
};

use crate::common;

// ── shared emission ────────────────────────────────────────────────────

/// Emit a forest-exchange batch (port 0) and optional OOB batch (port 1).
fn emit_forest_out(
    ctx: &NodeCtx,
    node: &str,
    forest_batch: RecordBatch,
    oob: Option<RecordBatch>,
) -> Result<PortOutputs, DagError> {
    match oob {
        Some(oob_batch) => common::emit_two(ctx, node, forest_batch, oob_batch),
        None => common::emit(ctx, node, forest_batch),
    }
}

fn oob_batch(node: &str, oob: Option<&OobPredictions>) -> Result<Option<RecordBatch>, DagError> {
    match oob {
        Some(o) => common::oob_to_batch(node, o).map(Some),
        None => Ok(None),
    }
}

/// A two-output-port layout (forest + OOB) shared by trainers with OOB.
fn ports_two() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(None)
        .add_output_port(None)
}
/// A single-output-port layout for trainers that produce only a forest.
fn ports_one() -> NodePorts {
    NodePorts::new().add_input_port(None).add_output_port(None)
}

// ═══════════════════════════════════════════════════════════════════════
// Regression
// ═══════════════════════════════════════════════════════════════════════

pub struct RegressionForestNode {
    spec: RegressionForestSpec,
    meta: NodePorts,
}
pub struct RegressionNodeFactory;
impl NodeFactory for RegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_regression_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a generalized random forest for regression."
    }
    fn doc(&self) -> &'static str {
        "grf_regression_forest: fits a regression forest (grf::regression_forest) on the training data and emits a forest-exchange batch (port 0) plus OOB predictions (port 1)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RegressionForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(RegressionForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for RegressionForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_regression_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for RegressionForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Causal (carries aux vectors for downstream analysis)
// ═══════════════════════════════════════════════════════════════════════

pub struct CausalForestNode {
    spec: CausalForestSpec,
    meta: NodePorts,
}
pub struct CausalNodeFactory;
impl NodeFactory for CausalNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_causal_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a causal forest (R-learner)."
    }
    fn doc(&self) -> &'static str {
        "grf_causal_forest: fits a causal forest via the R-learner and emits a causal-forest exchange batch carrying the forest plus Ŷ/Ŵ and original Y/W for downstream analysis (ATE, BLP, calibration, scores)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CausalForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(CausalForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for CausalForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_causal_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_causal_forest(&out)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for CausalForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Quantile
// ═══════════════════════════════════════════════════════════════════════

pub struct QuantileForestNode {
    spec: QuantileForestSpec,
    meta: NodePorts,
}
pub struct QuantileNodeFactory;
impl NodeFactory for QuantileNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_quantile_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a quantile forest."
    }
    fn doc(&self) -> &'static str {
        "grf_quantile_forest: fits a quantile regression forest for the requested quantiles."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(QuantileForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_one()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(QuantileForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for QuantileForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_quantile_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        emit_forest_out(ctx, node, fb, None)
    }
}
impl Clone for QuantileForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Probability
// ═══════════════════════════════════════════════════════════════════════

pub struct ProbabilityForestNode {
    spec: ProbabilityForestSpec,
    meta: NodePorts,
}
pub struct ProbabilityNodeFactory;
impl NodeFactory for ProbabilityNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_probability_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a probability forest (classification)."
    }
    fn doc(&self) -> &'static str {
        "grf_probability_forest: fits a probability forest for categorical outcomes; port 1 carries per-class OOB probabilities."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProbabilityForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(ProbabilityForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for ProbabilityForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_probability_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for ProbabilityForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Survival
// ═══════════════════════════════════════════════════════════════════════

pub struct SurvivalForestNode {
    spec: SurvivalForestSpec,
    meta: NodePorts,
}
pub struct SurvivalNodeFactory;
impl NodeFactory for SurvivalNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_survival_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a survival forest (right-censored)."
    }
    fn doc(&self) -> &'static str {
        "grf_survival_forest: fits a survival forest; port 1 carries OOB survival probabilities per failure time."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SurvivalForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(SurvivalForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for SurvivalForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_survival_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for SurvivalForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Multi-Regression
// ═══════════════════════════════════════════════════════════════════════

pub struct MultiRegressionForestNode {
    spec: MultiRegressionForestSpec,
    meta: NodePorts,
}
pub struct MultiRegressionNodeFactory;
impl NodeFactory for MultiRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_multi_regression_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a multi-task regression forest."
    }
    fn doc(&self) -> &'static str {
        "grf_multi_regression_forest: fits a forest with multiple outcome columns; port 1 carries per-task OOB predictions."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MultiRegressionForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(MultiRegressionForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for MultiRegressionForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_multi_regression_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for MultiRegressionForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Instrumental
// ═══════════════════════════════════════════════════════════════════════

pub struct InstrumentalForestNode {
    spec: InstrumentalForestSpec,
    meta: NodePorts,
}
pub struct InstrumentalNodeFactory;
impl NodeFactory for InstrumentalNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_instrumental_forest"
    }
    fn desc(&self) -> &'static str {
        "Train an instrumental-forest."
    }
    fn doc(&self) -> &'static str {
        "grf_instrumental_forest: fits an IV forest (Y, W treatment, Z instrument)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(InstrumentalForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(InstrumentalForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for InstrumentalForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_instrumental_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for InstrumentalForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// LM forest
// ═══════════════════════════════════════════════════════════════════════

pub struct LmForestNode {
    spec: LmForestSpec,
    meta: NodePorts,
}
pub struct LmNodeFactory;
impl NodeFactory for LmNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_lm_forest"
    }
    fn desc(&self) -> &'static str {
        "Train an LM forest."
    }
    fn doc(&self) -> &'static str {
        "grf_lm_forest: fits a linear-model forest with outcome columns Y and regressor columns W."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LmForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(LmForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for LmForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_lm_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for LmForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// LL Regression
// ═══════════════════════════════════════════════════════════════════════

pub struct LlRegressionForestNode {
    spec: LlRegressionForestSpec,
    meta: NodePorts,
}
pub struct LlRegressionNodeFactory;
impl NodeFactory for LlRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_ll_regression_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a local-linear regression forest."
    }
    fn doc(&self) -> &'static str {
        "grf_ll_regression_forest: fits a local-linear regression forest."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LlRegressionForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_one()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(LlRegressionForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for LlRegressionForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_ll_regression_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        emit_forest_out(ctx, node, fb, None)
    }
}
impl Clone for LlRegressionForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Boosted Regression
// ═══════════════════════════════════════════════════════════════════════

pub struct BoostedRegressionForestNode {
    spec: BoostedRegressionForestSpec,
    meta: NodePorts,
}
pub struct BoostedRegressionNodeFactory;
impl NodeFactory for BoostedRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_boosted_regression_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a boosted regression forest."
    }
    fn doc(&self) -> &'static str {
        "grf_boosted_regression_forest: fits an ensemble of regression forests. Port 0 carries the last forest blob; port 1 carries OOB predictions (single pred column); port 2 carries the OOB error scalar."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BoostedRegressionForestSpec)
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
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(BoostedRegressionForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for BoostedRegressionForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_boosted_regression_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        // Transport the last forest blob for analysis/metadata; the summed OOB
        // predictions and error are emitted on ports 1/2.
        let last = out
            .forests
            .last()
            .ok_or_else(|| dag_err(node, "boosted forest produced no forests"))?;
        let fb = common::encode_forest(last)?;
        let oob_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "pred_0",
                DataType::Float64,
                false,
            )])),
            vec![Arc::new(Float64Array::from(out.oob_predictions.clone()))],
        )
        .map_err(|e| dag_err(node, &format!("oob batch: {e}")))?;
        let err_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "oob_error",
                DataType::Float64,
                false,
            )])),
            vec![Arc::new(Float64Array::from(vec![out.oob_error]))],
        )
        .map_err(|e| dag_err(node, &format!("error batch: {e}")))?;
        let df0 = ctx
            .session()
            .read_batch(fb)
            .map_err(|e| dag_err(node, &format!("read_batch(0): {e}")))?;
        let df1 = ctx
            .session()
            .read_batch(oob_batch)
            .map_err(|e| dag_err(node, &format!("read_batch(1): {e}")))?;
        let df2 = ctx
            .session()
            .read_batch(err_batch)
            .map_err(|e| dag_err(node, &format!("read_batch(2): {e}")))?;
        let mut res = PortOutputs::new();
        res.insert(0, df0);
        res.insert(1, df1);
        res.insert(2, df2);
        Ok(res)
    }
}
impl Clone for BoostedRegressionForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Multi-Arm Causal
// ═══════════════════════════════════════════════════════════════════════

pub struct MultiArmCausalForestNode {
    spec: MultiArmCausalForestSpec,
    meta: NodePorts,
}
pub struct MultiArmCausalNodeFactory;
impl NodeFactory for MultiArmCausalNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_multi_arm_causal_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a multi-arm causal forest."
    }
    fn doc(&self) -> &'static str {
        "grf_multi_arm_causal_forest: fits a causal forest with K-armed treatment."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MultiArmCausalForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_two()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(MultiArmCausalForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for MultiArmCausalForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_multi_arm_causal_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        let oob = oob_batch(node, out.oob_predictions.as_ref())?;
        emit_forest_out(ctx, node, fb, oob)
    }
}
impl Clone for MultiArmCausalForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Causal Survival
// ═══════════════════════════════════════════════════════════════════════

pub struct CausalSurvivalForestNode {
    spec: CausalSurvivalForestSpec,
    meta: NodePorts,
}
pub struct CausalSurvivalNodeFactory;
impl NodeFactory for CausalSurvivalNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_causal_survival_forest"
    }
    fn desc(&self) -> &'static str {
        "Train a causal survival forest."
    }
    fn doc(&self) -> &'static str {
        "grf_causal_survival_forest: fits a causal forest for survival outcomes (RMST or survival-probability target)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CausalSurvivalForestSpec)
    }
    fn ports(&self) -> NodePorts {
        ports_one()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(CausalSurvivalForestNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for CausalSurvivalForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_causal_survival_forest"
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
        let node = self.kind();
        let batches = common::collect_all(node, inputs).await?;
        let out = self
            .spec
            .fit(&batches)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let fb = common::encode_forest(&out.forest)?;
        emit_forest_out(ctx, node, fb, None)
    }
}
impl Clone for CausalSurvivalForestNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}

// ── shared node-type error helper ─────────────────────────────────────

fn dag_err(node: &str, msg: &str) -> DagError {
    DagError::NodeError {
        node_type: node.into(),
        msg: msg.to_string(),
    }
}
