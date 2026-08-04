//! Nonparametric rank-test nodes: `hypothesize.wilcoxon`, `hypothesize.kruskal_wallis`,
//! `hypothesize.friedman_test`.

use super::common::{HypoNodeError, collect_input, emit_test_row, extract_f64_column, extract_groups};
use crate::dag::DagError;
use crate::nodes::meta::{DagNode, NodeInput, NodePorts};
use crate::node_registry::registry::{NodeCtx, NodeFactory};
use async_trait::async_trait;
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

fn default_alt() -> String { "two.sided".to_string() }
fn default_true() -> bool { true }

// ════ wilcoxon ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WilcoxonNodeSpec {
    pub x_column: String,
    pub y_column: Option<String>,
    pub group_column: Option<String>,
    #[serde(default)]
    pub mu: f64,
    #[serde(default)]
    pub paired: bool,
    #[serde(default = "default_true")]
    pub correct: bool,
    #[serde(default = "default_alt")]
    pub alternative: String,
}

pub struct WilcoxonNodeFactory;
impl NodeFactory for WilcoxonNodeFactory {
    fn kind(&self) -> &'static str { "hypothesize.wilcoxon" }
    fn desc(&self) -> &'static str { "Wilcoxon signed-rank or Mann-Whitney test." }
    fn doc(&self) -> &'static str { "Nonparametric rank-based location test." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(WilcoxonNodeSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_output_port(None).add_input_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: WilcoxonNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(WilcoxonNode { meta: NodePorts::new().add_output_port(None).add_input_port(None), spec: s }))
    }
}

#[derive(Clone)]
struct WilcoxonNode { meta: NodePorts, spec: WilcoxonNodeSpec }

#[async_trait]
impl DagNode for WilcoxonNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "hypothesize.wilcoxon" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &crate::dag::node_event::NodeReporter) -> Result<crate::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let alt = h::Alternative::parse_r(&self.spec.alternative).map_err(|e| HypoNodeError::Spec(e.to_string()))?;
        let x = extract_f64_column(&batches, &self.spec.x_column)?;
        let result = if let Some(gc) = &self.spec.group_column {
            let groups = extract_groups(&batches, &self.spec.x_column, gc)?;
            if groups.len() < 2 { return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into()); }
            h::mann_whitney(&groups[0].1, &groups[1].1, alt, self.spec.correct)
        } else if let Some(yc) = &self.spec.y_column {
            let y = extract_f64_column(&batches, yc)?;
            if self.spec.paired {
                h::wilcoxon_signed_rank(&x, self.spec.mu, alt, h::ZeroMethod::Wilcox, self.spec.correct)
            } else {
                h::mann_whitney(&x, &y, alt, self.spec.correct)
            }
        } else {
            h::wilcoxon_signed_rank(&x, self.spec.mu, alt, h::ZeroMethod::Wilcox, self.spec.correct)
        }.map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ kruskal_wallis ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KruskalWallisNodeSpec {
    pub value_column: String,
    pub group_column: String,
}

pub struct KruskalWallisNodeFactory;
impl NodeFactory for KruskalWallisNodeFactory {
    fn kind(&self) -> &'static str { "hypothesize.kruskal_wallis" }
    fn desc(&self) -> &'static str { "Kruskal-Wallis rank-sum test for k groups." }
    fn doc(&self) -> &'static str { "Nonparametric one-way ANOVA via ranks." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(KruskalWallisNodeSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_output_port(None).add_input_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: KruskalWallisNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KruskalWallisNode { meta: NodePorts::new().add_output_port(None).add_input_port(None), spec: s }))
    }
}

#[derive(Clone)]
struct KruskalWallisNode { meta: NodePorts, spec: KruskalWallisNodeSpec }

#[async_trait]
impl DagNode for KruskalWallisNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "hypothesize.kruskal_wallis" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &crate::dag::node_event::NodeReporter) -> Result<crate::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let groups = extract_groups(&batches, &self.spec.value_column, &self.spec.group_column)?;
        if groups.len() < 2 { return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into()); }
        let refs: Vec<&[f64]> = groups.iter().map(|(_, v)| v.as_slice()).collect();
        let result = h::kruskal_wallis(&refs).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ friedman_test ═════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FriedmanNodeSpec {
    pub value_column: String,
    pub group_column: String,
    pub block_column: String,
}

pub struct FriedmanNodeFactory;
impl NodeFactory for FriedmanNodeFactory {
    fn kind(&self) -> &'static str { "hypothesize.friedman_test" }
    fn desc(&self) -> &'static str { "Friedman rank-sum test for randomized blocks." }
    fn doc(&self) -> &'static str { "Nonparametric test for differences across treatments in a randomized block design." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(FriedmanNodeSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_output_port(None).add_input_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: FriedmanNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(FriedmanNode { meta: NodePorts::new().add_output_port(None).add_input_port(None), spec: s }))
    }
}

#[derive(Clone)]
struct FriedmanNode { meta: NodePorts, spec: FriedmanNodeSpec }

#[async_trait]
impl DagNode for FriedmanNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "hypothesize.friedman_test" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &crate::dag::node_event::NodeReporter) -> Result<crate::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        // Build blocks × treatments from value/group/block columns.
        let mut blocks: std::collections::BTreeMap<String, Vec<(String, f64)>> = std::collections::BTreeMap::new();
        for batch in &batches {
            use arrow_array::{Array, Float64Array};
            let vals = batch.column_by_name(&self.spec.value_column).ok_or_else(|| HypoNodeError::Column(self.spec.value_column.clone()))?;
            let grps = batch.column_by_name(&self.spec.group_column).ok_or_else(|| HypoNodeError::Column(self.spec.group_column.clone()))?;
            let blks = batch.column_by_name(&self.spec.block_column).ok_or_else(|| HypoNodeError::Column(self.spec.block_column.clone()))?;
            let va = vals.as_any().downcast_ref::<Float64Array>().ok_or_else(|| HypoNodeError::Column("value not f64".into()))?;
            let g_strs = crate::nodes::meta::string_opt_values(grps.as_ref()).unwrap_or_default();
            let b_strs = crate::nodes::meta::string_opt_values(blks.as_ref()).unwrap_or_default();
            for i in 0..va.len() {
                if va.is_null(i) { continue; }
                if let (Some(g), Some(b)) = (g_strs.get(i).and_then(|x| x.as_ref()), b_strs.get(i).and_then(|x| x.as_ref())) {
                    blocks.entry(b.clone()).or_default().push((g.clone(), va.value(i)));
                }
            }
        }
        // Pivot: find unique treatment names and build per-block rows.
        let mut treatments: Vec<String> = Vec::new();
        for entries in blocks.values() {
            for (g, _) in entries {
                if !treatments.contains(g) { treatments.push(g.clone()); }
            }
        }
        let mut data: Vec<Vec<f64>> = Vec::new(); // rows = blocks, cols = treatments
        for entries in blocks.values() {
            let row: Vec<f64> = treatments.iter().map(|t| {
                entries.iter().find(|(g, _)| g == t).map(|(_, v)| *v).unwrap_or(f64::NAN)
            }).collect();
            data.push(row);
        }
        let refs: Vec<&[f64]> = data.iter().map(|r| r.as_slice()).collect();
        let result = h::friedman_test(&refs).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

