use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Logistic Regression
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LogisticSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_alpha")]
    pub alpha: f64,
    #[serde(default = "d_max_iter")]
    pub max_iter: usize,
}
fn d_alpha() -> f64 {
    0.1
}
fn d_max_iter() -> usize {
    200
}

pub struct LogisticFactory;
impl NodeFactory for LogisticFactory {
    fn kind(&self) -> &'static str {
        "ml_logistic"
    }
    fn desc(&self) -> &'static str {
        "Binary logistic regression classifier."
    }
    fn doc(&self) -> &'static str {
        "LogisticRegression: L2-regularised binary logistic regression via gradient descent. Outputs predictions + probabilities."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LogisticSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LogisticSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LogisticNode {
            features: s.features,
            label_column: s.label_column,
            alpha: s.alpha,
            max_iter: s.max_iter,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LogisticNode {
    features: Vec<String>,
    label_column: String,
    alpha: f64,
    max_iter: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LogisticNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_logistic"
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
                node_type: "ml_logistic".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_logistic".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result = ml::classify::logistic_regression(&data, &labels, self.alpha, self.max_iter)
            .map_err(|e| DagError::NodeError {
            node_type: "ml_logistic".into(),
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
                    node_type: "ml_logistic".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
