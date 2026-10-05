//! Parametric sample-data test nodes: `hypothesize.t_test`, `hypothesize.z_test`,
//! `hypothesize.prop_test`, `hypothesize.var_test`, `hypothesize.cor_test`.

use super::common::{
    HypoNodeError, collect_input, emit_test_row, extract_f64_column, extract_groups,
    extract_paired_columns,
};
use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

// ════════════════════════════════════════════════════════════════════════════
//  t_test
// ════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TTestNodeSpec {
    pub x_column: String,
    /// For two-sample: group labels. Mutually exclusive with `y_column`.
    pub group_column: Option<String>,
    /// For paired/two-sample via wide format.
    pub y_column: Option<String>,
    #[serde(default = "default_mu")]
    pub mu: f64,
    #[serde(default)]
    pub paired: bool,
    #[serde(default)]
    pub var_equal: bool,
    #[serde(default = "default_alt")]
    pub alternative: String,
    /// Confidence level for the estimate CI (R `t.test` `conf.level`), in
    /// (0, 1). Default 0.95.
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
}

fn default_mu() -> f64 {
    0.0
}
fn default_alt() -> String {
    "two.sided".to_string()
}
fn default_conf_level() -> f64 {
    0.95
}

pub struct TTestNodeFactory;
impl NodeFactory for TTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.t_test"
    }
    fn desc(&self) -> &'static str {
        "One-sample, two-sample, or paired t-test."
    }
    fn doc(&self) -> &'static str {
        "Performs a t-test on column data. Supports one-sample, \
        two-sample (group_column or y_column with var_equal), and paired \
        (paired=true with y_column) modes. mu is the null value in every \
        mode (R t.test mu: one-sample mean, paired mean of differences, \
        two-sample difference in means). Paired drops a pair when either \
        side is null (R complete.cases); wide two-sample treats the \
        columns as independent samples and drops NA rows per column (R \
        t.test(x, y) vector semantics); group_column follows R formula \
        na.omit. Emits a standard test row with stderr/conf_low/conf_high \
        at conf_level (default 0.95)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: TTestNodeSpec = serde_json::from_value(spec)?;
        if !(s.conf_level > 0.0 && s.conf_level < 1.0) {
            return Err(dag_core::registry::error::Error::Unknown(
                "conf_level must be in (0, 1)".into(),
            ));
        }
        Ok(Box::new(TTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct TTestNode {
    meta: NodePorts,
    spec: TTestNodeSpec,
}

#[async_trait]
impl DagNode for TTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.t_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let alt = h::Alternative::parse_r(&self.spec.alternative)
            .map_err(|e| HypoNodeError::Spec(e.to_string()))?;
        let cl = self.spec.conf_level;

        let result = if self.spec.paired {
            // R t.test(paired=TRUE) deletes pairs with an NA on either side
            // — extracting x and y independently would misalign the pairs.
            let yc = self
                .spec
                .y_column
                .as_deref()
                .ok_or_else(|| HypoNodeError::Spec("paired requires y_column".into()))?;
            let (pairs, n_dropped) = extract_paired_columns(&batches, &self.spec.x_column, yc)?;
            let xs: Vec<f64> = pairs.iter().map(|(a, _)| *a).collect();
            let ys: Vec<f64> = pairs.iter().map(|(_, b)| *b).collect();
            let mut r = h::t_test_paired(&xs, &ys, self.spec.mu, alt, cl)
                .map_err(|e| HypoNodeError::Test(e.to_string()))?;
            r.extras
                .insert("n_dropped".into(), serde_json::json!(n_dropped));
            r
        } else if let Some(gc) = &self.spec.group_column {
            // Group branch already follows R formula na.omit semantics via
            // extract_groups: rows with a null response or null group label
            // are dropped before grouping.
            let groups = extract_groups(&batches, &self.spec.x_column, gc)?;
            if groups.len() < 2 {
                return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
            }
            h::t_test_two(&groups[0].1, &groups[1].1, self.spec.var_equal, self.spec.mu, alt, cl)
                .map_err(|e| HypoNodeError::Test(e.to_string()))?
        } else if let Some(yc) = &self.spec.y_column {
            // Wide two-sample: the two columns are independent samples (R
            // t.test(x, y)), so NA rows drop per column, not pairwise —
            // extract_f64_column skips nulls within each column while
            // preserving cross-batch row order. Complete-case here would
            // silently import paired semantics into an unpaired test.
            let xs = extract_f64_column(&batches, &self.spec.x_column)?;
            let ys = extract_f64_column(&batches, yc)?;
            let mut r = h::t_test_two(&xs, &ys, self.spec.var_equal, self.spec.mu, alt, cl)
                .map_err(|e| HypoNodeError::Test(e.to_string()))?;
            r.extras
                .insert("n_x".into(), serde_json::json!(xs.len()));
            r.extras
                .insert("n_y".into(), serde_json::json!(ys.len()));
            r
        } else {
            let x = extract_f64_column(&batches, &self.spec.x_column)?;
            h::t_test_one(&x, self.spec.mu, alt, cl)
                .map_err(|e| HypoNodeError::Test(e.to_string()))?
        };

        emit_test_row(ctx, &result)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  z_test
// ════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ZTestNodeSpec {
    pub x_column: String,
    #[serde(default = "default_mu")]
    pub mu: f64,
    pub sigma: f64,
    #[serde(default = "default_alt")]
    pub alternative: String,
}

pub struct ZTestNodeFactory;
impl NodeFactory for ZTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.z_test"
    }
    fn desc(&self) -> &'static str {
        "One-sample z-test with known σ."
    }
    fn doc(&self) -> &'static str {
        "One-sample z-test of H₀: μ = mu with known population σ."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ZTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: ZTestNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ZTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct ZTestNode {
    meta: NodePorts,
    spec: ZTestNodeSpec,
}

#[async_trait]
impl DagNode for ZTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.z_test"
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
        let result = h::z_test(&x, self.spec.mu, self.spec.sigma, alt)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  prop_test
// ════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PropTestNodeSpec {
    /// Column of 0/1 or 1/0 binary outcomes, or counts.
    pub count_column: Option<String>,
    pub n_column: Option<String>,
    /// Scalar count and n for one-sample test.
    pub count: Option<u32>,
    pub n: Option<u32>,
    #[serde(default = "default_p0")]
    pub p: f64,
    #[serde(default = "default_true")]
    pub correct: bool,
}

fn default_p0() -> f64 {
    0.5
}
fn default_true() -> bool {
    true
}

pub struct PropTestNodeFactory;
impl NodeFactory for PropTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.prop_test"
    }
    fn desc(&self) -> &'static str {
        "One- or two-sample proportion z-test."
    }
    fn doc(&self) -> &'static str {
        "Proportion test with optional Yates continuity correction."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PropTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: PropTestNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PropTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct PropTestNode {
    meta: NodePorts,
    spec: PropTestNodeSpec,
}

#[async_trait]
impl DagNode for PropTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.prop_test"
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
        let result = if let (Some(x), Some(n)) = (self.spec.count, self.spec.n) {
            h::prop_test_one(x, n, self.spec.p, self.spec.correct)
        } else if let (Some(cc), Some(nc)) = (&self.spec.count_column, &self.spec.n_column) {
            let batches = collect_input(inputs).await?;
            let counts = extract_f64_column(&batches, cc)?;
            let ns = extract_f64_column(&batches, nc)?;
            if counts.len() == 1 {
                h::prop_test_one(
                    counts[0] as u32,
                    ns[0] as u32,
                    self.spec.p,
                    self.spec.correct,
                )
            } else if counts.len() == 2 {
                h::prop_test_two(
                    counts[0] as u32,
                    ns[0] as u32,
                    counts[1] as u32,
                    ns[1] as u32,
                    self.spec.correct,
                )
            } else {
                return Err(HypoNodeError::Spec("prop_test expects 1 or 2 rows".into()).into());
            }
        } else {
            return Err(HypoNodeError::Spec(
                "prop_test needs count+n scalars or count_column+n_column".into(),
            )
            .into());
        }
        .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  var_test
// ════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct VarTestNodeSpec {
    pub x_column: String,
    pub group_column: String,
    #[serde(default = "default_ratio")]
    pub ratio: f64,
    #[serde(default = "default_alt")]
    pub alternative: String,
}
fn default_ratio() -> f64 {
    1.0
}

pub struct VarTestNodeFactory;
impl NodeFactory for VarTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.var_test"
    }
    fn desc(&self) -> &'static str {
        "F-test comparing two variances."
    }
    fn doc(&self) -> &'static str {
        "Tests H₀: σ₁²/σ₂² = ratio via the F statistic."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(VarTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: VarTestNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(VarTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct VarTestNode {
    meta: NodePorts,
    spec: VarTestNodeSpec,
}

#[async_trait]
impl DagNode for VarTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.var_test"
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
        let groups = extract_groups(&batches, &self.spec.x_column, &self.spec.group_column)?;
        if groups.len() < 2 {
            return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
        }
        let result = h::var_test(&groups[0].1, &groups[1].1, self.spec.ratio, alt)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  cor_test
// ════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CorTestNodeSpec {
    pub x_column: String,
    pub y_column: String,
    #[serde(default = "default_cormethod")]
    pub method: String,
    #[serde(default = "default_alt")]
    pub alternative: String,
}
fn default_cormethod() -> String {
    "pearson".to_string()
}

pub struct CorTestNodeFactory;
impl NodeFactory for CorTestNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.cor_test"
    }
    fn desc(&self) -> &'static str {
        "Correlation test (Pearson/Spearman/Kendall)."
    }
    fn doc(&self) -> &'static str {
        "Tests for association between two columns."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CorTestNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CorTestNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CorTestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct CorTestNode {
    meta: NodePorts,
    spec: CorTestNodeSpec,
}

#[async_trait]
impl DagNode for CorTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.cor_test"
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
        let method = match self.spec.method.to_lowercase().as_str() {
            "pearson" => h::CorMethod::Pearson,
            "spearman" => h::CorMethod::Spearman,
            "kendall" => h::CorMethod::Kendall,
            other => return Err(HypoNodeError::Spec(format!("unknown method '{other}'")).into()),
        };
        let x = extract_f64_column(&batches, &self.spec.x_column)?;
        let y = extract_f64_column(&batches, &self.spec.y_column)?;
        let result =
            h::cor_test(&x, &y, method, alt).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}
