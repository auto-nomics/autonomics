use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Impute
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ImputeSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub fill_value: Option<f64>,
}
fn default_strategy() -> String {
    "mean".into()
}

pub struct ImputeFactory;
impl NodeFactory for ImputeFactory {
    fn kind(&self) -> &'static str {
        "ml_impute"
    }
    fn desc(&self) -> &'static str {
        "Replace NaN values with mean, median, or a constant."
    }
    fn doc(&self) -> &'static str {
        "Imputer: fills NaN values in specified columns using mean, median, or constant fill_value strategy."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ImputeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ImputeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ImputeNode {
            columns: s.columns,
            strategy: s.strategy,
            fill_value: s.fill_value,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ImputeNode {
    columns: Vec<String>,
    strategy: String,
    fill_value: Option<f64>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ImputeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_impute"
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
        let strat = match self.strategy.as_str() {
            "median" => ImputeStrategy::Median,
            "constant" => ImputeStrategy::Constant(self.fill_value.unwrap_or(0.0)),
            _ => ImputeStrategy::Mean,
        };
        let imputer = Imputer::fit(&data, strat).map_err(|e| DagError::NodeError {
            node_type: "ml_impute".into(),
            msg: e.to_string(),
        })?;
        let mut transformed = data;
        imputer.transform(&mut transformed);
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}
