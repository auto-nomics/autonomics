//! Combinator nodes (Group C): `hypothesize.combine_pvalues`,
//! `hypothesize.boolean_test`, `hypothesize.adjust_pvalues`.
//!
//! These operate on the `p_value` column of their input port (each row =
//! one component test) and emit either a single combined test row or a
//! passthrough table with adjusted p-values.

use super::common::{HypoNodeError, collect_input, emit_test_row, extract_f64_column};
use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::sync::Arc;

fn default_bonf() -> String {
    "bonferroni".to_string()
}

fn default_alpha() -> f64 {
    0.05
}

// ════ combine_pvalues ═══════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CombineNodeSpec {
    #[serde(default = "default_fisher")]
    pub method: String,
    pub p_column: Option<String>,
}
fn default_fisher() -> String {
    "fisher".to_string()
}

pub struct CombineNodeFactory;
impl NodeFactory for CombineNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.combine_pvalues"
    }
    fn desc(&self) -> &'static str {
        "Combine p-values (Fisher/Stouffer/Tippett/Wilkinson)."
    }
    fn doc(&self) -> &'static str {
        "Takes a column of p-values and combines them into a single omnibus test."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CombineNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CombineNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CombineNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct CombineNode {
    meta: NodePorts,
    spec: CombineNodeSpec,
}
#[async_trait]
impl DagNode for CombineNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.combine_pvalues"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let pvals =
            extract_f64_column(&batches, self.spec.p_column.as_deref().unwrap_or("p_value"))?;
        let result = match self.spec.method.to_lowercase().as_str() {
            "fisher" => h::fisher_combine_pvals(&pvals),
            "stouffer" => h::stouffer_combine_pvals(&pvals, None),
            "tippett_min" | "tippett" => h::tippett_combine_pvals(&pvals, h::TippettVariant::Min),
            "tippett_max" => h::tippett_combine_pvals(&pvals, h::TippettVariant::Max),
            other => {
                return Err(
                    HypoNodeError::Spec(format!("unknown combine method '{other}'")).into(),
                );
            }
        }
        .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ boolean_test ══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BooleanNodeSpec {
    pub op: String, // "intersection" | "union" | "complement"
    pub p_column: Option<String>,
}

pub struct BooleanNodeFactory;
impl NodeFactory for BooleanNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.boolean_test"
    }
    fn desc(&self) -> &'static str {
        "Boolean algebra over hypothesis tests (AND/OR/NOT)."
    }
    fn doc(&self) -> &'static str {
        "intersection (max p), union (min p), or complement (1-p)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BooleanNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: BooleanNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(BooleanNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct BooleanNode {
    meta: NodePorts,
    spec: BooleanNodeSpec,
}
#[async_trait]
impl DagNode for BooleanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.boolean_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let pvals =
            extract_f64_column(&batches, self.spec.p_column.as_deref().unwrap_or("p_value"))?;
        let result = match self.spec.op.to_lowercase().as_str() {
            "intersection" | "and" => h::intersection_test(&pvals),
            "union" | "or" => h::union_test(&pvals),
            "complement" | "not" => {
                if pvals.len() != 1 {
                    return Err(HypoNodeError::Spec(
                        "complement requires exactly 1 p-value".into(),
                    )
                    .into());
                }
                let test = h::wald_uni(0.0, 1.0, 0.0).unwrap(); // placeholder to get a HypothesisTest
                Ok(h::complement_test(&test).reassign_pval(1.0 - pvals[0]))
            }
            other => {
                return Err(HypoNodeError::Spec(format!("unknown boolean op '{other}'")).into());
            }
        }
        .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ adjust_pvalues ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdjustNodeSpec {
    #[serde(default = "default_bonf")]
    pub method: String,
    pub p_column: Option<String>,
    pub n_total: Option<usize>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}

pub struct AdjustNodeFactory;
impl NodeFactory for AdjustNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.adjust_pvalues"
    }
    fn desc(&self) -> &'static str {
        "Multiple-testing correction (Bonferroni/Holm/BH/BY/...)."
    }
    fn doc(&self) -> &'static str {
        "Applies p.adjust to a column of p-values, adding p_adj and reject \
        (p_adj < alpha) to the output."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AdjustNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: AdjustNodeSpec = serde_json::from_value(spec)?;
        h::AdjustMethod::parse(&s.method)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        if !(0.0..=1.0).contains(&s.alpha) || s.alpha == 0.0 {
            return Err(dag_core::registry::error::Error::Unknown(
                "alpha must be in (0, 1]".into(),
            ));
        }
        Ok(Box::new(AdjustNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct AdjustNode {
    meta: NodePorts,
    spec: AdjustNodeSpec,
}
#[async_trait]
impl DagNode for AdjustNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.adjust_pvalues"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let pcol = self.spec.p_column.as_deref().unwrap_or("p_value");
        let pvals = extract_f64_column(&batches, pcol)?;
        let method = h::AdjustMethod::parse(&self.spec.method)
            .map_err(|e| HypoNodeError::Spec(e.to_string()))?;
        let adjusted = h::p_adjust_raw(&pvals, method, self.spec.n_total)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;

        // Build output: same input batch but with `p_adj` column appended.
        let batch = &batches[0];
        let schema = batch.schema();
        let mut fields: Vec<arrow_schema::FieldRef> = schema.fields().iter().cloned().collect();
        let mut cols: Vec<Arc<dyn arrow_array::Array>> = batch.columns().to_vec();
        fields.push(Arc::new(Field::new("p_adj", DataType::Float64, false)));
        cols.push(Arc::new(Float64Array::from(adjusted.clone())));
        fields.push(Arc::new(Field::new("reject", DataType::Int32, false)));
        cols.push(Arc::new(Int32Array::from(
            adjusted
                .iter()
                .map(|&p| if p < self.spec.alpha { 1 } else { 0 })
                .collect::<Vec<_>>(),
        )));

        let new_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
            .map_err(|e| HypoNodeError::ReadBatch(e.to_string()))?;
        let session = ctx.session();
        let df = session
            .read_batch(new_batch)
            .map_err(|e| HypoNodeError::ReadBatch(e.to_string()))?;
        let mut res = dag_core::dag::graph::PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// Helper trait to reassign p_value (for complement op on raw p-value lists).
trait ReassignPval {
    fn reassign_pval(self, p: f64) -> Self;
}
impl ReassignPval for h::HypothesisTest {
    fn reassign_pval(mut self, p: f64) -> Self {
        self.p_value = p;
        self
    }
}
