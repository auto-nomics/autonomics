//! Forest-analysis DAG nodes — weights, split frequencies, variable importance.

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
    GetForestWeightsSpec, GetTreeSpec, MergeForestsSpec, SplitFrequenciesSpec,
    VariableImportanceSpec,
};

use crate::common;
use crate::common::{FOREST_BYTES, FOREST_KIND, N_FEATURES};

// ═══════════════════════════════════════════════════════════════════════
// Get Forest Weights
// ═══════════════════════════════════════════════════════════════════════

pub struct GetForestWeightsNode {
    spec: GetForestWeightsSpec,
    meta: NodePorts,
}
impl Clone for GetForestWeightsNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}
pub struct GetForestWeightsNodeFactory;
impl NodeFactory for GetForestWeightsNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_get_forest_weights"
    }
    fn desc(&self) -> &'static str {
        "Compute forest weights for new data."
    }
    fn doc(&self) -> &'static str {
        "grf_get_forest_weights: port 0 = forest, port 1 = train X, port 2 = test X. Emits the α(test, train) weights matrix as a long (test, train, weight) table."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GetForestWeightsSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_input_port(None)
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(GetForestWeightsNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for GetForestWeightsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_get_forest_weights"
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
        let fb = common::collect_port(node, inputs, 0)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| dag_err(node, "input port 0 (forest) not connected"))?;
        let train_b = common::collect_port(node, inputs, 1).await?;
        let test_b = common::collect_port(node, inputs, 2).await?;
        let forest = common::decode_forest(node, &fb)?;
        let (train_x, _) = common::batch_to_matrix(node, &train_b)?;
        let (test_x, _) = common::batch_to_matrix(node, &test_b)?;
        let out = self
            .spec
            .compute(&forest, train_x, test_x)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        // weights is column-major (n_train × n_test): emit long (test, train, weight).
        let mut t_idx: Vec<i64> = Vec::with_capacity(out.weights.len());
        let mut r_idx: Vec<i64> = Vec::with_capacity(out.weights.len());
        for t in 0..out.n_test {
            for r in 0..out.n_train {
                t_idx.push(t as i64);
                r_idx.push(r as i64);
            }
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("test_idx", DataType::Int64, false),
                Field::new("train_idx", DataType::Int64, false),
                Field::new("weight", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(t_idx)),
                Arc::new(Int64Array::from(r_idx)),
                Arc::new(Float64Array::from(out.weights)),
            ],
        )
        .map_err(|e| dag_err(node, &format!("weights batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Split Frequencies
// ═══════════════════════════════════════════════════════════════════════

pub struct SplitFrequenciesNode {
    spec: SplitFrequenciesSpec,
    meta: NodePorts,
}
impl Clone for SplitFrequenciesNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}
pub struct SplitFrequenciesNodeFactory;
impl NodeFactory for SplitFrequenciesNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_split_frequencies"
    }
    fn desc(&self) -> &'static str {
        "Split-frequency matrix of a forest."
    }
    fn doc(&self) -> &'static str {
        "grf_split_frequencies: port 0 = forest. Emits (depth, feature) split counts as a long table."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SplitFrequenciesSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(SplitFrequenciesNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for SplitFrequenciesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_split_frequencies"
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
        let fb = common::collect_port(node, inputs, 0)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| dag_err(node, "input port 0 (forest) not connected"))?;
        let forest = common::decode_forest(node, &fb)?;
        let out = self
            .spec
            .compute(&forest)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let mut depth: Vec<i64> = Vec::new();
        let mut feature: Vec<i64> = Vec::new();
        let mut count: Vec<i64> = Vec::new();
        for (d, row) in out.depths_x_features.iter().enumerate() {
            for (f, c) in row.iter().enumerate() {
                depth.push(d as i64);
                feature.push(f as i64);
                count.push(*c as i64);
            }
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("depth", DataType::Int64, false),
                Field::new("feature", DataType::Int64, false),
                Field::new("count", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(depth)),
                Arc::new(Int64Array::from(feature)),
                Arc::new(Int64Array::from(count)),
            ],
        )
        .map_err(|e| dag_err(node, &format!("splitfreq batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Variable Importance
// ═══════════════════════════════════════════════════════════════════════

pub struct VariableImportanceNode {
    spec: VariableImportanceSpec,
    meta: NodePorts,
}
impl Clone for VariableImportanceNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}
pub struct VariableImportanceNodeFactory;
impl NodeFactory for VariableImportanceNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_variable_importance"
    }
    fn desc(&self) -> &'static str {
        "Variable-importance scores of a forest."
    }
    fn doc(&self) -> &'static str {
        "grf_variable_importance: port 0 = forest. Emits one `importance` row per feature."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(VariableImportanceSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(VariableImportanceNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for VariableImportanceNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_variable_importance"
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
        let fb = common::collect_port(node, inputs, 0)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| dag_err(node, "input port 0 (forest) not connected"))?;
        let forest = common::decode_forest(node, &fb)?;
        let out = self
            .spec
            .compute(&forest)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let n = out.importance.len();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Int64, false),
                Field::new("importance", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(
                    (0..n).map(|i| i as i64).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(out.importance)),
            ],
        )
        .map_err(|e| dag_err(node, &format!("importance batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Get Tree
// ═══════════════════════════════════════════════════════════════════════

pub struct GetTreeNode {
    spec: GetTreeSpec,
    meta: NodePorts,
}
impl Clone for GetTreeNode {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            meta: self.meta.clone(),
        }
    }
}
pub struct GetTreeNodeFactory;
impl NodeFactory for GetTreeNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_get_tree"
    }
    fn desc(&self) -> &'static str {
        "Extract a single tree (serialized)."
    }
    fn doc(&self) -> &'static str {
        "grf_get_tree: port 0 = forest, spec.index selects the tree. Emits a single-row `tree` binary column."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GetTreeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        Ok(Box::new(GetTreeNode {
            spec: serde_json::from_value(spec)?,
            meta: self.ports(),
        }))
    }
}
#[async_trait]
impl DagNode for GetTreeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_get_tree"
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
        let fb = common::collect_port(node, inputs, 0)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| dag_err(node, "input port 0 (forest) not connected"))?;
        let forest = common::decode_forest(node, &fb)?;
        let out = self
            .spec
            .extract(&forest)
            .map_err(|e| dag_err(node, &e.to_string()))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "tree",
                DataType::Binary,
                false,
            )])),
            vec![Arc::new(arrow_array::BinaryArray::from(vec![Some(
                out.serialized.as_slice(),
            )]))],
        )
        .map_err(|e| dag_err(node, &format!("tree batch: {e}")))?;
        common::emit(ctx, node, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Merge Forests
// ═══════════════════════════════════════════════════════════════════════

/// Merges up to 8 forest-exchange batches (ports 0..8) into one forest.
pub struct MergeForestsNode {
    meta: NodePorts,
}
impl Clone for MergeForestsNode {
    fn clone(&self) -> Self {
        Self {
            meta: self.meta.clone(),
        }
    }
}
pub struct MergeForestsNodeFactory;
impl NodeFactory for MergeForestsNodeFactory {
    fn kind(&self) -> &'static str {
        "grf_merge_forests"
    }
    fn desc(&self) -> &'static str {
        "Merge multiple forests into one."
    }
    fn doc(&self) -> &'static str {
        "grf_merge_forests: ports 0..7 = forest-exchange batches (same kind). Concatenates their trees and emits a single forest-exchange batch on port 0."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MergeForestsSpec)
    }
    fn ports(&self) -> NodePorts {
        let mut p = NodePorts::new();
        for _ in 0..8 {
            p = p.add_input_port(None);
        }
        p.add_output_port(None)
    }
    fn build(
        &self,
        _spec: serde_json::Value,
        _c: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        // The spec's `forests` field is filled from the connected ports at
        // execute time; kind/n_features are read from the first batch.
        Ok(Box::new(MergeForestsNode { meta: self.ports() }))
    }
}
#[async_trait]
impl DagNode for MergeForestsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "grf_merge_forests"
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
        if batches.is_empty() {
            return Err(dag_err(node, "no forest batches to merge"));
        }
        // Read kind + n_features from the first batch; gather all serialized blobs.
        let first = common::get_string(node, &batches[0], FOREST_KIND)?.to_string();
        let n_features = common::get_i32(node, &batches[0], N_FEATURES)? as usize;
        let mut forests = Vec::with_capacity(batches.len());
        for b in &batches {
            forests.push(common::get_binary(node, b, FOREST_BYTES)?.to_vec());
        }
        let spec = MergeForestsSpec {
            forests,
            kind: first,
            n_features,
        };
        let out = spec.merge().map_err(|e| dag_err(node, &e.to_string()))?;
        let batch = common::encode_forest(&out.forest)?;
        common::emit(ctx, node, batch)
    }
}

fn dag_err(node: &str, msg: &str) -> DagError {
    DagError::NodeError {
        node_type: node.into(),
        msg: msg.to_string(),
    }
}
