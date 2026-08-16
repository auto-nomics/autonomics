use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Apriori Association Rules
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AprioriSpec {
    pub item_column: String,
    pub transaction_column: String,
    #[serde(default = "d_min_sup")]
    pub min_support: f64,
    #[serde(default = "d_min_conf")]
    pub min_confidence: f64,
}
fn d_min_sup() -> f64 {
    0.1
}
fn d_min_conf() -> f64 {
    0.5
}

pub struct AprioriFactory;
impl NodeFactory for AprioriFactory {
    fn kind(&self) -> &'static str {
        "ml_apriori"
    }
    fn desc(&self) -> &'static str {
        "Apriori frequent itemset + association rule mining."
    }
    fn doc(&self) -> &'static str {
        "Apriori: discovers frequent itemsets and association rules from transaction data. Requires item + transaction ID columns."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AprioriSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AprioriSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AprioriNode {
            item_column: s.item_column,
            transaction_column: s.transaction_column,
            min_support: s.min_support,
            min_confidence: s.min_confidence,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct AprioriNode {
    item_column: String,
    transaction_column: String,
    min_support: f64,
    min_confidence: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for AprioriNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_apriori"
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
        let items = common::extract_string_column(&batches, &self.item_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_apriori".into(),
                msg: e.to_string(),
            }
        })?;
        let txn_ids =
            common::extract_string_column(&batches, &self.transaction_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_apriori".into(),
                    msg: e.to_string(),
                }
            })?;

        // Group items by transaction
        let mut txn_map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (item, txn) in items.iter().zip(txn_ids.iter()) {
            txn_map.entry(txn.clone()).or_default().push(item.clone());
        }
        let transactions: Vec<Vec<String>> = txn_map.into_values().collect();

        let (_frequent, rules) =
            ml::assoc_recsys::apriori(&transactions, self.min_support, self.min_confidence)
                .map_err(|e| DagError::NodeError {
                    node_type: "ml_apriori".into(),
                    msg: e.to_string(),
                })?;

        let antecedents: Vec<String> = rules.iter().map(|r| r.antecedent.join(", ")).collect();
        let consequents: Vec<String> = rules.iter().map(|r| r.consequent.join(", ")).collect();
        let supports: Vec<f64> = rules.iter().map(|r| r.support).collect();
        let confidences: Vec<f64> = rules.iter().map(|r| r.confidence).collect();
        let lifts: Vec<f64> = rules.iter().map(|r| r.lift).collect();

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("antecedent", DataType::Utf8, false),
                Field::new("consequent", DataType::Utf8, false),
                Field::new("support", DataType::Float64, false),
                Field::new("confidence", DataType::Float64, false),
                Field::new("lift", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(antecedents)),
                Arc::new(StringArray::from(consequents)),
                Arc::new(Float64Array::from(supports)),
                Arc::new(Float64Array::from(confidences)),
                Arc::new(Float64Array::from(lifts)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_apriori".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
