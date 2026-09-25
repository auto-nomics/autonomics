//! Model artifact DAG nodes — save/load fitted models.
//!
//! Uses local filesystem for now; opendal integration planned for the
//! full model-artifact framework.

use std::sync::Arc;

use arrow_array::{Array, BinaryArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

mod model_save;
pub use model_save::ModelSaveFactory;

mod model_load;
pub use model_load::ModelLoadFactory;

// ── shared artifact-row helpers (used by save and load) ─────────────────

pub(super) async fn collect_artifact_input(
    inputs: &[NodeInput],
) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_model".into(),
        msg: "no artifact input".into(),
    })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_model".into(),
            msg: format!("collect: {e}"),
        })
}

/// Extract the raw `artifact_bytes` value from a single-row artifact table.
pub(super) fn artifact_row_bytes(batches: &[RecordBatch]) -> Result<Vec<u8>, String> {
    let batch = batches
        .first()
        .ok_or("no artifact input rows".to_string())?;
    if batch.num_rows() != 1 {
        return Err(format!(
            "artifact table must have exactly 1 row, got {}",
            batch.num_rows()
        ));
    }
    let idx = batch
        .schema()
        .index_of("artifact_bytes")
        .map_err(|_| "artifact_bytes column not found".to_string())?;
    let col = batch.column(idx);
    let binary = col
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or("artifact_bytes is not Binary".to_string())?;
    Ok(binary.value(0).to_vec())
}

/// Validate raw bytes as a `ModelArtifact` (kind + feature names readable).
pub(super) fn artifact_from_batch(batches: &[RecordBatch]) -> Result<ml::ModelArtifact, String> {
    let raw = artifact_row_bytes(batches)?;
    ml::ModelArtifact::from_bytes(&raw).map_err(|e| format!("invalid ModelArtifact: {e}"))
}

/// Build the canonical single-row artifact table emitted by save/load.
pub(super) fn artifact_to_batch(
    artifact: &ml::ModelArtifact,
    raw: &[u8],
) -> Result<RecordBatch, String> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("artifact_bytes", DataType::Binary, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("feature_names", DataType::Utf8, false),
            Field::new("training_meta", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(BinaryArray::from(vec![raw])),
            Arc::new(StringArray::from(vec![artifact.kind.clone()])),
            Arc::new(StringArray::from(vec![artifact.feature_names.join(", ")])),
            Arc::new(StringArray::from(vec![artifact.training_meta.clone()])),
        ],
    )
    .map_err(|e| format!("build batch: {e}"))
}
