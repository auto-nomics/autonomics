//! Survey-weighted hypothesis test nodes.
//!
//! Nodes: `svyttest`, `svyranktest`, `svychisq`, `svyciprop`. Each takes a
//! data DataFrame + [`SurveyDesignSpec`] and outputs a test-result DataFrame
//! (statistic, p-value, df, estimate).
//!
//! R package reference: `survey::svyttest`, `svyranktest`, `svychisq`,
//! `svyciprop`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use arrow_array::Float64Array;

use crate::survey_common::{SurveyDesignSpec, gen_design_r, one_in_one_out};
use dag_core::codegen::helpers::{input_0, parse_spec};
use dag_core::codegen::{CodegenCtx, CodegenError, NodeCodegen};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

// =====================================================================
// svyttest
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyTtestSpec {
    pub design: SurveyDesignSpec,
    /// Two-sided formula `y ~ group` or one-sided for one-sample test.
    /// Specify as `response` + optional `group`.
    pub response: String,
    /// Grouping variable for two-sample test. If absent, one-sample test
    /// against the null mean of `response`.
    #[serde(default)]
    pub group: Option<String>,
    /// Null hypothesis value (default 0).
    #[serde(default)]
    pub null_value: Option<f64>,
}

/// Survey-weighted t-test node — **implemented** (one-sample).
/// Two-sample path falls back to R codegen for now.
#[derive(Clone)]
pub struct SvyTtestNode {
    meta: NodePorts,
    spec: SvyTtestSpec,
}

impl SvyTtestNode {
    pub fn new(spec: SvyTtestSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyTtestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyttest"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svyttest".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyttest".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let y = crate::survey_common::extract_variables(&batches, &[self.spec.response.clone()])?;
        let mu = self.spec.null_value.unwrap_or(0.0);

        let t_result = if let Some(group_col) = &self.spec.group {
            // Two-sample: extract group indicator.
            let group_str = crate::survey_common::extract_string_column_pub(&batches, group_col)
                .map_err(|e| DagError::NodeError {
                    node_type: "svyttest".into(),
                    msg: e.0,
                })?;
            // Map to 0/1 by sorted unique values.
            let mut levels: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for s in &group_str {
                if seen.insert(s.clone()) {
                    levels.push(s.clone());
                }
            }
            if levels.len() != 2 {
                return Err(DagError::NodeError {
                    node_type: "svyttest".into(),
                    msg: format!(
                        "two-sample t-test requires exactly 2 groups, got {}",
                        levels.len()
                    ),
                });
            }
            let z: Vec<u8> = group_str.iter().map(|g| (g == &levels[1]) as u8).collect();
            survey::svy_ttest_twosample(&y[0], &z, &design, mu).map_err(|e| {
                DagError::NodeError {
                    node_type: "svyttest".into(),
                    msg: e.to_string(),
                }
            })?
        } else {
            // One-sample.
            survey::svy_ttest_onesample(&y[0], &design, mu).map_err(|e| DagError::NodeError {
                node_type: "svyttest".into(),
                msg: e.to_string(),
            })?
        };

        // Build output RecordBatch.
        use arrow_array::{Float64Array, RecordBatch};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![
            Field::new("statistic", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("df", DataType::Float64, false),
            Field::new("estimate", DataType::Float64, false),
            Field::new("null_value", DataType::Float64, false),
            Field::new("ci_lower", DataType::Float64, false),
            Field::new("ci_upper", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![t_result.statistic])),
                Arc::new(Float64Array::from(vec![t_result.p_value])),
                Arc::new(Float64Array::from(vec![t_result.df as f64])),
                Arc::new(Float64Array::from(vec![t_result.estimate])),
                Arc::new(Float64Array::from(vec![t_result.null_value])),
                Arc::new(Float64Array::from(vec![t_result.ci_lower])),
                Arc::new(Float64Array::from(vec![t_result.ci_upper])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyttest".into(),
            msg: format!("failed to build output: {e}"),
        })?;

        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyttest".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

pub struct SvyTtestFactory;

impl NodeFactory for SvyTtestFactory {
    fn kind(&self) -> &'static str {
        "svyttest"
    }
    fn desc(&self) -> &'static str {
        "Design-based t-test (one-sample or two-sample)"
    }
    fn doc(&self) -> &'static str {
        "Performs a design-based t-test. For a two-sample test provide a \
         grouping variable; for a one-sample test omit it. \
         Wraps survey::svyttest."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyTtestSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyTtestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyTtestNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyTtestSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let formula = match &s.group {
            Some(g) => format!("{} ~ {}", s.response, g),
            None => format!("~{}", s.response),
        };
        let null_arg = match s.null_value {
            Some(v) => format!(", mu = {}", v),
            None => String::new(),
        };
        code.push(format!("{out} <- svyttest({formula}, {des}{null_arg})"));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svyranktest
// =====================================================================

fn default_rank_test() -> String {
    "wilcoxon".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyRankTestSpec {
    pub design: SurveyDesignSpec,
    /// Response variable column name.
    pub response: String,
    /// Grouping variable (two-sample test).
    pub group: String,
    /// Test type: `"wilcoxon"`, `"vanderWaerden"`, `"median"`, `"KruskalWallis"`.
    #[serde(default = "default_rank_test")]
    pub test: String,
}

/// Rank test node — **implemented** (2-group; Kruskal-Wallis via rank scores).
#[derive(Clone)]
pub struct SvyRankTestNode {
    meta: NodePorts,
    spec: SvyRankTestSpec,
}

impl SvyRankTestNode {
    pub fn new(spec: SvyRankTestSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyRankTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyranktest"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svyranktest".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyranktest".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let y = crate::survey_common::extract_variables(&batches, &[self.spec.response.clone()])?;
        let group_str = crate::survey_common::extract_string_column_pub(&batches, &self.spec.group)
            .map_err(|e| DagError::NodeError {
                node_type: "svyranktest".into(),
                msg: e.0,
            })?;

        // Map group to 0/1 (requires exactly 2 groups for the t-test form).
        let mut levels: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for g in &group_str {
            if seen.insert(g.clone()) {
                levels.push(g.clone());
            }
        }
        if levels.len() != 2 {
            return Err(DagError::NodeError {
                node_type: "svyranktest".into(),
                msg: format!(
                    "two-sample rank test requires exactly 2 groups, got {}",
                    levels.len()
                ),
            });
        }
        let group: Vec<f64> = group_str
            .iter()
            .map(|g| if g == &levels[1] { 1.0 } else { 0.0 })
            .collect();

        let r = survey::svy_ranktest(&y[0], &group, &design, &self.spec.test).map_err(|e| {
            DagError::NodeError {
                node_type: "svyranktest".into(),
                msg: e.to_string(),
            }
        })?;

        // Output: statistic, p_value, df, estimate.
        use arrow_array::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![
            Field::new("statistic", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("df", DataType::Float64, false),
            Field::new("estimate", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![r.statistic])),
                Arc::new(Float64Array::from(vec![r.p_value])),
                Arc::new(Float64Array::from(vec![r.df as f64])),
                Arc::new(Float64Array::from(vec![r.estimate])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyranktest".into(),
            msg: format!("failed to build output: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyranktest".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyRankTestFactory;

impl NodeFactory for SvyRankTestFactory {
    fn kind(&self) -> &'static str {
        "svyranktest"
    }
    fn desc(&self) -> &'static str {
        "Design-based two-sample rank test (Wilcoxon, Kruskal-Wallis, etc.)"
    }
    fn doc(&self) -> &'static str {
        "Performs a design-based two-sample rank test based on influence \
         functions. Supports Wilcoxon, van der Waerden, median, and \
         Kruskal-Wallis variants. Wraps survey::svyranktest."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyRankTestSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyRankTestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyRankTestNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyRankTestSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        code.push(format!(
            "{out} <- svyranktest({resp} ~ {grp}, {des}, test = \"{test}\")",
            resp = s.response,
            grp = s.group,
            test = s.test
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svychisq
// =====================================================================

fn default_chisq_stat() -> String {
    "F".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyChisqSpec {
    pub design: SurveyDesignSpec,
    /// Two variables forming the two-way table.
    pub row_var: String,
    pub col_var: String,
    /// Statistic type: `"Pearson"`, `"Adj"`, `"Wald"`, `"F"`, `"Wald-mloglinear"`,
    /// `"saddlepoint"`, `"lincom"`, `"chisq"`.
    #[serde(default = "default_chisq_stat")]
    pub statistic: String,
    /// Significance level.
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}

fn default_alpha() -> f64 {
    0.05
}

/// Chi-squared node — **implemented** (Rao-Scott F).
#[derive(Clone)]
pub struct SvyChisqNode {
    meta: NodePorts,
    spec: SvyChisqSpec,
}

impl SvyChisqNode {
    pub fn new(spec: SvyChisqSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyChisqNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svychisq"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svychisq".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svychisq".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let row = crate::survey_common::extract_string_column_pub(&batches, &self.spec.row_var)
            .map_err(|e| DagError::NodeError {
                node_type: "svychisq".into(),
                msg: e.0,
            })?;
        let col = crate::survey_common::extract_string_column_pub(&batches, &self.spec.col_var)
            .map_err(|e| DagError::NodeError {
                node_type: "svychisq".into(),
                msg: e.0,
            })?;

        let r = survey::svy_chisq(&row, &col, &design).map_err(|e| DagError::NodeError {
            node_type: "svychisq".into(),
            msg: e.to_string(),
        })?;

        // Output: statistic, ndf, ddf, p_value.
        use arrow_array::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![
            Field::new("statistic", DataType::Float64, false),
            Field::new("ndf", DataType::Float64, false),
            Field::new("ddf", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![r.statistic])),
                Arc::new(Float64Array::from(vec![r.ndf])),
                Arc::new(Float64Array::from(vec![r.ddf])),
                Arc::new(Float64Array::from(vec![r.p_value])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svychisq".into(),
            msg: format!("failed to build output: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svychisq".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyChisqFactory;

impl NodeFactory for SvyChisqFactory {
    fn kind(&self) -> &'static str {
        "svychisq"
    }
    fn desc(&self) -> &'static str {
        "Design-based chi-squared test for two-way tables"
    }
    fn doc(&self) -> &'static str {
        "Tests independence in a two-way table under a complex survey design. \
         Supports Pearson, adjusted Pearson, Wald, F, saddlepoint, and other \
         statistics. Wraps survey::svychisq."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyChisqSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyChisqSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyChisqNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyChisqSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        code.push(format!(
            "{out} <- svychisq(~{r} * {c}, {des}, statistic = \"{stat}\")",
            r = s.row_var,
            c = s.col_var,
            stat = s.statistic
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svyciprop
// =====================================================================

fn default_ci_method() -> String {
    "logit".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyCiPropSpec {
    pub design: SurveyDesignSpec,
    /// Binary (0/1) proportion variable column name.
    pub variable: String,
    /// CI method: `"Wald"`, `"logit"`, `"likelihood"`, `"asin"`, `"beta"`, `"mean"`.
    #[serde(default = "default_ci_method")]
    pub method: String,
    /// Significance level (default 0.05 → 95% CI).
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}

/// Proportion CI node — **implemented**.
#[derive(Clone)]
pub struct SvyCiPropNode {
    meta: NodePorts,
    spec: SvyCiPropSpec,
}

impl SvyCiPropNode {
    pub fn new(spec: SvyCiPropSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyCiPropNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyciprop"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svyciprop".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyciprop".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let y = crate::survey_common::extract_variables(&batches, &[self.spec.variable.clone()])?;

        // Map R method names to our implementation.
        let method = match self.spec.method.as_str() {
            "Wald" | "mean" => "mean",
            "logit" => "logit",
            "likelihood" => "likelihood",
            other => {
                return Err(DagError::NodeError {
                    node_type: "svyciprop".into(),
                    msg: format!("unsupported method '{other}'"),
                });
            }
        };
        let level = 1.0 - self.spec.alpha;

        let (prop, lo, hi) =
            survey::svy_ciprop(&y[0], &design, method, level).map_err(|e| DagError::NodeError {
                node_type: "svyciprop".into(),
                msg: e.to_string(),
            })?;

        use arrow_array::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![
            Field::new("proportion", DataType::Float64, false),
            Field::new("ci_lower", DataType::Float64, false),
            Field::new("ci_upper", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![prop])),
                Arc::new(Float64Array::from(vec![lo])),
                Arc::new(Float64Array::from(vec![hi])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyciprop".into(),
            msg: format!("failed to build output: {e}"),
        })?;

        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyciprop".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyCiPropFactory;

impl NodeFactory for SvyCiPropFactory {
    fn kind(&self) -> &'static str {
        "svyciprop"
    }
    fn desc(&self) -> &'static str {
        "Confidence interval for a survey-weighted proportion"
    }
    fn doc(&self) -> &'static str {
        "Computes a design-based confidence interval for a proportion. \
         Supports Wald, logit, likelihood, arcsine, and Beta methods. \
         Wraps survey::svyciprop."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyCiPropSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyCiPropSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyCiPropNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyCiPropSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        code.push(format!(
            "{out} <- svyciprop(~{v}, {des}, method = \"{m}\", level = {lvl})",
            v = s.variable,
            m = s.method,
            lvl = 1.0 - s.alpha
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svychisq_spec_defaults() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"]},
            "row_var": "gender",
            "col_var": "smoke"
        });
        let s: SvyChisqSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.statistic, "F");
        assert!((s.alpha - 0.05).abs() < 1e-10);
    }

    #[test]
    fn svyttest_two_sample() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"], "weights": "wt"},
            "response": "bp",
            "group": "treatment"
        });
        let s: SvyTtestSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.group.as_deref(), Some("treatment"));
    }

    // ── End-to-end test ────────────────────────────────────────────────────

    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }

    #[tokio::test]
    async fn svyttest_node_matches_r() {
        use arrow_array::{Float64Array, Int32Array};
        // fpc dataset, one-sample test of x against 0.
        let schema = std::sync::Arc::new(arrow_schema::Schema::new(vec![
            arrow_schema::Field::new("stratid", arrow_schema::DataType::Int32, false),
            arrow_schema::Field::new("psuid", arrow_schema::DataType::Int32, false),
            arrow_schema::Field::new("weight", arrow_schema::DataType::Float64, false),
            arrow_schema::Field::new("x", arrow_schema::DataType::Float64, false),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                std::sync::Arc::new(Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                std::sync::Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                std::sync::Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
                std::sync::Arc::new(Float64Array::from(vec![
                    2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2,
                ])),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyTtestSpec {
            design: crate::survey_common::SurveyDesignSpec {
                ids: vec!["psuid".into()],
                strata: vec!["stratid".into()],
                probs: vec![],
                weights: Some("weight".into()),
                fpc: vec![],
                nest: true,
                pps: "none".into(),
                variance: "HT".into(),
                lonely_psu: Some("remove".into()),
            },
            response: "x".into(),
            group: None,
            null_value: None,
        };

        let mut node = SvyTtestNode::new(spec);
        let input = NodeInput { port: 0, data: df };
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let result = outs[&0].clone().collect().await.unwrap();
        let t: f64 = result[0]
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let p: f64 = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let est: f64 = result[0]
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);

        // R golden: t=7.349765, p=0.0007318758, estimate=5.448148
        assert!((t - 7.349765).abs() < 0.01, "t: {t}");
        assert!((p - 0.0007318758).abs() < 0.001, "p: {p}");
        assert!((est - 5.448148).abs() < 0.01, "estimate: {est}");
    }
}
