//! Goodness-of-fit and contingency-table nodes: `hypothesize.chisq_gof`,
//! `hypothesize.ks_test`, `hypothesize.shapiro_test`, `hypothesize.ad_test`,
//! `hypothesize.fisher_exact`.

use super::common::{
    HypoNodeError, collect_input, emit_test_row, extract_f64_column, extract_groups,
};
use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

fn default_alt() -> String {
    "two.sided".to_string()
}

// ════ chisq_gof ═════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChisqGofNodeSpec {
    pub count_column: String,
    /// Optional probability column (same length as counts). If absent, uniform.
    pub p_column: Option<String>,
    #[serde(default = "default_true")]
    pub rescale_p: bool,
}
fn default_true() -> bool {
    true
}

pub struct ChisqGofNodeFactory;
impl NodeFactory for ChisqGofNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.chisq_gof"
    }
    fn desc(&self) -> &'static str {
        "Chi-squared goodness-of-fit test."
    }
    fn doc(&self) -> &'static str {
        "Tests observed counts against expected proportions."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ChisqGofNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: ChisqGofNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ChisqGofNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct ChisqGofNode {
    meta: NodePorts,
    spec: ChisqGofNodeSpec,
}
#[async_trait]
impl DagNode for ChisqGofNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.chisq_gof"
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
        let counts_f = extract_f64_column(&batches, &self.spec.count_column)?;
        let counts: Vec<u64> = counts_f.iter().map(|&x| x as u64).collect();
        let k = counts.len();
        let p = if let Some(pc) = &self.spec.p_column {
            extract_f64_column(&batches, pc)?
        } else {
            vec![1.0_f64 / k as f64; k]
        };
        let result = h::chisq_gof(&counts, &p, self.spec.rescale_p)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ ks_test ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KsTestNodeSpec {
    pub x_column: String,
    pub y_column: Option<String>,
    pub group_column: Option<String>,
    #[serde(default = "default_alt")]
    pub alternative: String,
}

pub struct KsTestNodeFactory;
impl NodeFactory for KsTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.ks_test"
    }
    fn desc(&self) -> &'static str {
        "Kolmogorov-Smirnov test (one or two sample)."
    }
    fn doc(&self) -> &'static str {
        "One-sample (vs normal CDF) or two-sample KS test."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KsTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: KsTestNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KsTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct KsTestNode {
    meta: NodePorts,
    spec: KsTestNodeSpec,
}
#[async_trait]
impl DagNode for KsTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.ks_test"
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
        let alt = h::Alternative::parse_r(&self.spec.alternative)
            .map_err(|e| HypoNodeError::Spec(e.to_string()))?;
        let x = extract_f64_column(&batches, &self.spec.x_column)?;
        let result = if let Some(gc) = &self.spec.group_column {
            let groups = extract_groups(&batches, &self.spec.x_column, gc)?;
            if groups.len() < 2 {
                return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
            }
            h::ks_two_sample(&groups[0].1, &groups[1].1, alt)
        } else if let Some(yc) = &self.spec.y_column {
            let y = extract_f64_column(&batches, yc)?;
            h::ks_two_sample(&x, &y, alt)
        } else {
            // One-sample vs standard normal CDF.
            h::ks_one_sample(&x, h::normal_cdf)
        }
        .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ shapiro_test ══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ShapiroNodeSpec {
    pub x_column: String,
}

pub struct ShapiroNodeFactory;
impl NodeFactory for ShapiroNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.shapiro_test"
    }
    fn desc(&self) -> &'static str {
        "Shapiro-Wilk/Shapiro-Francia normality test."
    }
    fn doc(&self) -> &'static str {
        "Tests H₀: data is normally distributed. Reports the W statistic."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ShapiroNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: ShapiroNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ShapiroNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct ShapiroNode {
    meta: NodePorts,
    spec: ShapiroNodeSpec,
}
#[async_trait]
impl DagNode for ShapiroNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.shapiro_test"
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
        let x = extract_f64_column(&batches, &self.spec.x_column)?;
        let result = h::shapiro_wilk(&x).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ ad_test ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdTestNodeSpec {
    pub x_column: String,
    #[serde(default = "default_ad_dist")]
    pub dist: String,
}
fn default_ad_dist() -> String {
    "normal".to_string()
}

pub struct AdTestNodeFactory;
impl NodeFactory for AdTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.ad_test"
    }
    fn desc(&self) -> &'static str {
        "Anderson-Darling goodness-of-fit test."
    }
    fn doc(&self) -> &'static str {
        "Tests normality or exponentiality via the Anderson-Darling A² statistic."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AdTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: AdTestNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AdTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct AdTestNode {
    meta: NodePorts,
    spec: AdTestNodeSpec,
}
#[async_trait]
impl DagNode for AdTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.ad_test"
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
        let x = extract_f64_column(&batches, &self.spec.x_column)?;
        let dist = match self.spec.dist.to_lowercase().as_str() {
            "normal" | "norm" => h::AdDist::Normal,
            "exponential" | "exp" => h::AdDist::Exponential,
            _ => h::AdDist::Normal,
        };
        let result =
            h::anderson_darling(&x, dist).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ fisher_exact ══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FisherExactNodeSpec {
    pub row_column: String,
    pub col_column: String,
    #[serde(default = "default_alt")]
    pub alternative: String,
    #[serde(default = "default_conf")]
    pub conf_level: f64,
}
fn default_conf() -> f64 {
    0.95
}

pub struct FisherExactNodeFactory;
impl NodeFactory for FisherExactNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.fisher_exact"
    }
    fn desc(&self) -> &'static str {
        "Fisher's exact test for 2×2 tables."
    }
    fn doc(&self) -> &'static str {
        "Exact test of independence on a 2×2 contingency table."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FisherExactNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: FisherExactNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(FisherExactNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct FisherExactNode {
    meta: NodePorts,
    spec: FisherExactNodeSpec,
}
#[async_trait]
impl DagNode for FisherExactNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.fisher_exact"
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
        let alt = h::Alternative::parse_r(&self.spec.alternative)
            .map_err(|e| HypoNodeError::Spec(e.to_string()))?;
        // Build 2×2 table from two categorical columns.
        use std::collections::BTreeMap;
        let mut cells: BTreeMap<(String, String), u32> = BTreeMap::new();
        let mut rows_order: Vec<String> = Vec::new();
        let mut cols_order: Vec<String> = Vec::new();
        for batch in &batches {
            let ra = batch
                .column_by_name(&self.spec.row_column)
                .ok_or_else(|| HypoNodeError::Column(self.spec.row_column.clone()))?;
            let ca = batch
                .column_by_name(&self.spec.col_column)
                .ok_or_else(|| HypoNodeError::Column(self.spec.col_column.clone()))?;
            let rs = dag_core::node::string_opt_values(ra.as_ref()).unwrap_or_default();
            let cs = dag_core::node::string_opt_values(ca.as_ref()).unwrap_or_default();
            for i in 0..rs.len() {
                if let (Some(r), Some(c)) = (rs[i].as_ref(), cs[i].as_ref()) {
                    if !rows_order.contains(r) {
                        rows_order.push(r.clone());
                    }
                    if !cols_order.contains(c) {
                        cols_order.push(c.clone());
                    }
                    *cells.entry((r.clone(), c.clone())).or_insert(0) += 1;
                }
            }
        }
        if rows_order.len() != 2 || cols_order.len() != 2 {
            return Err(HypoNodeError::Spec(format!(
                "fisher_exact requires a 2×2 table, got {}×{}",
                rows_order.len(),
                cols_order.len()
            ))
            .into());
        }
        let table = [
            [
                cells
                    .get(&(rows_order[0].clone(), cols_order[0].clone()))
                    .copied()
                    .unwrap_or(0),
                cells
                    .get(&(rows_order[0].clone(), cols_order[1].clone()))
                    .copied()
                    .unwrap_or(0),
            ],
            [
                cells
                    .get(&(rows_order[1].clone(), cols_order[0].clone()))
                    .copied()
                    .unwrap_or(0),
                cells
                    .get(&(rows_order[1].clone(), cols_order[1].clone()))
                    .copied()
                    .unwrap_or(0),
            ],
        ];
        let result = h::fisher_exact(&table, alt, self.spec.conf_level)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}
