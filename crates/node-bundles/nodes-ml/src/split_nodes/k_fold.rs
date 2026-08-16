use super::*;

// ═══════════════════════════════════════════════════════════════════════
// KFold
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KFoldSpec {
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

pub struct KFoldFactory;
impl NodeFactory for KFoldFactory {
    fn kind(&self) -> &'static str {
        "ml_kfold"
    }
    fn desc(&self) -> &'static str {
        "K-Fold cross-validation fold assignment."
    }
    fn doc(&self) -> &'static str {
        "KFold: assigns each row to one of K folds (0..K-1) via a new `fold` column. Downstream nodes can group by fold for CV."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KFoldSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: KFoldSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KFoldNode {
            k: s.k,
            shuffle: s.shuffle,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct KFoldNode {
    k: usize,
    shuffle: bool,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for KFoldNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_kfold"
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
        let folds = ml::split::kfold(n, self.k, self.shuffle, self.seed).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_kfold".into(),
                msg: e.to_string(),
            }
        })?;

        // Assign fold labels
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
