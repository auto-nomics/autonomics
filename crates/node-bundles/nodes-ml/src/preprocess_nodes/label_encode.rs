use super::*;

// ═══════════════════════════════════════════════════════════════════════
// LabelEncode
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LabelEncodeSpec {
    pub column: String,
}

pub struct LabelEncodeFactory;
impl NodeFactory for LabelEncodeFactory {
    fn kind(&self) -> &'static str {
        "ml_label_encode"
    }
    fn desc(&self) -> &'static str {
        "Encode a categorical column as integer labels [0, n_classes)."
    }
    fn doc(&self) -> &'static str {
        "LabelEncoder: maps each unique value in a categorical column to an integer starting from 0."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LabelEncodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LabelEncodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LabelEncodeNode {
            column: s.column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LabelEncodeNode {
    column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LabelEncodeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_label_encode"
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
        let values = common::extract_string_column(&batches, &self.column)?;
        // Build label map
        let mut sorted_cats: Vec<String> = values.to_vec();
        sorted_cats.sort();
        sorted_cats.dedup();
        let map: std::collections::HashMap<&String, u32> = sorted_cats
            .iter()
            .enumerate()
            .map(|(i, c)| (c, i as u32))
            .collect();
        let encoded: Vec<u32> = values.iter().map(|v| *map.get(v).unwrap_or(&0)).collect();

        let (schema, mut fields, concat_arrays) = common::concat_input(&batches)?;
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();
        for (i, f) in schema.fields().iter().enumerate() {
            if f.name() == &self.column {
                fields[i] = Arc::new(Field::new(&self.column, DataType::UInt32, true));
                arrays.push(Arc::new(UInt32Array::from(encoded.clone())));
            } else {
                arrays.push(concat_arrays[i].clone());
            }
        }
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_label_encode".into(),
                msg: e.to_string(),
            }
        })?;
        emit_batch(ctx, batch)
    }
}
