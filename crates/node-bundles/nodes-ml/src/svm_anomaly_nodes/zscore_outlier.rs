use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Z-score outlier
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ZscoreOutlierSpec {
    pub features: Vec<String>,
    #[serde(default = "d_zs_threshold")]
    pub threshold: f64,
}
fn d_zs_threshold() -> f64 {
    3.0
}

pub struct ZscoreOutlierFactory;
impl NodeFactory for ZscoreOutlierFactory {
    fn kind(&self) -> &'static str {
        "ml_zscore_outlier"
    }
    fn desc(&self) -> &'static str {
        "Z-score outlier detection."
    }
    fn doc(&self) -> &'static str {
        "Z-score: flags rows where any feature's z-score exceeds threshold. Simple but effective for Gaussian-distributed features."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ZscoreOutlierSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ZscoreOutlierSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ZscoreOutlierNode {
            features: s.features,
            threshold: s.threshold,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ZscoreOutlierNode {
    features: Vec<String>,
    threshold: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ZscoreOutlierNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_zscore_outlier"
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
                node_type: "ml_zscore_outlier".into(),
                msg: e.to_string(),
            })?;
        let result = ml::anomaly::zscore_outliers(&data, self.threshold).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_zscore_outlier".into(),
                msg: e.to_string(),
            }
        })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("max_zscore", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.scores)));
        fields.push(Arc::new(Field::new("is_outlier", DataType::Boolean, false)));
        arrays.push(Arc::new(BooleanArray::from(result.is_outlier)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_zscore_outlier".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
