use super::*;

// ═══════════════════════════════════════════════════════════════════════
// ModelSave
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModelSaveSpec {
    pub uri: String,
    pub kind: String,
    #[serde(default)]
    pub feature_names: Vec<String>,
    #[serde(default)]
    pub training_meta: serde_json::Value,
}

pub struct ModelSaveFactory;
impl NodeFactory for ModelSaveFactory {
    fn kind(&self) -> &'static str {
        "ml_model_save"
    }
    fn desc(&self) -> &'static str {
        "Save a model artifact to storage."
    }
    fn doc(&self) -> &'static str {
        "ModelSave: wraps raw model bytes in a ModelArtifact with metadata and writes to the specified URI (local path for now)."
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
            kind: s.kind,
            feature_names: s.feature_names,
            training_meta: s.training_meta,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ModelSaveNode {
    uri: String,
    kind: String,
    feature_names: Vec<String>,
    training_meta: serde_json::Value,
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
        "ml_model_save"
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
        let artifact = ml::ModelArtifact::new(
            &self.kind,
            &Vec::<u8>::new(),
            self.feature_names.clone(),
            self.training_meta.clone(),
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: e.to_string(),
        })?;

        let bytes = artifact.to_bytes().map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: e.to_string(),
        })?;

        std::fs::write(&self.uri, &bytes).map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: format!("fs write: {e}"),
        })?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "model_uri",
                DataType::Utf8,
                false,
            )])),
            vec![Arc::new(StringArray::from(vec![self.uri.clone()]))],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: e.to_string(),
        })?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::NodeError {
                node_type: "ml_model_save".into(),
                msg: format!("read_batch: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
