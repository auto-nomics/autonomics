use super::*;

// ═══════════════════════════════════════════════════════════════════════
// ModelSave — persist an artifact row's bytes to storage
// ═══════════════════════════════════════════════════════════════════════
//
// Port-0 input is a single-row artifact table produced by a fit node
// (`artifact_bytes` Binary column + metadata columns).  The bytes are
// validated as a `ModelArtifact` and written verbatim to `uri`; port 0
// passes the artifact through so save can sit inline on an edge.

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModelSaveSpec {
    /// Local path the bincode `ModelArtifact` will be written to.
    pub uri: String,
}

pub struct ModelSaveFactory;
impl NodeFactory for ModelSaveFactory {
    fn kind(&self) -> &'static str {
        "ml_model_save"
    }
    fn desc(&self) -> &'static str {
        "Persist a fitted-model artifact row to storage."
    }
    fn doc(&self) -> &'static str {
        "ml_model_save: reads a single-row artifact table (artifact_bytes Binary column, \
        as emitted by fit nodes or ml_model_load), validates it as a ModelArtifact, writes \
        the bytes to the specified URI, and passes the artifact through on port 0."
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
        "ml_model_save"
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
        let batches = collect_artifact_input(inputs).await?;
        let artifact = artifact_from_batch(&batches).map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: e,
        })?;
        let raw = artifact_row_bytes(&batches).map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: e,
        })?;

        std::fs::write(&self.uri, &raw).map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: format!("fs write: {e}"),
        })?;

        let batch = artifact_to_batch(&artifact, &raw).map_err(|e| DagError::NodeError {
            node_type: "ml_model_save".into(),
            msg: e,
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

// ── shared artifact-row helpers live in model_nodes.rs (mod head) ───────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_deserialize() {
        let spec: ModelSaveSpec = serde_json::from_str(r#"{"uri": "/tmp/m.bin"}"#).unwrap();
        assert_eq!(spec.uri, "/tmp/m.bin");
    }

    #[test]
    fn test_artifact_row_roundtrip() {
        let artifact = ml::ModelArtifact::new(
            "pam:v1",
            &vec![1.0_f64, 2.0],
            vec!["a".into(), "b".into()],
            serde_json::json!({"delta": 2.5}),
        )
        .unwrap();
        let raw = artifact.to_bytes().unwrap();
        let batch = artifact_to_batch(&artifact, &raw).unwrap();
        let back = artifact_from_batch(&[batch]).unwrap();
        assert_eq!(back.kind, "pam:v1");
        assert_eq!(back.feature_names, vec!["a", "b"]);
        assert_eq!(
            back.deserialize_fitted::<Vec<f64>>().unwrap(),
            vec![1.0, 2.0]
        );
    }

    #[test]
    fn test_rejects_multi_row() {
        let artifact = ml::ModelArtifact::new(
            "pam:v1",
            &vec![0.0_f64],
            vec!["a".into()],
            serde_json::json!({}),
        )
        .unwrap();
        let raw = artifact.to_bytes().unwrap();
        let single = artifact_to_batch(&artifact, &raw).unwrap();
        let doubled = RecordBatch::try_new(
            single.schema(),
            vec![
                Arc::new(BinaryArray::from(vec![raw.as_slice(), raw.as_slice()])),
                Arc::new(StringArray::from(vec!["pam:v1", "pam:v1"])),
                Arc::new(StringArray::from(vec!["a", "a"])),
                Arc::new(StringArray::from(vec!["{}", "{}"])),
            ],
        )
        .unwrap();
        assert!(artifact_row_bytes(&[doubled]).is_err());
    }
}
