use super::*;

// ═══════════════════════════════════════════════════════════════════════
// MinMaxScale
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MinMaxScaleSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_lo")]
    pub feature_min: f64,
    #[serde(default = "default_hi")]
    pub feature_max: f64,
}
fn default_lo() -> f64 {
    0.0
}
fn default_hi() -> f64 {
    1.0
}

pub struct MinMaxScaleFactory;
impl NodeFactory for MinMaxScaleFactory {
    fn kind(&self) -> &'static str {
        "ml_minmax_scale"
    }
    fn desc(&self) -> &'static str {
        "Scale numeric columns to a fixed range [min, max]."
    }
    fn doc(&self) -> &'static str {
        "MinMaxScaler: linearly scales each column to [feature_min, feature_max]. Default range is [0, 1]."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MinMaxScaleSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: MinMaxScaleSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MinMaxScaleNode {
            columns: s.columns,
            feature_range: (s.feature_min, s.feature_max),
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct MinMaxScaleNode {
    columns: Vec<String>,
    feature_range: (f64, f64),
    meta: NodePorts,
}

#[async_trait]
impl DagNode for MinMaxScaleNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_minmax_scale"
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
        let data = extract_features(&batches, &self.columns)?;
        let (_, mut transformed) =
            MinMaxScaler::fit_transform(&data, self.feature_range).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_minmax_scale".into(),
                    msg: e.to_string(),
                }
            })?;
        let _ = &mut transformed;
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}
