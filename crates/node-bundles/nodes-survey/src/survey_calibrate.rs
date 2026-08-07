//! Survey weight-calibration nodes.
//!
//! Nodes: `post_stratify`, `rake`, `calibrate`, `trim_weights`. Each takes a
//! data DataFrame + [`SurveyDesignSpec`] and outputs the **same data with an
//! added weight column** (named `calibrated_weight` / `raked_weight` / etc.),
//! so downstream nodes can reference it in their `design.weights`.
//!
//! R package reference: `survey::postStratify`, `rake`, `calibrate`,
//! `trimWeights`.

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use crate::survey_common::{
    SurveyDesignSpec, build_survey_design, formula_rhs, gen_design_r, one_in_one_out, r_true_false,
};
use dag_core::codegen::helpers::{input_0, parse_spec};
use dag_core::codegen::{CodegenCtx, CodegenError, NodeCodegen};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

// =====================================================================
// post_stratify
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PostStratifySpec {
    pub design: SurveyDesignSpec,
    /// Post-stratification variable column name(s).
    pub strata: Vec<String>,
    /// Population totals as `{level: count}` pairs, e.g.
    /// `{"A": 100, "B": 200}`. For multi-variable post-strat, use a
    /// flat key like `"A:Male"`.
    pub population: std::collections::HashMap<String, f64>,
    /// If `true`, silently ignore population strata absent from sample
    /// (R `partial = TRUE`).
    #[serde(default)]
    pub partial: bool,
    /// Output weight column name.
    #[serde(default = "default_ps_weight_col")]
    pub weight_col: String,
}

fn default_ps_weight_col() -> String {
    "ps_weight".to_string()
}

/// Post-stratify node — **implemented**.
#[derive(Clone)]
pub struct PostStratifyNode {
    meta: NodePorts,
    spec: PostStratifySpec,
}

impl PostStratifyNode {
    pub fn new(spec: PostStratifySpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for PostStratifyNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "post_stratify"
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
            node_type: "post_stratify".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "post_stratify".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = build_survey_design(&self.spec.design, &batches)?;
        let strata_vec =
            crate::survey_common::extract_string_column_pub(&batches, &self.spec.strata[0])
                .map_err(|e| DagError::NodeError {
                    node_type: "post_stratify".into(),
                    msg: e.0,
                })?;

        let new_design = survey::post_stratify(
            &design,
            &strata_vec,
            &self.spec.population,
            self.spec.partial,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "post_stratify".into(),
            msg: e.to_string(),
        })?;

        let new_weights = new_design.weights();

        // Build output batch: original data + new weight column.
        // Concatenate input batches via arrow::compute.
        let schema_ref = batches[0].schema_ref();
        let combined = arrow::compute::concat_batches(&schema_ref, &batches).map_err(|e| {
            DagError::NodeError {
                node_type: "post_stratify".into(),
                msg: format!("concat failed: {e}"),
            }
        })?;

        let new_schema = Arc::new(Schema::new(
            combined
                .schema()
                .fields()
                .iter()
                .cloned()
                .chain(std::iter::once(Arc::new(Field::new(
                    &self.spec.weight_col,
                    DataType::Float64,
                    false,
                ))))
                .collect::<Vec<_>>(),
        ));

        let mut new_columns: Vec<Arc<dyn arrow_array::Array>> =
            combined.columns().iter().cloned().collect();
        new_columns.push(Arc::new(Float64Array::from(new_weights)));

        let output_batch =
            RecordBatch::try_new(new_schema, new_columns).map_err(|e| DagError::NodeError {
                node_type: "post_stratify".into(),
                msg: format!("failed to build output: {e}"),
            })?;

        let ctx = node_ctx.session();
        let df_out = ctx
            .read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: "post_stratify".into(),
                msg: format!("read_batch failed: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct PostStratifyFactory;

impl NodeFactory for PostStratifyFactory {
    fn kind(&self) -> &'static str {
        "post_stratify"
    }
    fn desc(&self) -> &'static str {
        "Post-stratify a survey design to known population totals"
    }
    fn doc(&self) -> &'static str {
        "Adjusts sampling weights so that marginal totals match known \
         population counts. Outputs the original data with an added weight \
         column. Wraps survey::postStratify."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PostStratifySpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: PostStratifySpec = serde_json::from_value(spec)?;
        Ok(Box::new(PostStratifyNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<PostStratifySpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);

        // Build the population data.frame in R.
        let levels: Vec<String> = s.population.keys().cloned().collect();
        let counts: Vec<String> = s.population.values().map(|v| v.to_string()).collect();
        let pop_var = ctx.fresh_var("pop");
        let strat_rhs = formula_rhs(&s.strata);
        let strat_single = &s.strata[0];
        code.push(format!(
            "{pop_var} <- data.frame({strat} = c({lvls}), Freq = c({cnts}))",
            strat = strat_single,
            lvls = levels
                .iter()
                .map(|l| format!("\"{l}\""))
                .collect::<Vec<_>>()
                .join(", "),
            cnts = counts.join(", ")
        ));

        let partial_arg = if s.partial { ", partial = TRUE" } else { "" };
        let des_ps = ctx.fresh_var("des_ps");
        code.push(format!(
            "{des_ps} <- postStratify({des}, ~{strat_rhs}, {pop_var}{pa})",
            pa = partial_arg
        ));
        code.push(format!(
            "{out} <- cbind({input}, {wc} = weights({des_ps}))",
            wc = s.weight_col
        ));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// rake
// =====================================================================

/// One raking margin (a variable + its population distribution).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RakeMargin {
    /// Variable column name for this margin.
    pub variable: String,
    /// Population totals for each level of the variable.
    pub population: std::collections::HashMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RakeSpec {
    pub design: SurveyDesignSpec,
    /// Raking margins (one per variable to rake on).
    pub margins: Vec<RakeMargin>,
    /// Maximum iterations.
    #[serde(default = "default_maxit")]
    pub maxit: usize,
    /// Convergence threshold (relative to total weight).
    #[serde(default = "default_epsilon")]
    pub epsilon: f64,
    /// Output weight column name.
    #[serde(default = "default_rake_weight_col")]
    pub weight_col: String,
}

fn default_maxit() -> usize {
    10
}

fn default_epsilon() -> f64 {
    1.0
}

fn default_rake_weight_col() -> String {
    "raked_weight".to_string()
}

/// Rake node — **implemented**.
#[derive(Clone)]
pub struct RakeNode {
    meta: NodePorts,
    spec: RakeSpec,
}

impl RakeNode {
    pub fn new(spec: RakeSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for RakeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "rake"
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
            node_type: "rake".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "rake".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = build_survey_design(&self.spec.design, &batches)?;

        // Build the margins: (column values, population map) per margin.
        let mut margins: Vec<(Vec<String>, std::collections::HashMap<String, f64>)> = Vec::new();
        for m in &self.spec.margins {
            let col = crate::survey_common::extract_string_column_pub(&batches, &m.variable)
                .map_err(|e| DagError::NodeError {
                    node_type: "rake".into(),
                    msg: e.0,
                })?;
            margins.push((col, m.population.clone()));
        }

        let new_design = survey::rake(&design, &margins, self.spec.maxit, self.spec.epsilon)
            .map_err(|e| DagError::NodeError {
                node_type: "rake".into(),
                msg: e.to_string(),
            })?;

        let new_weights = new_design.weights();

        // Build output: original data + raked weight column.
        let schema_ref = batches[0].schema_ref();
        let combined = arrow::compute::concat_batches(&schema_ref, &batches).map_err(|e| {
            DagError::NodeError {
                node_type: "rake".into(),
                msg: format!("concat failed: {e}"),
            }
        })?;

        let new_schema = Arc::new(Schema::new(
            combined
                .schema()
                .fields()
                .iter()
                .cloned()
                .chain(std::iter::once(Arc::new(Field::new(
                    &self.spec.weight_col,
                    DataType::Float64,
                    false,
                ))))
                .collect::<Vec<_>>(),
        ));

        let mut new_columns: Vec<Arc<dyn arrow_array::Array>> =
            combined.columns().iter().cloned().collect();
        new_columns.push(Arc::new(Float64Array::from(new_weights)));

        let output_batch =
            RecordBatch::try_new(new_schema, new_columns).map_err(|e| DagError::NodeError {
                node_type: "rake".into(),
                msg: format!("failed to build output: {e}"),
            })?;

        let ctx = node_ctx.session();
        let df_out = ctx
            .read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: "rake".into(),
                msg: format!("read_batch failed: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct RakeFactory;

impl NodeFactory for RakeFactory {
    fn kind(&self) -> &'static str {
        "rake"
    }
    fn desc(&self) -> &'static str {
        "Rake (iterative proportional fitting) a survey design"
    }
    fn doc(&self) -> &'static str {
        "Adjusts sampling weights by iterative proportional fitting so that \
         each marginal variable matches its population distribution. \
         Wraps survey::rake."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RakeSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: RakeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RakeNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<RakeSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);

        // Build sample.margins list and population.margins list.
        let sm_var = ctx.fresh_var("sm");
        let pm_var = ctx.fresh_var("pm");
        let sm_items: Vec<String> = s
            .margins
            .iter()
            .map(|m| format!("~{}", m.variable))
            .collect();
        code.push(format!("{sm_var} <- list({})", sm_items.join(", ")));

        let pm_items: Vec<String> = s
            .margins
            .iter()
            .map(|m| {
                let levels: Vec<String> = m.population.keys().cloned().collect();
                let counts: Vec<String> = m.population.values().map(|v| v.to_string()).collect();
                format!(
                    "data.frame({var} = c({lvls}), Freq = c({cnts}))",
                    var = m.variable,
                    lvls = levels
                        .iter()
                        .map(|l| format!("\"{l}\""))
                        .collect::<Vec<_>>()
                        .join(", "),
                    cnts = counts.join(", ")
                )
            })
            .collect();
        code.push(format!("{pm_var} <- list({})", pm_items.join(", ")));

        let des_raked = ctx.fresh_var("des_raked");
        code.push(format!(
            "{des_raked} <- rake({des}, sample.margins = {sm_var}, \
             population.margins = {pm_var}, control = list(maxit = {mi}, epsilon = {eps}))",
            mi = s.maxit,
            eps = s.epsilon
        ));
        code.push(format!(
            "{out} <- cbind({input}, {wc} = weights({des_raked}))",
            wc = s.weight_col
        ));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// calibrate
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CalibrateSpec {
    pub design: SurveyDesignSpec,
    /// Auxiliary variable column name(s) for calibration.
    pub variables: Vec<String>,
    /// Population totals for each auxiliary variable, aligned with `variables`.
    pub population_totals: Vec<f64>,
    /// Calibration distance function: `"linear"`, `"raking"`, `"logit"`, `"sinh"`.
    #[serde(default = "default_calfun")]
    pub calfun: String,
    /// Bounds for bounded calibration (only used when calfun != "linear").
    /// For logit: `[lower, upper]` on the weight ratio.
    #[serde(default)]
    pub bounds: Option<Vec<f64>>,
    /// Maximum iterations.
    #[serde(default = "default_cal_maxit")]
    pub maxit: usize,
    /// Convergence threshold.
    #[serde(default = "default_cal_epsilon")]
    pub epsilon: f64,
    /// Output weight column name.
    #[serde(default = "default_cal_weight_col")]
    pub weight_col: String,
}

fn default_calfun() -> String {
    "linear".to_string()
}

fn default_cal_maxit() -> usize {
    50
}

fn default_cal_epsilon() -> f64 {
    1e-7
}

fn default_cal_weight_col() -> String {
    "calibrated_weight".to_string()
}

/// Calibrate node — **implemented** (linear/regcalibrate path).
#[derive(Clone)]
pub struct CalibrateNode {
    meta: NodePorts,
    spec: CalibrateSpec,
}

impl CalibrateNode {
    pub fn new(spec: CalibrateSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for CalibrateNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "calibrate"
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
        // Only the linear (regcalibrate) path is implemented in Rust.
        if self.spec.calfun != "linear" {
            return Err(DagError::NodeError {
                node_type: "calibrate".into(),
                msg: format!(
                    "Rust execution only supports calfun='linear'; requested '{}'. \
                     Use codegen_r for R code.",
                    self.spec.calfun
                ),
            });
        }
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "calibrate".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "calibrate".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = build_survey_design(&self.spec.design, &batches)?;
        let aux = crate::survey_common::extract_variables(&batches, &self.spec.variables)?;

        let new_design = survey::calibrate_linear(&design, &aux, &self.spec.population_totals)
            .map_err(|e| DagError::NodeError {
                node_type: "calibrate".into(),
                msg: e.to_string(),
            })?;
        let new_weights = new_design.weights();

        // Output: original data + calibrated weight column.
        let schema_ref = batches[0].schema_ref();
        let combined = arrow::compute::concat_batches(&schema_ref, &batches).map_err(|e| {
            DagError::NodeError {
                node_type: "calibrate".into(),
                msg: format!("concat failed: {e}"),
            }
        })?;
        let new_schema = Arc::new(Schema::new(
            combined
                .schema()
                .fields()
                .iter()
                .cloned()
                .chain(std::iter::once(Arc::new(Field::new(
                    &self.spec.weight_col,
                    DataType::Float64,
                    false,
                ))))
                .collect::<Vec<_>>(),
        ));
        let mut new_columns: Vec<Arc<dyn arrow_array::Array>> =
            combined.columns().iter().cloned().collect();
        new_columns.push(Arc::new(Float64Array::from(new_weights)));
        let output_batch =
            RecordBatch::try_new(new_schema, new_columns).map_err(|e| DagError::NodeError {
                node_type: "calibrate".into(),
                msg: format!("failed to build output: {e}"),
            })?;
        let ctx = node_ctx.session();
        let df_out = ctx
            .read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: "calibrate".into(),
                msg: format!("read_batch failed: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct CalibrateFactory;

impl NodeFactory for CalibrateFactory {
    fn kind(&self) -> &'static str {
        "calibrate"
    }
    fn desc(&self) -> &'static str {
        "Calibrate survey weights (generalised raking) to auxiliary totals"
    }
    fn doc(&self) -> &'static str {
        "Adjusts sampling weights via generalised raking (linear, raking, \
         logit, or sinh distance) so that weighted totals of auxiliary \
         variables match known population totals. \
         Wraps survey::calibrate."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CalibrateSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: CalibrateSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CalibrateNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<CalibrateSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);

        let totals: String = s
            .population_totals
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let vars = formula_rhs(&s.variables);

        let mut extra_args = format!(
            ", calfun = \"{}\", maxit = {}, epsilon = {}",
            s.calfun, s.maxit, s.epsilon
        );
        if let Some(bounds) = &s.bounds {
            if bounds.len() >= 2 {
                extra_args.push_str(&format!(", bounds = c({}, {})", bounds[0], bounds[1]));
            }
        }

        let des_cal = ctx.fresh_var("des_cal");
        code.push(format!(
            "{des_cal} <- calibrate({des}, ~{vars}, c({totals}){ea})",
            ea = extra_args
        ));
        code.push(format!(
            "{out} <- cbind({input}, {wc} = weights({des_cal}))",
            wc = s.weight_col
        ));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// trim_weights
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TrimWeightsSpec {
    pub design: SurveyDesignSpec,
    /// Upper bound on weights. `null` = no upper bound.
    #[serde(default)]
    pub upper: Option<f64>,
    /// Lower bound on weights. `null` = no lower bound.
    #[serde(default)]
    pub lower: Option<f64>,
    /// If `true`, iteratively trim until all weights are within bounds
    /// (R `strict = TRUE`).
    #[serde(default)]
    pub strict: bool,
    /// Output weight column name.
    #[serde(default = "default_trim_weight_col")]
    pub weight_col: String,
}

fn default_trim_weight_col() -> String {
    "trimmed_weight".to_string()
}

/// Trim-weights node — **implemented**.
#[derive(Clone)]
pub struct TrimWeightsNode {
    meta: NodePorts,
    spec: TrimWeightsSpec,
}

impl TrimWeightsNode {
    pub fn new(spec: TrimWeightsSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for TrimWeightsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "trim_weights"
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
            node_type: "trim_weights".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "trim_weights".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = build_survey_design(&self.spec.design, &batches)?;
        let upper = self.spec.upper.unwrap_or(f64::INFINITY);
        let lower = self.spec.lower.unwrap_or(f64::NEG_INFINITY);

        let new_design =
            survey::trim_weights(&design, upper, lower, self.spec.strict).map_err(|e| {
                DagError::NodeError {
                    node_type: "trim_weights".into(),
                    msg: e.to_string(),
                }
            })?;

        let new_weights = new_design.weights();

        // Build output: original data + trimmed weight column.
        let schema_ref = batches[0].schema_ref();
        let combined = arrow::compute::concat_batches(&schema_ref, &batches).map_err(|e| {
            DagError::NodeError {
                node_type: "trim_weights".into(),
                msg: format!("concat failed: {e}"),
            }
        })?;
        let new_schema = Arc::new(Schema::new(
            combined
                .schema()
                .fields()
                .iter()
                .cloned()
                .chain(std::iter::once(Arc::new(Field::new(
                    &self.spec.weight_col,
                    DataType::Float64,
                    false,
                ))))
                .collect::<Vec<_>>(),
        ));
        let mut new_columns: Vec<Arc<dyn arrow_array::Array>> =
            combined.columns().iter().cloned().collect();
        new_columns.push(Arc::new(Float64Array::from(new_weights)));

        let output_batch =
            RecordBatch::try_new(new_schema, new_columns).map_err(|e| DagError::NodeError {
                node_type: "trim_weights".into(),
                msg: format!("failed to build output: {e}"),
            })?;
        let ctx = node_ctx.session();
        let df_out = ctx
            .read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: "trim_weights".into(),
                msg: format!("read_batch failed: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct TrimWeightsFactory;

impl NodeFactory for TrimWeightsFactory {
    fn kind(&self) -> &'static str {
        "trim_weights"
    }
    fn desc(&self) -> &'static str {
        "Trim extreme survey weights to upper/lower bounds"
    }
    fn doc(&self) -> &'static str {
        "Trims sampling weights to within specified bounds, redistributing \
         the excess. Wraps survey::trimWeights."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TrimWeightsSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: TrimWeightsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TrimWeightsNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<TrimWeightsSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);

        let upper = s
            .upper
            .map(|v| v.to_string())
            .unwrap_or_else(|| "Inf".to_string());
        let lower = s
            .lower
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-Inf".to_string());
        let strict = r_true_false(s.strict);

        let des_trim = ctx.fresh_var("des_trim");
        code.push(format!(
            "{des_trim} <- trimWeights({des}, upper = {u}, lower = {lo}, strict = {st})",
            u = upper,
            lo = lower,
            st = strict
        ));
        code.push(format!(
            "{out} <- cbind({input}, {wc} = weights({des_trim}))",
            wc = s.weight_col
        ));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Float64Array, Int32Array};
    use arrow_schema::Schema;
    use std::sync::Arc;

    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }

    #[tokio::test]
    async fn post_stratify_node_doubles_stratum_2() {
        // fpc dataset; post-stratify so that stratum 2's population count is
        // doubled from 12 to 24. Weights in stratum 2 should double.
        let schema = Arc::new(Schema::new(vec![
            arrow_schema::Field::new("stratid", arrow_schema::DataType::Int32, false),
            arrow_schema::Field::new("psuid", arrow_schema::DataType::Int32, false),
            arrow_schema::Field::new("weight", arrow_schema::DataType::Float64, false),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let mut pop = std::collections::HashMap::new();
        pop.insert("1".into(), 15.0);
        pop.insert("2".into(), 24.0);

        let spec = PostStratifySpec {
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
            strata: vec!["stratid".into()],
            population: pop,
            partial: false,
            weight_col: "ps_weight".into(),
        };

        let mut node = PostStratifyNode::new(spec);
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
        assert_eq!(total_rows, 8);

        // Find the ps_weight column index.
        let schema = result[0].schema();
        let idx = schema
            .index_of("ps_weight")
            .expect("ps_weight column missing");
        let new_weights: Vec<f64> = result[0]
            .column(idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();

        // Stratum 1: unchanged (15/15 = 1.0 ratio, weight = 3.0).
        for i in 0..5 {
            assert!(
                (new_weights[i] - 3.0).abs() < 1e-9,
                "stratum 1 weight[{}]: {}",
                i,
                new_weights[i]
            );
        }
        // Stratum 2: doubled (24/12 = 2.0 ratio, weight = 4.0 * 2 = 8.0).
        for i in 5..8 {
            assert!(
                (new_weights[i] - 8.0).abs() < 1e-9,
                "stratum 2 weight[{}]: {}",
                i,
                new_weights[i]
            );
        }
    }

    #[tokio::test]
    async fn rake_node_single_margin_matches_poststratify() {
        // Rake on the strata margin with population {1:20, 2:16}.
        // This is equivalent to post_stratify.
        let schema = Arc::new(Schema::new(vec![
            arrow_schema::Field::new("stratid", arrow_schema::DataType::Int32, false),
            arrow_schema::Field::new("psuid", arrow_schema::DataType::Int32, false),
            arrow_schema::Field::new("weight", arrow_schema::DataType::Float64, false),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let mut pop1 = std::collections::HashMap::new();
        pop1.insert("1".into(), 20.0);
        pop1.insert("2".into(), 16.0);

        let spec = RakeSpec {
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
            margins: vec![RakeMargin {
                variable: "stratid".into(),
                population: pop1,
            }],
            maxit: 100,
            epsilon: 1e-9,
            weight_col: "raked_weight".into(),
        };

        let mut node = RakeNode::new(spec);
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
        let schema = result[0].schema();
        let idx = schema
            .index_of("raked_weight")
            .expect("raked_weight column missing");
        let new_weights: Vec<f64> = result[0]
            .column(idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();

        // ratio = 20/15 = 4/3 for stratum 1 → weight = 3 * 4/3 = 4.0
        // ratio = 16/12 = 4/3 for stratum 2 → weight = 4 * 4/3 = 16/3 ≈ 5.333
        for i in 0..5 {
            assert!(
                (new_weights[i] - 4.0).abs() < 1e-6,
                "s1 w[{}]: {}",
                i,
                new_weights[i]
            );
        }
        for i in 5..8 {
            assert!(
                (new_weights[i] - 16.0 / 3.0).abs() < 1e-6,
                "s2 w[{}]: {}",
                i,
                new_weights[i]
            );
        }
    }

    #[test]
    fn calibrate_spec_defaults() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"], "weights": "wt"},
            "variables": ["x1", "x2"],
            "population_totals": [100.0, 200.0]
        });
        let s: CalibrateSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.calfun, "linear");
        assert_eq!(s.maxit, 50);
        assert!((s.epsilon - 1e-7).abs() < 1e-15);
    }

    #[test]
    fn rake_spec_with_margins() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"]},
            "margins": [
                {"variable": "sex", "population": {"M": 100, "F": 110}},
                {"variable": "age", "population": {"young": 50, "old": 160}}
            ]
        });
        let s: RakeSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.margins.len(), 2);
        assert_eq!(s.margins[0].variable, "sex");
    }
}
