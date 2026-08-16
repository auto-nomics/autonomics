use super::*;

// ═══════════════════════════════════════════════════════════════════════
// CorrelationRank (regression)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CorrelationRankSpec {
    pub features: Vec<String>,
    pub target_column: String,
}

pub struct CorrelationRankFactory;
impl NodeFactory for CorrelationRankFactory {
    fn kind(&self) -> &'static str {
        "ml_correlation_rank"
    }
    fn desc(&self) -> &'static str {
        "Rank features by absolute correlation with a continuous target."
    }
    fn doc(&self) -> &'static str {
        "CorrelationRank: computes |Pearson r| between each feature and the target column, outputs feature name and correlation sorted by descending strength."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CorrelationRankSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: CorrelationRankSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CorrelationRankNode {
            features: s.features,
            target_column: s.target_column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct CorrelationRankNode {
    features: Vec<String>,
    target_column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for CorrelationRankNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_correlation_rank"
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
                node_type: "ml_correlation_rank".into(),
                msg: e.to_string(),
            })?;
        let target =
            common::extract_numeric_column(&batches, &self.target_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_correlation_rank".into(),
                    msg: e.to_string(),
                }
            })?;
        let corrs = ml::feat_select::f_regression(&data, &target);
        let mut indexed: Vec<(usize, f64)> = corrs.iter().copied().enumerate().collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let names: Vec<String> = indexed
            .iter()
            .map(|&(i, _)| self.features[i].clone())
            .collect();
        let values: Vec<f64> = indexed.iter().map(|&(_, c)| c).collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("feature", DataType::Utf8, false),
                Field::new("abs_correlation", DataType::Float64, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(names)),
                Arc::new(Float64Array::from(values)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_correlation_rank".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
