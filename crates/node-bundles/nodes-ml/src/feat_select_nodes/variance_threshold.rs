use super::*;

// ═══════════════════════════════════════════════════════════════════════
// VarianceThreshold
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct VarianceThresholdSpec {
    pub features: Vec<String>,
    #[serde(default = "d_vt_threshold")]
    pub threshold: f64,
}
fn d_vt_threshold() -> f64 {
    0.0
}

pub struct VarianceThresholdFactory;
impl NodeFactory for VarianceThresholdFactory {
    fn kind(&self) -> &'static str {
        "ml_variance_threshold"
    }
    fn desc(&self) -> &'static str {
        "Remove features with variance below a threshold."
    }
    fn doc(&self) -> &'static str {
        "VarianceThreshold: filters out low-variance features. Outputs a summary table with feature names, variances, and selected status."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(VarianceThresholdSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: VarianceThresholdSpec = serde_json::from_value(spec)?;
        Ok(Box::new(VarianceThresholdNode {
            features: s.features,
            threshold: s.threshold,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct VarianceThresholdNode {
    features: Vec<String>,
    threshold: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for VarianceThresholdNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_variance_threshold"
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
                node_type: "ml_variance_threshold".into(),
                msg: e.to_string(),
            })?;
        let variances = ml::feat_select::column_variances(&data);
        let selected = ml::feat_select::variance_threshold(&data, self.threshold);

        let names: Vec<&str> = self.features.iter().map(|s| s.as_str()).collect();
        let sel_bools: Vec<bool> = (0..self.features.len())
            .map(|i| selected.contains(&i))
            .collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("variance", DataType::Float64, false),
                Field::new("selected", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(names)),
                Arc::new(Float64Array::from(variances)),
                Arc::new(arrow_array::BooleanArray::from(sel_bools)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_variance_threshold".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
