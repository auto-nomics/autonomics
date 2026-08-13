//! `dl_model_save`, `dl_model_load`, `dl_model_info`.

use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::common;
use dl::DLModelArtifact;

// ═══════════════════════════════════════════════════════════════════════
// dl_model_save
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModelSaveSpec {
    pub uri: String,
}

pub struct ModelSaveFactory;
impl NodeFactory for ModelSaveFactory {
    fn kind(&self) -> &'static str {
        "dl_model_save"
    }
    fn desc(&self) -> &'static str {
        "Save a DL model artifact to storage."
    }
    fn doc(&self) -> &'static str {
        "dl_model_save: writes a DLModelArtifact to the specified URI (local path). \
        Passes the artifact through on port 0 for chaining."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ModelSaveSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ModelSaveSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ModelSaveNode {
            uri: s.uri,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ModelSaveNode {
    uri: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ModelSaveNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "dl_model_save"
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
        let batches = common::collect_batches(inputs, "dl_model_save").await?;
        let artifact = extract_artifact_from_port(&batches)?;
        let bytes = serde_json::to_vec(&artifact)
            .map_err(|e| common::err("dl_model_save", format!("serialize: {e}")))?;
        std::fs::write(&self.uri, &bytes)
            .map_err(|e| common::err("dl_model_save", format!("fs write: {e}")))?;

        // Pass through the artifact on port 0.
        let artifact_bytes = serde_json::to_vec(&artifact)
            .map_err(|e| common::err("dl_model_save", format!("re-serialize: {e}")))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("artifact_bytes", DataType::Binary, false),
                Field::new("architecture", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(arrow_array::BinaryArray::from(vec![
                    artifact_bytes.as_slice(),
                ])),
                Arc::new(StringArray::from(vec![artifact.architecture.as_str()])),
            ],
        )
        .map_err(|e| common::err("dl_model_save", format!("build batch: {e}")))?;

        common::emit_batch(ctx, batch, "dl_model_save")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_model_load
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModelLoadSpec {
    pub uri: String,
}

pub struct ModelLoadFactory;
impl NodeFactory for ModelLoadFactory {
    fn kind(&self) -> &'static str {
        "dl_model_load"
    }
    fn desc(&self) -> &'static str {
        "Load a DL model artifact from storage."
    }
    fn doc(&self) -> &'static str {
        "dl_model_load: reads a serialised DLModelArtifact from the specified URI. \
        The artifact can then be fed to dl_predict for inference."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ModelLoadSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ModelLoadSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ModelLoadNode {
            uri: s.uri,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ModelLoadNode {
    uri: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ModelLoadNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "dl_model_load"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let bytes = std::fs::read(&self.uri)
            .map_err(|e| common::err("dl_model_load", format!("fs read: {e}")))?;
        let artifact: DLModelArtifact = serde_json::from_slice(&bytes)
            .map_err(|e| common::err("dl_model_load", format!("deserialize: {e}")))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("artifact_bytes", DataType::Binary, false),
                Field::new("architecture", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(arrow_array::BinaryArray::from(vec![bytes.as_slice()])),
                Arc::new(StringArray::from(vec![artifact.architecture.as_str()])),
            ],
        )
        .map_err(|e| common::err("dl_model_load", format!("build batch: {e}")))?;

        common::emit_batch(ctx, batch, "dl_model_load")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_model_info
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModelInfoSpec {
    #[serde(default = "d_true")]
    pub include_architecture: bool,
    #[serde(default = "d_true")]
    pub include_training_meta: bool,
}

fn d_true() -> bool {
    true
}

pub struct ModelInfoFactory;
impl NodeFactory for ModelInfoFactory {
    fn kind(&self) -> &'static str {
        "dl_model_info"
    }
    fn desc(&self) -> &'static str {
        "Report DL model metadata."
    }
    fn doc(&self) -> &'static str {
        "dl_model_info: extracts and reports metadata from a DLModelArtifact: architecture type, \
        task type, feature names, total parameters, training history."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ModelInfoSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ModelInfoSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ModelInfoNode {
            spec: s,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ModelInfoNode {
    spec: ModelInfoSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ModelInfoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "dl_model_info"
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
        let batches = common::collect_batches(inputs, "dl_model_info").await?;
        let artifact = extract_artifact_from_port(&batches)?;

        let feature_names = artifact.feature_names.join(", ");
        let total_params = artifact.training_meta.total_params;
        let n_epochs = artifact.training_meta.n_epochs_run;
        let best_epoch = artifact
            .training_meta
            .best_epoch
            .map(|e| e as u64)
            .unwrap_or(0);
        let best_metric = artifact.training_meta.best_val_metric.unwrap_or(f64::NAN);

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("backend", DataType::Utf8, false),
                Field::new("architecture", DataType::Utf8, false),
                Field::new("task_type", DataType::Utf8, false),
                Field::new("feature_names", DataType::Utf8, false),
                Field::new("total_params", DataType::UInt64, false),
                Field::new("n_epochs_run", DataType::UInt64, false),
                Field::new("best_epoch", DataType::UInt64, false),
                Field::new("best_val_metric", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![artifact.backend.clone()])),
                Arc::new(StringArray::from(vec![
                    artifact.architecture.as_str().to_string(),
                ])),
                Arc::new(StringArray::from(vec![format!("{:?}", artifact.task_type)])),
                Arc::new(StringArray::from(vec![feature_names])),
                Arc::new(UInt64Array::from(vec![total_params as u64])),
                Arc::new(UInt64Array::from(vec![n_epochs as u64])),
                Arc::new(UInt64Array::from(vec![best_epoch])),
                Arc::new(arrow_array::Float64Array::from(vec![best_metric])),
            ],
        )
        .map_err(|e| common::err("dl_model_info", format!("build batch: {e}")))?;

        common::emit_batch(ctx, batch, "dl_model_info")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shared helper
// ═══════════════════════════════════════════════════════════════════════

fn extract_artifact_from_port(batches: &[RecordBatch]) -> Result<DLModelArtifact, DagError> {
    let batch = batches
        .first()
        .ok_or(common::err("dl_model", "no artifact input"))?;
    let idx = batch
        .schema()
        .index_of("artifact_bytes")
        .map_err(|_| common::err("dl_model", "artifact_bytes column not found"))?;
    let col = batch.column(idx);
    let binary_col = col
        .as_any()
        .downcast_ref::<arrow_array::BinaryArray>()
        .ok_or_else(|| common::err("dl_model", "artifact_bytes is not Binary"))?;
    let bytes = binary_col.value(0);
    serde_json::from_slice(bytes).map_err(|e| common::err("dl_model", format!("deserialize: {e}")))
}
