use super::*;

// ═══════════════════════════════════════════════════════════════════════
// ModelLoad — read a persisted ModelArtifact back onto an edge
// ═══════════════════════════════════════════════════════════════════════
//
// Emits the canonical single-row artifact table (artifact_bytes + kind +
// human-readable metadata) on port 0.  `ml_frozen_predict` consumes only
// `artifact_bytes`; the extra columns make the frozen model inspectable
// at DAG-view time.

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
        "Load a fitted-model artifact from storage."
    }
    fn doc(&self) -> &'static str {
        "ml_model_load: reads a bincode-serialised ModelArtifact from the specified URI and \
        emits it as a single-row artifact table (artifact_bytes Binary + kind + metadata) \
        that prediction nodes can consume."
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
        let raw = std::fs::read(&self.uri).map_err(|e| DagError::NodeError {
            node_type: "ml_model_load".into(),
            msg: format!("fs read: {e}"),
        })?;
        let artifact = ml::ModelArtifact::from_bytes(&raw).map_err(|e| DagError::NodeError {
            node_type: "ml_model_load".into(),
            msg: format!("invalid ModelArtifact: {e}"),
        })?;

        let batch = artifact_to_batch(&artifact, &raw).map_err(|e| DagError::NodeError {
            node_type: "ml_model_load".into(),
            msg: e,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_deserialize() {
        let spec: ModelLoadSpec = serde_json::from_str(r#"{"uri": "/tmp/m.bin"}"#).unwrap();
        assert_eq!(spec.uri, "/tmp/m.bin");
    }
}
