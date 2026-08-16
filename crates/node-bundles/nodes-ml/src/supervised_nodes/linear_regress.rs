use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Linear Regression
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LinearRegressSpec {
    pub features: Vec<String>,
    pub target_column: String,
}

pub struct LinearRegressFactory;
impl NodeFactory for LinearRegressFactory {
    fn kind(&self) -> &'static str {
        "ml_linear_regress"
    }
    fn desc(&self) -> &'static str {
        "Ordinary least squares linear regression."
    }
    fn doc(&self) -> &'static str {
        "LinearRegression: OLS via linfa-linear. Outputs predictions + coefficients."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LinearRegressSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LinearRegressSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LinearRegressNode {
            features: s.features,
            target_column: s.target_column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LinearRegressNode {
    features: Vec<String>,
    target_column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LinearRegressNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_linear_regress"
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
                node_type: "ml_linear_regress".into(),
                msg: e.to_string(),
            })?;
        let target =
            common::extract_numeric_column(&batches, &self.target_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_linear_regress".into(),
                    msg: e.to_string(),
                }
            })?;
        let result =
            ml::regress::linear_regression(&data, &target).map_err(|e| DagError::NodeError {
                node_type: "ml_linear_regress".into(),
                msg: e.to_string(),
            })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.predictions)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_linear_regress".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
