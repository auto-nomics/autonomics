use super::*;

// ═══════════════════════════════════════════════════════════════════════
// StratifiedKFold
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StratifiedKFoldSpec {
    /// Column containing class labels for stratification.
    pub label_column: String,
    pub k: usize,
    #[serde(default = "d_shuffle")]
    pub shuffle: bool,
    #[serde(default = "d_seed")]
    pub seed: u64,
}

fn d_shuffle() -> bool {
    true
}

fn d_seed() -> u64 {
    42
}

pub struct StratifiedKFoldFactory;
impl NodeFactory for StratifiedKFoldFactory {
    fn kind(&self) -> &'static str {
        "ml_stratified_kfold"
    }
    fn desc(&self) -> &'static str {
        "Stratified K-Fold: preserves class proportions per fold."
    }
    fn doc(&self) -> &'static str {
        "StratifiedKFold: assigns folds such that each fold maintains the same class proportion as the full dataset. Requires a label column."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StratifiedKFoldSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: StratifiedKFoldSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StratifiedKFoldNode {
            label_column: s.label_column,
            k: s.k,
            shuffle: s.shuffle,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct StratifiedKFoldNode {
    label_column: String,
    k: usize,
    shuffle: bool,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for StratifiedKFoldNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_stratified_kfold"
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
        let labels = common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_stratified_kfold".into(),
                msg: e.to_string(),
            }
        })?;
        let labels_usize: Vec<usize> = labels.into_iter().map(|v| v as usize).collect();
        let folds = ml::split::stratified_kfold(&labels_usize, self.k, self.shuffle, self.seed)
            .map_err(|e| DagError::NodeError {
                node_type: "ml_stratified_kfold".into(),
                msg: e.to_string(),
            })?;

        let mut fold_labels = vec![0u32; n];
        for (fold_idx, (_, test)) in folds.iter().enumerate() {
            for &i in test {
                fold_labels[i] = fold_idx as u32;
            }
        }

        let batch = append_column(
            &batches,
            "fold",
            Arc::new(UInt32Array::from(fold_labels)),
            DataType::UInt32,
        )?;
        emit_batch(ctx, batch)
    }
}
