use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Gaussian Naive Bayes
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GaussianNbSpec {
    pub features: Vec<String>,
    pub label_column: String,
}

pub struct GaussianNbFactory;
impl NodeFactory for GaussianNbFactory {
    fn kind(&self) -> &'static str {
        "ml_gaussian_nb"
    }
    fn desc(&self) -> &'static str {
        "Gaussian Naive Bayes classifier."
    }
    fn doc(&self) -> &'static str {
        "GaussianNB: assumes features are conditionally independent given class, each following a Gaussian distribution."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GaussianNbSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: GaussianNbSpec = serde_json::from_value(spec)?;
        Ok(Box::new(GaussianNbNode {
            features: s.features,
            label_column: s.label_column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct GaussianNbNode {
    features: Vec<String>,
    label_column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for GaussianNbNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_gaussian_nb"
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
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_gaussian_nb".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_gaussian_nb".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result =
            ml::classify::gaussian_nb(&data, &labels).map_err(|e| DagError::NodeError {
                node_type: "ml_gaussian_nb".into(),
                msg: e.to_string(),
            })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        fields.push(Arc::new(Field::new(
            "probability",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.probabilities)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_gaussian_nb".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
