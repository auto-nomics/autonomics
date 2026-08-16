use super::*;

// ═══════════════════════════════════════════════════════════════════════
// TrainTestSplit
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TrainTestSplitSpec {
    /// Stratify column (optional). If set, class proportions are preserved.
    #[serde(default)]
    pub stratify_column: Option<String>,
    /// Fraction allocated to the test set, in (0, 1). Default 0.2.
    #[serde(default = "d_test_size")]
    pub test_size: f64,
    /// Random seed. Default 42.
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_test_size() -> f64 {
    0.2
}
fn d_seed() -> u64 {
    42
}

pub struct TrainTestSplitFactory;
impl NodeFactory for TrainTestSplitFactory {
    fn kind(&self) -> &'static str {
        "ml_train_test_split"
    }
    fn desc(&self) -> &'static str {
        "Split data into train and test subsets."
    }
    fn doc(&self) -> &'static str {
        "TrainTestSplit: randomly partitions rows into train (port 0) and test (port 1) sets. Supports stratified splitting to preserve class proportions."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TrainTestSplitSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TrainTestSplitSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TrainTestSplitNode {
            stratify_column: s.stratify_column,
            test_size: s.test_size,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TrainTestSplitNode {
    stratify_column: Option<String>,
    test_size: f64,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for TrainTestSplitNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_train_test_split"
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
        let n: usize = batches.iter().map(|b| b.num_rows()).sum();

        let stratify_labels = match &self.stratify_column {
            Some(col) => {
                let vals = common::extract_numeric_column(&batches, col).map_err(|e| {
                    DagError::NodeError {
                        node_type: "ml_train_test_split".into(),
                        msg: e.to_string(),
                    }
                })?;
                Some(vals.into_iter().map(|v| v as usize).collect::<Vec<_>>())
            }
            None => None,
        };

        let split =
            ml::split::train_test_split(n, self.test_size, stratify_labels.as_deref(), self.seed)
                .map_err(|e| DagError::NodeError {
                node_type: "ml_train_test_split".into(),
                msg: e.to_string(),
            })?;

        let train_batch = select_rows(&batches, &split.train_indices)?;
        let test_batch = select_rows(&batches, &split.test_indices)?;
        emit_two_batches(ctx, train_batch, test_batch)
    }
}
