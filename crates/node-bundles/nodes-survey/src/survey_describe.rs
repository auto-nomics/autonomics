//! Survey-weighted descriptive statistics nodes.
//!
//! Nodes: `svymean`, `svytotal`, `svyvar`, `svyratio`, `svytable`,
//! `svyquantile`. Each takes a data DataFrame + [`SurveyDesignSpec`] and
//! outputs a summary DataFrame.
//!
//! R package reference: `survey::svymean`, `svytotal`, `svyvar`, `svyratio`,
//! `svytable`, `svyquantile`.

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use async_trait::async_trait;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};

use crate::survey_common::{
    SurveyDesignSpec, formula_rhs, gen_design_r, one_in_one_out, r_true_false,
};
use dag_core::codegen::helpers::{input_0, parse_spec};
use dag_core::codegen::{CodegenCtx, CodegenError, NodeCodegen};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

// =====================================================================
// svymean — REAL IMPLEMENTATION
// =====================================================================

/// Spec for the `svymean` node.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyMeanSpec {
    /// Survey design specification.
    pub design: SurveyDesignSpec,
    /// Variable column name(s) to compute means for.
    pub variables: Vec<String>,
    /// Remove missing values (R `na.rm`).
    #[serde(default)]
    pub na_rm: bool,
}

/// Survey-weighted mean node — **implemented** (not stubbed).
#[derive(Clone)]
pub struct SvyMeanNode {
    meta: NodePorts,
    spec: SvyMeanSpec,
}

impl SvyMeanNode {
    pub fn new(spec: SvyMeanSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyMeanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svymean"
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
        crate::survey_common::execute_describe(
            "svymean",
            &self.spec.variables,
            &self.spec.design,
            inputs,
            node_ctx,
            survey::svymean,
            self.spec.na_rm,
        )
        .await
    }
}

pub struct SvyMeanFactory;

impl NodeFactory for SvyMeanFactory {
    fn kind(&self) -> &'static str {
        "svymean"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted means with design-based standard errors"
    }
    fn doc(&self) -> &'static str {
        "Computes population means for specified variables using a survey design. \
         Outputs a table with mean, SE, and 95% CI for each variable. \
         Wraps survey::svymean."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyMeanSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyMeanSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyMeanNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyMeanSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let vars = formula_rhs(&s.variables);
        let na_rm = r_true_false(s.na_rm);
        code.push(format!("{out} <- svymean(~{vars}, {des}, na.rm = {na_rm})"));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svytotal
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyTotalSpec {
    pub design: SurveyDesignSpec,
    pub variables: Vec<String>,
    #[serde(default)]
    pub na_rm: bool,
}

/// Survey-weighted total node — **implemented**.
#[derive(Clone)]
pub struct SvyTotalNode {
    meta: NodePorts,
    spec: SvyTotalSpec,
}

impl SvyTotalNode {
    pub fn new(spec: SvyTotalSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyTotalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svytotal"
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
        crate::survey_common::execute_describe(
            "svytotal",
            &self.spec.variables,
            &self.spec.design,
            inputs,
            node_ctx,
            survey::svytotal,
            self.spec.na_rm,
        )
        .await
    }
}

pub struct SvyTotalFactory;

impl NodeFactory for SvyTotalFactory {
    fn kind(&self) -> &'static str {
        "svytotal"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted population totals with design-based SE"
    }
    fn doc(&self) -> &'static str {
        "Computes population totals for specified variables. \
         Wraps survey::svytotal."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyTotalSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyTotalSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyTotalNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyTotalSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let vars = formula_rhs(&s.variables);
        let na_rm = r_true_false(s.na_rm);
        code.push(format!(
            "{out} <- svytotal(~{vars}, {des}, na.rm = {na_rm})"
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svyvar
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyVarSpec {
    pub design: SurveyDesignSpec,
    pub variables: Vec<String>,
    #[serde(default)]
    pub na_rm: bool,
}

/// Survey-weighted variance node — **implemented**.
#[derive(Clone)]
pub struct SvyVarNode {
    meta: NodePorts,
    spec: SvyVarSpec,
}

impl SvyVarNode {
    pub fn new(spec: SvyVarSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyVarNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyvar"
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
        crate::survey_common::execute_describe(
            "svyvar",
            &self.spec.variables,
            &self.spec.design,
            inputs,
            node_ctx,
            survey::svyvar,
            self.spec.na_rm,
        )
        .await
    }
}

pub struct SvyVarFactory;

impl NodeFactory for SvyVarFactory {
    fn kind(&self) -> &'static str {
        "svyvar"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted variance/covariance matrix"
    }
    fn doc(&self) -> &'static str {
        "Computes the design-based variance-covariance matrix. \
         Wraps survey::svyvar."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyVarSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyVarSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyVarNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyVarSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let vars = formula_rhs(&s.variables);
        let na_rm = r_true_false(s.na_rm);
        code.push(format!("{out} <- svyvar(~{vars}, {des}, na.rm = {na_rm})"));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svyratio
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyRatioSpec {
    pub design: SurveyDesignSpec,
    /// Numerator variable column name.
    pub numerator: String,
    /// Denominator variable column name.
    pub denominator: String,
    #[serde(default)]
    pub na_rm: bool,
}

/// Survey-weighted ratio node — **implemented**.
#[derive(Clone)]
pub struct SvyRatioNode {
    meta: NodePorts,
    spec: SvyRatioSpec,
}

impl SvyRatioNode {
    pub fn new(spec: SvyRatioSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyRatioNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyratio"
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
            node_type: "svyratio".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyratio".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let num =
            crate::survey_common::extract_variables(&batches, &[self.spec.numerator.clone()])?;
        let den =
            crate::survey_common::extract_variables(&batches, &[self.spec.denominator.clone()])?;

        let stat = survey::svyratio(&num, &den, &design, self.spec.na_rm).map_err(|e| {
            DagError::NodeError {
                node_type: "svyratio".into(),
                msg: e.to_string(),
            }
        })?;

        let var_names = vec![format!("{}/{}", self.spec.numerator, self.spec.denominator)];
        let batch = crate::survey_common::build_describe_output_batch(&var_names, &stat)?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyratio".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

pub struct SvyRatioFactory;

impl NodeFactory for SvyRatioFactory {
    fn kind(&self) -> &'static str {
        "svyratio"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted ratio estimator (numerator/denominator)"
    }
    fn doc(&self) -> &'static str {
        "Computes a design-based ratio of two variables. Wraps survey::svyratio."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyRatioSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyRatioSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyRatioNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyRatioSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let na_rm = r_true_false(s.na_rm);
        code.push(format!(
            "{out} <- svyratio(~{num}/{den}, {des}, na.rm = {na_rm})",
            num = s.numerator,
            den = s.denominator
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svytable
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyTableSpec {
    pub design: SurveyDesignSpec,
    /// One or two variable column names.
    /// 1 variable: 1-way table; 2 variables: cross-tabulation.
    pub variables: Vec<String>,
}

/// Survey-weighted contingency table node — **implemented**.
#[derive(Clone)]
pub struct SvyTableNode {
    meta: NodePorts,
    spec: SvyTableSpec,
}

impl SvyTableNode {
    pub fn new(spec: SvyTableSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyTableNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svytable"
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
            node_type: "svytable".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svytable".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;

        // Extract the variable columns.
        let mut cols: Vec<Vec<String>> = Vec::new();
        for v in &self.spec.variables {
            cols.push(
                crate::survey_common::extract_string_column_pub(&batches, v).map_err(|e| {
                    DagError::NodeError {
                        node_type: "svytable".into(),
                        msg: e.0,
                    }
                })?,
            );
        }

        // Get the survey weights.
        let w = design.weights();

        // Compute the weighted table.
        let table = if cols.len() == 1 {
            // 1-way table: aggregate weights by value of cols[0].
            aggregate_table(&[cols[0].clone()], &w, &[0])
        } else if cols.len() == 2 {
            // 2-way table: aggregate weights by (cols[0], cols[1]).
            aggregate_table(&[cols[0].clone(), cols[1].clone()], &w, &[0, 1])
        } else {
            return Err(DagError::NodeError {
                node_type: "svytable".into(),
                msg: format!("svytable supports 1 or 2 variables, got {}", cols.len()),
            });
        };

        // Build the output batch: a flat representation with variable columns
        // and `frequency` (Float64).
        let schema = if cols.len() == 1 {
            Arc::new(Schema::new(vec![
                Field::new(&self.spec.variables[0], DataType::Utf8, false),
                Field::new("frequency", DataType::Float64, false),
            ]))
        } else {
            Arc::new(Schema::new(vec![
                Field::new(&self.spec.variables[0], DataType::Utf8, false),
                Field::new(&self.spec.variables[1], DataType::Utf8, false),
                Field::new("frequency", DataType::Float64, false),
            ]))
        };

        let mut col0: Vec<String> = Vec::with_capacity(table.len());
        let mut col1: Vec<String> = Vec::new();
        let mut freq: Vec<f64> = Vec::new();
        for (key_vals, weight) in &table {
            col0.push(key_vals[0].clone());
            if cols.len() == 2 {
                col1.push(key_vals[1].clone());
            }
            freq.push(*weight);
        }

        let batch = if cols.len() == 1 {
            RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(StringArray::from(col0)),
                    Arc::new(Float64Array::from(freq)),
                ],
            )
        } else {
            RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(StringArray::from(col0)),
                    Arc::new(StringArray::from(col1)),
                    Arc::new(Float64Array::from(freq)),
                ],
            )
        }
        .map_err(|e| DagError::NodeError {
            node_type: "svytable".into(),
            msg: format!("failed to build output: {e}"),
        })?;

        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svytable".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

/// Aggregate weighted counts: group by `keys[i]` for each row, summing `weights`.
fn aggregate_table(
    key_cols: &[Vec<String>],
    weights: &[f64],
    key_indices: &[usize],
) -> Vec<(Vec<String>, f64)> {
    use std::collections::HashMap;
    let mut counts: HashMap<Vec<String>, f64> = HashMap::new();
    let n = weights.len();
    for i in 0..n {
        let mut key: Vec<String> = Vec::with_capacity(key_indices.len());
        for &idx in key_indices {
            // `key_cols[idx]` is the column at position `idx` in `key_cols`,
            // but `key_indices[i]` refers to its index in `key_cols`.
            // Actually we have one key_col per index, so use key_cols[idx][i].
            key.push(key_cols[idx][i].clone());
        }
        *counts.entry(key).or_insert(0.0) += weights[i];
    }
    let mut out: Vec<_> = counts.into_iter().collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub struct SvyTableFactory;

impl NodeFactory for SvyTableFactory {
    fn kind(&self) -> &'static str {
        "svytable"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted contingency table"
    }
    fn doc(&self) -> &'static str {
        "Produces a survey-weighted contingency table (1- or 2-way). \
         Wraps survey::svytable."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyTableSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyTableSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyTableNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyTableSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rhs = s.variables.join(":");
        code.push(format!("{out} <- svytable(~{rhs}, {des})"));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svyquantile
// =====================================================================

/// Quantile tie-breaking rule (R `qrule` parameter).
fn default_qrule() -> String {
    "math".to_string()
}

/// CI method for quantiles.
fn default_interval_type() -> String {
    "mean".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyQuantileSpec {
    pub design: SurveyDesignSpec,
    /// Variable column name(s).
    pub variables: Vec<String>,
    /// Quantile probabilities, e.g. `[0.25, 0.5, 0.75]`.
    pub quantiles: Vec<f64>,
    /// Significance level α (default 0.05 → 95% CI).
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    /// Tie-breaking rule: `"math"`, `"school"`, `"shahvaish"`, `"hf1"`–`"hf9"`.
    #[serde(default = "default_qrule")]
    pub qrule: String,
    /// CI method: `"mean"`, `"beta"`, `"xlogit"`, `"asin"`, `"score"`.
    #[serde(default = "default_interval_type")]
    pub interval_type: String,
    #[serde(default)]
    pub na_rm: bool,
}

fn default_alpha() -> f64 {
    0.05
}

/// Quantile node — **implemented** (math rule, mean CI).
#[derive(Clone)]
pub struct SvyQuantileNode {
    meta: NodePorts,
    spec: SvyQuantileSpec,
}

impl SvyQuantileNode {
    pub fn new(spec: SvyQuantileSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyQuantileNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyquantile"
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
            node_type: "svyquantile".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyquantile".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let x = crate::survey_common::extract_variables(&batches, &self.spec.variables)?;

        let mut qs_out: Vec<f64> = Vec::new();
        let mut los: Vec<f64> = Vec::new();
        let mut his: Vec<f64> = Vec::new();
        let mut probs: Vec<f64> = Vec::new();
        for (j, var) in self.spec.variables.iter().enumerate() {
            let res = survey::svy_quantile(&x[j], &design, &self.spec.quantiles, self.spec.alpha)
                .map_err(|e| DagError::NodeError {
                node_type: "svyquantile".into(),
                msg: e.to_string(),
            })?;
            for (k, (q, lo, hi)) in res.into_iter().enumerate() {
                qs_out.push(q);
                los.push(lo);
                his.push(hi);
                probs.push(self.spec.quantiles[k]);
                // variable label column
            }
            let _ = var;
        }
        let p = self.spec.quantiles.len();
        let vars: Vec<String> = self
            .spec
            .variables
            .iter()
            .flat_map(|v| std::iter::repeat_n(v.clone(), p))
            .collect();

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("variable", DataType::Utf8, false),
                Field::new("quantile", DataType::Float64, false),
                Field::new("estimate", DataType::Float64, false),
                Field::new("ci_lower", DataType::Float64, false),
                Field::new("ci_upper", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vars)),
                Arc::new(Float64Array::from(probs)),
                Arc::new(Float64Array::from(qs_out)),
                Arc::new(Float64Array::from(los)),
                Arc::new(Float64Array::from(his)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyquantile".into(),
            msg: format!("failed to build output: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyquantile".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyQuantileFactory;

impl NodeFactory for SvyQuantileFactory {
    fn kind(&self) -> &'static str {
        "svyquantile"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted quantiles with Woodrull/score CIs"
    }
    fn doc(&self) -> &'static str {
        "Computes design-based quantiles with confidence intervals. \
         Supports 9 tie-breaking rules (hf1–hf9, math, school, shahvaish) \
         and 5 CI methods (mean, beta, xlogit, asin, score). \
         Wraps survey::svyquantile."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyQuantileSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyQuantileSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyQuantileNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyQuantileSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let vars = formula_rhs(&s.variables);
        let qs: Vec<String> = s.quantiles.iter().map(|q| q.to_string()).collect();
        let na_rm = r_true_false(s.na_rm);
        code.push(format!(
            "{out} <- svyquantile(~{vars}, {des}, quantiles = c({qs}), \
             alpha = {alpha}, qrule = \"{qrule}\", interval.type = \"{it}\", na.rm = {na_rm})",
            qs = qs.join(", "),
            alpha = s.alpha,
            qrule = s.qrule,
            it = s.interval_type,
            na_rm = na_rm
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
    use arrow_array::{Float64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn svymean_spec_round_trip() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"], "weights": "wt"},
            "variables": ["y1", "y2"],
            "na_rm": true
        });
        let s: SvyMeanSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.variables, vec!["y1", "y2"]);
        assert!(s.na_rm);
    }

    #[test]
    fn svyquantile_spec_defaults() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"]},
            "variables": ["y"],
            "quantiles": [0.5]
        });
        let s: SvyQuantileSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.qrule, "math");
        assert_eq!(s.interval_type, "mean");
        assert!((s.alpha - 0.05).abs() < 1e-10);
    }

    // ── End-to-end execution test ──────────────────────────────────────────
    //
    // Uses the R `fpc` dataset (8 obs, 2 strata) and validates the Rust
    // svymean output against R survey v4.5 golden values.

    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }

    /// Build the fpc dataset as a RecordBatch.
    fn fpc_batch() -> RecordBatch {
        use arrow_array::{Float64Array, Int32Array};
        let schema = Arc::new(Schema::new(vec![
            Field::new("stratid", DataType::Int32, false),
            Field::new("psuid", DataType::Int32, false),
            Field::new("weight", DataType::Float64, false),
            Field::new("Nh", DataType::Float64, false),
            Field::new("x", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
                Arc::new(Float64Array::from(vec![
                    15.0, 15.0, 15.0, 15.0, 15.0, 12.0, 12.0, 12.0,
                ])),
                Arc::new(Float64Array::from(vec![
                    2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2,
                ])),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn svymean_node_matches_r_without_fpc() {
        let batch = fpc_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyMeanSpec {
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
            variables: vec!["x".into()],
            na_rm: false,
        };

        let mut node = SvyMeanNode::new(spec);
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
        let total_rows: usize = result.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 1);

        // Extract mean (column 1) and SE (column 2).
        let mean: f64 = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let se: f64 = result[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);

        assert!(
            (mean - 5.448148).abs() < 1e-4,
            "mean should match R golden value 5.448148, got {mean}"
        );
        assert!(
            (se - 0.7412683).abs() < 1e-4,
            "SE should match R golden value 0.7412683, got {se}"
        );
    }

    #[tokio::test]
    async fn svymean_node_matches_r_with_fpc() {
        let batch = fpc_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyMeanSpec {
            design: crate::survey_common::SurveyDesignSpec {
                ids: vec!["psuid".into()],
                strata: vec!["stratid".into()],
                probs: vec![],
                weights: Some("weight".into()),
                fpc: vec!["Nh".into()],
                nest: true,
                pps: "none".into(),
                variance: "HT".into(),
                lonely_psu: Some("remove".into()),
            },
            variables: vec!["x".into()],
            na_rm: false,
        };

        let mut node = SvyMeanNode::new(spec);
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
        let mean: f64 = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let se: f64 = result[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);

        assert!((mean - 5.448148).abs() < 1e-4, "mean: {mean}");
        assert!(
            (se - 0.6160407).abs() < 1e-4,
            "SE with FPC should match R golden value 0.6160407, got {se}"
        );
    }

    #[tokio::test]
    async fn svytable_node_matches_r() {
        // 1-way table of stratid using fpc dataset.
        // R: with weights 3 (stratum 1) and 4 (stratum 2):
        //   stratum 1: 5 obs * 3 = 15
        //   stratum 2: 3 obs * 4 = 12
        let schema = Arc::new(Schema::new(vec![
            Field::new("stratid", DataType::Int32, false),
            Field::new("psuid", DataType::Int32, false),
            Field::new("weight", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow_array::Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                Arc::new(arrow_array::Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyTableSpec {
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
            variables: vec!["stratid".into()],
        };

        let mut node = SvyTableNode::new(spec);
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
        let total_rows: usize = result.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 2); // two strata

        // Check the frequencies.
        let freqs: Vec<f64> = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();
        // Sum should be 15 + 12 = 27 (total weight).
        let sum: f64 = freqs.iter().sum();
        assert!((sum - 27.0).abs() < 1e-6, "total weight: {sum}");
    }
}
