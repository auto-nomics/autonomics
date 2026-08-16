use super::*;

// ═══════════════════════════════════════════════════════════════════════
// LOF
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LofSpec {
    pub features: Vec<String>,
    #[serde(default = "d_lof_k")]
    pub k: usize,
}
fn d_lof_k() -> usize {
    20
}

pub struct LofFactory;
impl NodeFactory for LofFactory {
    fn kind(&self) -> &'static str {
        "ml_lof"
    }
    fn desc(&self) -> &'static str {
        "Local Outlier Factor anomaly detection."
    }
    fn doc(&self) -> &'static str {
        "LOF: density-based anomaly detection. LOF > 1 indicates a point is sparser than its neighbors."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LofSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LofSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LofNode {
            features: s.features,
            k: s.k,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LofNode {
    features: Vec<String>,
    k: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LofNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_lof"
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
                node_type: "ml_lof".into(),
                msg: e.to_string(),
            })?;
        let result =
            ml::anomaly::local_outlier_factor(&data, self.k).map_err(|e| DagError::NodeError {
                node_type: "ml_lof".into(),
                msg: e.to_string(),
            })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("lof_score", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.scores)));
        fields.push(Arc::new(Field::new("is_outlier", DataType::Boolean, false)));
        arrays.push(Arc::new(BooleanArray::from(result.is_outlier)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_lof".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
