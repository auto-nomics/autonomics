use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Isolation Forest
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct IsolationForestSpec {
    pub features: Vec<String>,
    #[serde(default = "d_if_trees")]
    pub n_trees: usize,
    #[serde(default = "d_if_samples")]
    pub max_samples: usize,
    #[serde(default = "d_if_seed")]
    pub seed: u64,
}
fn d_if_trees() -> usize {
    100
}
fn d_if_samples() -> usize {
    256
}
fn d_if_seed() -> u64 {
    42
}

pub struct IsolationForestFactory;
impl NodeFactory for IsolationForestFactory {
    fn kind(&self) -> &'static str {
        "ml_isolation_forest"
    }
    fn desc(&self) -> &'static str {
        "Isolation Forest anomaly detection."
    }
    fn doc(&self) -> &'static str {
        "IsolationForest: detects anomalies via random partition trees. Points with shorter average path lengths are more anomalous. Outputs anomaly_score + is_outlier."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(IsolationForestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: IsolationForestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(IsolationForestNode {
            features: s.features,
            n_trees: s.n_trees,
            max_samples: s.max_samples,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct IsolationForestNode {
    features: Vec<String>,
    n_trees: usize,
    max_samples: usize,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for IsolationForestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_isolation_forest"
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
                node_type: "ml_isolation_forest".into(),
                msg: e.to_string(),
            })?;
        let result =
            ml::anomaly::isolation_forest(&data, self.n_trees, self.max_samples, self.seed)
                .map_err(|e| DagError::NodeError {
                    node_type: "ml_isolation_forest".into(),
                    msg: e.to_string(),
                })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new(
            "anomaly_score",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.scores)));
        fields.push(Arc::new(Field::new("is_outlier", DataType::Boolean, false)));
        arrays.push(Arc::new(BooleanArray::from(result.is_outlier)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_isolation_forest".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
