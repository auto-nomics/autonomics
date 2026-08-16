use super::*;

// ═══════════════════════════════════════════════════════════════════════
// SelectKBest (classification)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SelectKBestSpec {
    pub features: Vec<String>,
    pub label_column: String,
    pub k: usize,
}

pub struct SelectKBestFactory;
impl NodeFactory for SelectKBestFactory {
    fn kind(&self) -> &'static str {
        "ml_select_k_best"
    }
    fn desc(&self) -> &'static str {
        "Select top-K features by ANOVA F-value."
    }
    fn doc(&self) -> &'static str {
        "SelectKBest: ranks features by ANOVA F-statistic against class labels, returns the top K. Outputs feature, score, rank."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SelectKBestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: SelectKBestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SelectKBestNode {
            features: s.features,
            label_column: s.label_column,
            k: s.k,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct SelectKBestNode {
    features: Vec<String>,
    label_column: String,
    k: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for SelectKBestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_select_k_best"
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
                node_type: "ml_select_k_best".into(),
                msg: e.to_string(),
            })?;
        let labels_f64 =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_select_k_best".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f64.into_iter().map(|v| v as usize).collect();
        let (indices, scores) =
            ml::feat_select::select_k_best(&data, &labels, self.k).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_select_k_best".into(),
                    msg: e.to_string(),
                }
            })?;

        let names: Vec<String> = indices.iter().map(|&i| self.features[i].clone()).collect();
        let ranks: Vec<u32> = (1..=indices.len() as u32).collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("f_score", DataType::Float64, false),
                Field::new("rank", DataType::UInt32, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(names)),
                Arc::new(Float64Array::from(scores)),
                Arc::new(UInt32Array::from(ranks)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_select_k_best".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
