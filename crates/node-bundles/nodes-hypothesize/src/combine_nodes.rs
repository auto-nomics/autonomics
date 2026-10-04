//! Combinator nodes (Group C): `hypothesize.combine_pvalues`,
//! `hypothesize.boolean_test`, `hypothesize.adjust_pvalues`.
//!
//! These operate on the `p_value` column of their input port (each row =
//! one component test) and emit either a single combined test row or a
//! passthrough table with adjusted p-values.

use super::common::{
    HypoNodeError, collect_input, emit_test_row, emit_test_row_with_p, extract_f64_column,
    extract_opt_f64_column,
};
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
        "intersection (max p), union (min p), or complement (1-p). NA semantics: \
        any null component p-value propagates to a null output p-value \
        (R min/max default NA rule); null rows are never silently dropped."
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
        let pvals = extract_opt_f64_column(
            &batches,
            self.spec.p_column.as_deref().unwrap_or("p_value"),
        )?;
        let n_tests = pvals.len();
        if n_tests == 0 {
            return Err(HypoNodeError::Insufficient("no p-values supplied".into()).into());
        }
        let n_missing = pvals.iter().filter(|p| p.is_none()).count();
        for p in pvals.iter().flatten() {
            if !(0.0..=1.0).contains(p) {
                return Err(HypoNodeError::Test(format!("p-value out of range: {p}")).into());
            }
        }

        // Any non-estimable component makes the joint p non-estimable. This
        // mirrors R's min/max NA rule: min(c(0.05, NA)) is NA, not 0.05 —
        // and a silent drop would turn `(0.001, NA)` into an intersection
        // p of 0.001, overstating the evidence.
        let (method, kind, p_out) = match self.spec.op.to_lowercase().as_str() {
            "intersection" | "and" => {
                let p = if n_missing > 0 {
                    None
                } else {
                    Some(
                        pvals
                            .iter()
                            .map(|p| p.unwrap())
                            .fold(f64::NEG_INFINITY, f64::max),
                    )
                };
                ("Intersection-Union Test", "intersection_test", p)
            }
            "union" | "or" => {
                let p = if n_missing > 0 {
                    None
                } else {
                    Some(
                        pvals
                            .iter()
                            .map(|p| p.unwrap())
                            .fold(f64::INFINITY, f64::min),
                    )
                };
                ("Union-Intersection Test", "union_test", p)
            }
            "complement" | "not" => {
                if n_tests != 1 {
                    return Err(HypoNodeError::Spec(
                        "complement requires exactly 1 p-value".into(),
                    )
                    .into());
                }
                ("Complemented Test", "complemented_test", pvals[0].map(|p| 1.0 - p))
            }
            other => {
                return Err(HypoNodeError::Spec(format!("unknown boolean op '{other}'")).into());
            }
        };

        let mut extras = serde_json::Map::new();
        extras.insert(h::KEY_KIND.into(), serde_json::json!(kind));
        extras.insert(h::KEY_N_TESTS.into(), serde_json::json!(n_tests));
        extras.insert("n_missing".into(), serde_json::json!(n_missing));
        extras.insert(
            h::KEY_COMPONENT_PVALS.into(),
            serde_json::json!(pvals), // Vec<Option<f64>> keeps nulls in the JSON array
        );
        if method == "Complemented Test" {
            extras.insert(
                h::KEY_ORIGINAL_PVAL.into(),
                match pvals[0] {
                    Some(p) => serde_json::json!(p),
                    None => serde_json::Value::Null,
                },
            );
        }
        extras.insert("n".into(), serde_json::json!(n_tests));

        let result = h::HypothesisTest {
            stat: f64::NAN,
            p_value: p_out.unwrap_or(f64::NAN),
            dof: f64::NAN,
            alternative: h::Alternative::TwoSided,
            method,
            extras,
        };
        emit_test_row_with_p(ctx, &result, p_out)
    }
}

// ════ adjust_pvalues ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdjustNodeSpec {
    /// Adjustment method (R `p.adjust` names). If omitted, Bonferroni is used
    /// and a WARN is emitted at runtime — research DAGs should set it
    /// explicitly (e.g. BH / BY).
    pub method: Option<String>,
    pub p_column: Option<String>,
    /// Family-size override (R `p.adjust(..., n = …)`). Defaults to the total
    /// row count across all batches, including rows with a null p-value
    /// (null rows count toward n and keep a null `p_adj`, mirroring
    /// `p.adjust(p, n = length(p))` with NA rows).
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
        "Applies p.adjust once over the full family (all input batches, \
        order preserved), appending p_adj and reject (p_adj < alpha) to \
        every batch. Rows with a null p keep a null p_adj/reject and still \
        count toward the family size n. Set method explicitly: omitting it \
        defaults to Bonferroni with a runtime WARN."
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
        if let Some(m) = &s.method {
            h::AdjustMethod::parse(m)
                .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        }
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
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let pcol = self.spec.p_column.as_deref().unwrap_or("p_value");
        // Full family in input order across all batches, nulls kept in place.
        let pvals = extract_opt_f64_column(&batches, pcol)?;
        let n_total = pvals.len();
        let n_eff = self.spec.n_total.unwrap_or(n_total);

        let method = match &self.spec.method {
            Some(m) => h::AdjustMethod::parse(m).map_err(|e| HypoNodeError::Spec(e.to_string()))?,
            None => {
                // Default kept for backward compatibility, but never silent:
                // a research DAG must choose its FDR/FWER procedure.
                let msg = "hypothesize.adjust_pvalues: 'method' not set; defaulting to \
                           Bonferroni. Set method explicitly (e.g. BH/BY) in research DAGs.";
                tracing::warn!("{msg}");
                reporter.warn(msg);
                h::AdjustMethod::Bonferroni
            }
        };

        // One family-wide adjustment over the estimable p-values; null rows
        // stay null in p_adj/reject but still count toward n (R semantics
        // for p.adjust(p, n = length(p)) with NA rows).
        let mut adjusted_full: Vec<Option<f64>> = vec![None; n_total];
        let nonnull: Vec<f64> = pvals.iter().flatten().copied().collect();
        if !nonnull.is_empty() {
            let adjusted = h::p_adjust_raw(&nonnull, method, Some(n_eff))
                .map_err(|e| HypoNodeError::Test(e.to_string()))?;
            let mut k = 0;
            for (i, p) in pvals.iter().enumerate() {
                if p.is_some() {
                    adjusted_full[i] = Some(adjusted[k]);
                    k += 1;
                }
            }
        }

        // Per-batch backfill: every batch keeps its row count and order,
        // with p_adj (Float64, nullable) and reject (Int32, nullable; null p
        // → null, not 0) appended.
        let mut dfs = Vec::new();
        let mut offset = 0usize;
        for batch in &batches {
            let rows = batch.num_rows();
            let slice = &adjusted_full[offset..offset + rows];
            offset += rows;

            let p_adj: Float64Array = slice.iter().copied().collect();
            let reject: Int32Array = slice
                .iter()
                .map(|p| p.map(|v| if v < self.spec.alpha { 1 } else { 0 }))
                .collect();

            let schema = batch.schema();
            let mut fields: Vec<arrow_schema::FieldRef> = schema.fields().iter().cloned().collect();
            let mut cols: Vec<Arc<dyn arrow_array::Array>> = batch.columns().to_vec();
            fields.push(Arc::new(Field::new("p_adj", DataType::Float64, true)));
            cols.push(Arc::new(p_adj));
            fields.push(Arc::new(Field::new("reject", DataType::Int32, true)));
            cols.push(Arc::new(reject));

            let new_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
                .map_err(|e| HypoNodeError::ReadBatch(e.to_string()))?;
            let df = ctx
                .session()
                .read_batch(new_batch)
                .map_err(|e| HypoNodeError::ReadBatch(e.to_string()))?;
            dfs.push(df);
        }

        let mut dfs = dfs.into_iter();
        let mut df = dfs
            .next()
            .ok_or_else(|| HypoNodeError::Insufficient("no input batches".into()))?;
        for other in dfs {
            df = df
                .union(other)
                .map_err(|e| HypoNodeError::Output(e.to_string()))?;
        }
        let mut res = dag_core::dag::graph::PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
