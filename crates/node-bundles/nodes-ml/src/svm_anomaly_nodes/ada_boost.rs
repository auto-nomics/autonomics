use super::*;

// ═══════════════════════════════════════════════════════════════════════
// AdaBoost
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdaBoostSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_ab_n")]
    pub n_estimators: usize,
    #[serde(default = "d_ab_lr")]
    pub learning_rate: f64,
}
fn d_ab_n() -> usize {
    50
}
fn d_ab_lr() -> f64 {
    1.0
}

pub struct AdaBoostFactory;
impl NodeFactory for AdaBoostFactory {
    fn kind(&self) -> &'static str {
        "ml_adaboost"
    }
    fn desc(&self) -> &'static str {
        "AdaBoost ensemble classifier."
    }
    fn doc(&self) -> &'static str {
        "AdaBoost: boosted ensemble of decision trees with adaptive sample weighting."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AdaBoostSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AdaBoostSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AdaBoostNode {
            features: s.features,
            label_column: s.label_column,
            n_estimators: s.n_estimators,
            learning_rate: s.learning_rate,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct AdaBoostNode {
    features: Vec<String>,
    label_column: String,
    n_estimators: usize,
    learning_rate: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for AdaBoostNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_adaboost"
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
                node_type: "ml_adaboost".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_adaboost".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result =
            ml::svm_ensemble::adaboost(&data, &labels, self.n_estimators, self.learning_rate)
                .map_err(|e| DagError::NodeError {
                    node_type: "ml_adaboost".into(),
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
                    node_type: "ml_adaboost".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
