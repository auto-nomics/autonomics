use super::*;

// ═══════════════════════════════════════════════════════════════════════
// ModelLoad
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModelLoadSpec {
    pub uri: String,
}

pub struct ModelLoadFactory;
impl NodeFactory for ModelLoadFactory {
    fn kind(&self) -> &'static str {
        "ml_model_load"
    }
    fn desc(&self) -> &'static str {
        "Load a model artifact from storage."
    }
    fn doc(&self) -> &'static str {
        "ModelLoad: reads a bincode-serialised ModelArtifact and returns its metadata."
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
        "ml_model_load"
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
        let bytes = std::fs::read(&self.uri).map_err(|e| DagError::NodeError {
            node_type: "ml_model_load".into(),
            msg: format!("fs read: {e}"),
        })?;
        let artifact = ml::ModelArtifact::from_bytes(&bytes).map_err(|e| DagError::NodeError {
            node_type: "ml_model_load".into(),
            msg: e.to_string(),
        })?;

        let feature_names = artifact.feature_names.join(", ");
        let training_meta = serde_json::to_string(&artifact.training_meta).unwrap_or_default();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("model_uri", DataType::Utf8, false),
                Field::new("kind", DataType::Utf8, false),
                Field::new("feature_names", DataType::Utf8, false),
                Field::new("training_meta", DataType::Utf8, false),
                Field::new("n_bytes", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![self.uri.clone()])),
                Arc::new(StringArray::from(vec![artifact.kind.clone()])),
                Arc::new(StringArray::from(vec![feature_names])),
                Arc::new(StringArray::from(vec![training_meta])),
                Arc::new(StringArray::from(vec![format!(
                    "{}",
                    artifact.fitted.len()
                )])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_model_load".into(),
            msg: e.to_string(),
        })?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::NodeError {
                node_type: "ml_model_load".into(),
                msg: format!("read_batch: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
