use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Decision Tree
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DecisionTreeSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_dt_depth")]
    pub max_depth: usize,
    #[serde(default = "d_dt_split")]
    pub min_samples_split: usize,
    #[serde(default = "d_dt_leaf")]
    pub min_samples_leaf: usize,
}
fn d_dt_depth() -> usize {
    10
}
fn d_dt_split() -> usize {
    2
}
fn d_dt_leaf() -> usize {
    1
}

pub struct DecisionTreeFactory;
impl NodeFactory for DecisionTreeFactory {
    fn kind(&self) -> &'static str {
        "ml_decision_tree"
    }
    fn desc(&self) -> &'static str {
        "Decision tree (CART) classifier."
    }
    fn doc(&self) -> &'static str {
        "DecisionTree: CART classification tree with configurable depth, split, and leaf constraints."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DecisionTreeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: DecisionTreeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DecisionTreeNode {
            features: s.features,
            label_column: s.label_column,
            max_depth: s.max_depth,
            min_samples_split: s.min_samples_split,
            min_samples_leaf: s.min_samples_leaf,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct DecisionTreeNode {
    features: Vec<String>,
    label_column: String,
    max_depth: usize,
    min_samples_split: usize,
    min_samples_leaf: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for DecisionTreeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_decision_tree"
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
                node_type: "ml_decision_tree".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_decision_tree".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result = ml::classify::decision_tree(
            &data,
            &labels,
            self.max_depth,
            self.min_samples_split,
            self.min_samples_leaf,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_decision_tree".into(),
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
                    node_type: "ml_decision_tree".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
