use super::*;

// ═══════════════════════════════════════════════════════════════════════
// OneHotEncode
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OneHotEncodeSpec {
    pub column: String,
    #[serde(default)]
    pub categories: Option<Vec<String>>,
}

pub struct OneHotEncodeFactory;
impl NodeFactory for OneHotEncodeFactory {
    fn kind(&self) -> &'static str {
        "ml_one_hot"
    }
    fn desc(&self) -> &'static str {
        "One-hot encode a categorical column into binary indicator columns."
    }
    fn doc(&self) -> &'static str {
        "OneHotEncoder: expands a categorical column into one binary column per category. If categories are not specified, they are inferred from the data."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OneHotEncodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: OneHotEncodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OneHotEncodeNode {
            column: s.column,
            categories: s.categories,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct OneHotEncodeNode {
    column: String,
    categories: Option<Vec<String>>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for OneHotEncodeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_one_hot"
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

        // Determine categories
        let categories: Vec<String> = match &self.categories {
            Some(c) => c.clone(),
            None => {
                let mut seen: Vec<String> = values.to_vec();
                seen.sort();
                seen.dedup();
                seen
            }
        };

        // Build indicator columns
        let mut fields: Vec<Arc<Field>> = Vec::new();
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        // Re-emit all original columns except the encoded one
        let schema = batches.first().unwrap().schema();
        for (i, f) in schema.fields().iter().enumerate() {
            if f.name() == &self.column {
                continue;
            }
            fields.push(f.clone());
            let mut col_chunks: Vec<Arc<dyn Array>> = Vec::new();
            for batch in &batches {
                col_chunks.push(batch.column(i).clone());
            }
            // Concatenate chunks
            arrays.push(col_chunks.into_iter().next().unwrap());
        }

        // Add one-hot columns
        for cat in &categories {
            let indicator: Vec<f64> = values
                .iter()
                .map(|v| if v.as_str() == cat.as_str() { 1.0 } else { 0.0 })
                .collect();
            let col_name = format!("{}_{}", self.column, cat);
            fields.push(Arc::new(Field::new(&col_name, DataType::Float64, true)));
            arrays.push(Arc::new(Float64Array::from(indicator)));
        }

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_one_hot".into(),
                msg: e.to_string(),
            }
        })?;
        emit_batch(ctx, batch)
    }
}
