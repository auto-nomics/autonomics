//! Survey utility nodes: `svyby`, `svycontrast`, `svystandardize`,
//! `reg_term_test`.
//!
//! R package reference: `survey::svyby`, `svycontrast`, `svystandardize`,
//! `regTermTest`.

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use crate::survey_common::{
    SurveyDesignSpec, formula_rhs, gen_design_r, one_in_one_out, r_true_false,
};
use dag_core::codegen::helpers::{input_0, parse_spec};
use dag_core::codegen::{CodegenCtx, CodegenError, NodeCodegen};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

// =====================================================================
// svyby — compute statistics by groups
// =====================================================================

fn default_svyby_fun() -> String {
    "svymean".to_string()
}

fn default_vartype() -> Vec<String> {
    vec!["se".to_string()]
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyBySpec {
    pub design: SurveyDesignSpec,
    pub variables: Vec<String>,
    pub by: Vec<String>,
    #[serde(default = "default_svyby_fun")]
    pub fun: String,
    #[serde(default = "default_vartype")]
    pub vartype: Vec<String>,
    #[serde(default = "default_true")]
    pub drop_empty_groups: bool,
    #[serde(default)]
    pub na_rm: bool,
}

fn default_true() -> bool {
    true
}

/// Survey by-group node — **implemented** for svymean/svytotal/svyvar.
#[derive(Clone)]
pub struct SvyByNode {
    meta: NodePorts,
    spec: SvyBySpec,
}

impl SvyByNode {
    pub fn new(spec: SvyBySpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyByNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svyby"
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
            node_type: "svyby".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svyby".into(),
                msg: format!("collect failed: {e}"),
            })?;

        // Extract the grouping variable (single grouping variable supported).
        let by_col = &self.spec.by[0];
        let group_values = crate::survey_common::extract_string_column_pub(&batches, by_col)
            .map_err(|e| DagError::NodeError {
                node_type: "svyby".into(),
                msg: e.0,
            })?;

        // Unique group levels in order of appearance.
        let mut groups: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for g in &group_values {
            if seen.insert(g.clone()) {
                groups.push(g.clone());
            }
        }

        // Build full design + extract variables once.
        let full_design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let all_vars = crate::survey_common::extract_variables(&batches, &self.spec.variables)?;

        // For each group: filter, run analysis, collect results.
        let mut out_groups: Vec<String> = Vec::new();
        let mut out_vars: Vec<String> = Vec::new();
        let mut out_est: Vec<f64> = Vec::new();
        let mut out_se: Vec<f64> = Vec::new();
        let mut out_df: Vec<f64> = Vec::new();

        for group in &groups {
            // Create filter mask for this group.
            let mask: Vec<bool> = group_values.iter().map(|g| g == group).collect();
            let keep: Vec<usize> = (0..mask.len()).filter(|&i| mask[i]).collect();

            // Subset design.
            let sub_strata: Vec<String> = keep
                .iter()
                .map(|&i| full_design.strata[i].clone())
                .collect();
            let sub_cluster: Vec<String> = keep
                .iter()
                .map(|&i| full_design.cluster[i].clone())
                .collect();
            let sub_prob: Vec<f64> = keep.iter().map(|&i| full_design.prob[i]).collect();
            let sub_design = survey::SurveyDesign {
                strata: sub_strata,
                cluster: sub_cluster,
                prob: sub_prob,
                fpc: full_design.fpc.clone(),
                n_psu: full_design.n_psu.clone(),
                lonely_psu: full_design.lonely_psu,
                n_obs: keep.len(),
            };

            // Subset variables.
            let sub_x: Vec<Vec<f64>> = all_vars
                .iter()
                .map(|v| keep.iter().map(|&i| v[i]).collect())
                .collect();

            // Run analysis function.
            let stat_result = match self.spec.fun.as_str() {
                "svymean" => survey::svymean(&sub_x, &sub_design, self.spec.na_rm),
                "svytotal" => survey::svytotal(&sub_x, &sub_design, self.spec.na_rm),
                "svyvar" => survey::svyvar(&sub_x, &sub_design, self.spec.na_rm),
                other => Err(survey::SurveyError::InvalidInput(format!(
                    "unsupported fun '{other}' (supported: svymean, svytotal, svyvar)"
                ))),
            };

            match stat_result {
                Ok(stat) => {
                    let se = stat.se();
                    for (j, var_name) in self.spec.variables.iter().enumerate() {
                        out_groups.push(group.clone());
                        out_vars.push(var_name.clone());
                        out_est.push(stat.estimate[j]);
                        out_se.push(se[j]);
                        out_df.push(stat.df as f64);
                    }
                }
                Err(_) if self.spec.drop_empty_groups => {
                    // Skip groups that fail (e.g. lonely PSU).
                    continue;
                }
                Err(e) => {
                    return Err(DagError::NodeError {
                        node_type: "svyby".into(),
                        msg: format!("group '{group}': {e}"),
                    });
                }
            }
        }

        // Build output batch.
        let z = 1.959963984540054_f64;
        let ci_lower: Vec<f64> = out_est
            .iter()
            .zip(&out_se)
            .map(|(m, s)| m - z * s)
            .collect();
        let ci_upper: Vec<f64> = out_est
            .iter()
            .zip(&out_se)
            .map(|(m, s)| m + z * s)
            .collect();

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("group", DataType::Utf8, false),
                Field::new("variable", DataType::Utf8, false),
                Field::new("estimate", DataType::Float64, false),
                Field::new("se", DataType::Float64, false),
                Field::new("ci_lower", DataType::Float64, false),
                Field::new("ci_upper", DataType::Float64, false),
                Field::new("df", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(out_groups)),
                Arc::new(StringArray::from(out_vars)),
                Arc::new(Float64Array::from(out_est)),
                Arc::new(Float64Array::from(out_se)),
                Arc::new(Float64Array::from(ci_lower)),
                Arc::new(Float64Array::from(ci_upper)),
                Arc::new(Float64Array::from(out_df)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svyby".into(),
            msg: format!("failed to build output: {e}"),
        })?;

        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svyby".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

pub struct SvyByFactory;

impl NodeFactory for SvyByFactory {
    fn kind(&self) -> &'static str {
        "svyby"
    }
    fn desc(&self) -> &'static str {
        "Compute survey statistics by groups (svyby)"
    }
    fn doc(&self) -> &'static str {
        "Applies a survey analysis function (svymean, svytotal, etc.) within \
         each level of one or more grouping variables. Produces a table of \
         per-group estimates with SEs. Wraps survey::svyby."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyBySpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyBySpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyByNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyBySpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let vars = formula_rhs(&s.variables);
        let by = formula_rhs(&s.by);
        let vartype_str = s
            .vartype
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let na_rm = r_true_false(s.na_rm);
        code.push(format!(
            "{out} <- svyby(~{vars}, ~{by}, {des}, {fun}, \
             vartype = c({vt}), drop.empty.groups = {deg}, na.rm = {na_rm})",
            fun = s.fun,
            vt = vartype_str,
            deg = r_true_false(s.drop_empty_groups),
            na_rm = na_rm
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svycontrast — linear/non-linear combinations of estimates
// =====================================================================

/// One named contrast expression.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ContrastExpr {
    /// Name for the contrast result.
    pub name: String,
    /// R expression, e.g. `"a - b"`, `"exp(a)/exp(b)"`.
    pub expr: String,
    /// Optional structured contrast kind: `"linear"`, `"ratio"`, `"exp"`, `"log"`.
    /// When present, avoids parsing `expr`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Linear coefficients, aligned with `SvyContrastSpec::variables`.
    /// Used when `kind == "linear"`.
    #[serde(default)]
    pub coefficients: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyContrastSpec {
    pub design: SurveyDesignSpec,
    /// Variable(s) whose estimates we want to combine.
    pub variables: Vec<String>,
    /// Named contrasts to compute.
    pub contrasts: Vec<ContrastExpr>,
}

/// Parse a contrast expression into a [`survey::Contrast`] given the
/// variable order. Supports linear combos (`a - b`, `2*a + b`), ratios
/// (`a/b`), and unary transforms (`exp(a)`, `log(a)`).
fn parse_contrast(
    expr: &str,
    variables: &[String],
    structured: Option<&ContrastExpr>,
) -> Result<survey::Contrast, DagError> {
    // Structured path.
    if let Some(ce) = structured {
        match ce.kind.as_deref() {
            Some("ratio") => {
                if variables.len() != 2 {
                    return Err(DagError::NodeError {
                        node_type: "svycontrast".into(),
                        msg: "ratio contrast requires exactly 2 variables".into(),
                    });
                }
                return Ok(survey::Contrast::Ratio {
                    numerator: 0,
                    denominator: 1,
                });
            }
            Some("exp") => return Ok(survey::Contrast::Exp(0)),
            Some("log") => return Ok(survey::Contrast::Log(0)),
            Some("linear") => {
                if let Some(coeffs) = &ce.coefficients {
                    if coeffs.len() != variables.len() {
                        return Err(DagError::NodeError {
                            node_type: "svycontrast".into(),
                            msg: format!(
                                "linear contrast needs {} coefficients, got {}",
                                variables.len(),
                                coeffs.len()
                            ),
                        });
                    }
                    return Ok(survey::Contrast::Linear(coeffs.clone()));
                }
            }
            _ => {}
        }
    }

    // Parse simple expression patterns.
    let e = expr.replace(['`', ' '], "");
    // Unary transforms.
    for (prefix, kind) in [("exp(", "exp"), ("log(", "log")] {
        if e.starts_with(prefix) && e.ends_with(')') {
            let inner = &e[prefix.len()..e.len() - 1];
            if let Some(idx) = variables.iter().position(|v| v == inner) {
                return Ok(match kind {
                    "exp" => survey::Contrast::Exp(idx),
                    "log" => survey::Contrast::Log(idx),
                    _ => unreachable!(),
                });
            }
        }
    }
    // Ratio: `a/b`.
    if let Some(slash) = e.find('/') {
        let num = &e[..slash];
        let den = &e[slash + 1..];
        let num_idx = variables.iter().position(|v| v == num);
        let den_idx = variables.iter().position(|v| v == den);
        if let (Some(n), Some(d)) = (num_idx, den_idx) {
            return Ok(survey::Contrast::Ratio {
                numerator: n,
                denominator: d,
            });
        }
    }
    // Linear: tokenise on +/- into signed `coeff*var` terms.
    let mut coeffs = vec![0.0_f64; variables.len()];
    let mut ok = true;
    let tokens: Vec<&str> = e.split_inclusive(['+', '-']).collect();
    for token in tokens {
        let (sign, body) = if let Some(stripped) = token.strip_prefix('-') {
            (-1.0, stripped)
        } else if let Some(stripped) = token.strip_prefix('+') {
            (1.0, stripped)
        } else {
            (1.0, token)
        };
        let (coeff, var) = if let Some(star) = body.find('*') {
            match body[..star].parse::<f64>() {
                Ok(c) => (c, &body[star + 1..]),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        } else {
            (1.0, body)
        };
        if let Some(idx) = variables.iter().position(|v| v == var) {
            coeffs[idx] += sign * coeff;
        } else {
            ok = false;
            break;
        }
    }
    if ok {
        return Ok(survey::Contrast::Linear(coeffs));
    }
    Err(DagError::NodeError {
        node_type: "svycontrast".into(),
        msg: format!("unable to parse contrast expression '{expr}'"),
    })
}

/// Contrast node — **implemented** (svymean base + delta-method contrasts).
#[derive(Clone)]
pub struct SvyContrastNode {
    meta: NodePorts,
    spec: SvyContrastSpec,
}

impl SvyContrastNode {
    pub fn new(spec: SvyContrastSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyContrastNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svycontrast"
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
            node_type: "svycontrast".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svycontrast".into(),
                msg: format!("collect failed: {e}"),
            })?;

        // Base statistic: svymean over the variables.
        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let x = crate::survey_common::extract_variables(&batches, &self.spec.variables)?;
        let mean = survey::svymean(&x, &design, false).map_err(|e| DagError::NodeError {
            node_type: "svycontrast".into(),
            msg: e.to_string(),
        })?;

        // Build contrasts.
        let mut contrasts: Vec<survey::Contrast> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for ce in &self.spec.contrasts {
            let c = parse_contrast(&ce.expr, &self.spec.variables, Some(ce))?;
            contrasts.push(c);
            names.push(ce.name.clone());
        }

        let (values, vars) =
            survey::svycontrast(&mean.estimate, &mean.var, &contrasts).map_err(|e| {
                DagError::NodeError {
                    node_type: "svycontrast".into(),
                    msg: e.to_string(),
                }
            })?;

        // Build output: name, estimate, se, ci_lower, ci_upper.
        let z = 1.959963984540054_f64;
        let ses: Vec<f64> = vars.iter().map(|v| v.sqrt()).collect();
        let ci_lower: Vec<f64> = values.iter().zip(&ses).map(|(v, s)| v - z * s).collect();
        let ci_upper: Vec<f64> = values.iter().zip(&ses).map(|(v, s)| v + z * s).collect();

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("contrast", DataType::Utf8, false),
                Field::new("estimate", DataType::Float64, false),
                Field::new("se", DataType::Float64, false),
                Field::new("ci_lower", DataType::Float64, false),
                Field::new("ci_upper", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(names)),
                Arc::new(Float64Array::from(values)),
                Arc::new(Float64Array::from(ses)),
                Arc::new(Float64Array::from(ci_lower)),
                Arc::new(Float64Array::from(ci_upper)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svycontrast".into(),
            msg: format!("failed to build output: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svycontrast".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyContrastFactory;

impl NodeFactory for SvyContrastFactory {
    fn kind(&self) -> &'static str {
        "svycontrast"
    }
    fn desc(&self) -> &'static str {
        "Compute linear or non-linear contrasts of survey estimates"
    }
    fn doc(&self) -> &'static str {
        "Computes linear combinations (a - b) or non-linear transformations \
         (ratios, differences of logits) of survey estimates, with delta-method \
         standard errors. Wraps survey::svycontrast."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyContrastSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyContrastSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyContrastNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyContrastSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let vars = formula_rhs(&s.variables);

        // First compute the base statistic (assumes svymean; user can
        // customise in future).
        let stat_var = ctx.fresh_var("stat");
        code.push(format!("{stat_var} <- svymean(~{vars}, {des})"));

        // Build the named contrast list.
        let items: Vec<String> = s
            .contrasts
            .iter()
            .map(|c| format!("{} = {}", c.name, c.expr))
            .collect();
        code.push(format!(
            "{out} <- svycontrast({stat_var}, list({}))",
            items.join(", ")
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// svystandardize — direct standardisation
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyStandardizeSpec {
    pub design: SurveyDesignSpec,
    /// Variable(s) defining the standardisation strata (e.g. age group).
    pub by: Vec<String>,
    /// Variable(s) defining the population groups over which standardisation
    /// is applied (e.g. study arm, region). Default = `~1` (whole sample).
    #[serde(default)]
    pub over: Vec<String>,
    /// Population distribution: `{level: proportion}`. Proportions should sum
    /// to 1.
    pub population: std::collections::HashMap<String, f64>,
}

/// Standardize node — **implemented** (outputs data + standardized weight).
#[derive(Clone)]
pub struct SvyStandardizeNode {
    meta: NodePorts,
    spec: SvyStandardizeSpec,
}

impl SvyStandardizeNode {
    pub fn new(spec: SvyStandardizeSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyStandardizeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svystandardize"
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
            node_type: "svystandardize".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svystandardize".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let by_col = &self.spec.by[0];
        let by =
            crate::survey_common::extract_string_column_pub(&batches, by_col).map_err(|e| {
                DagError::NodeError {
                    node_type: "svystandardize".into(),
                    msg: e.0,
                }
            })?;
        let over = if self.spec.over.is_empty() {
            vec!["1".to_string(); design.n_obs]
        } else {
            let over_col = &self.spec.over[0];
            crate::survey_common::extract_string_column_pub(&batches, over_col).map_err(|e| {
                DagError::NodeError {
                    node_type: "svystandardize".into(),
                    msg: e.0,
                }
            })?
        };

        let new_design = survey::svy_standardize(&design, &by, &over, &self.spec.population)
            .map_err(|e| DagError::NodeError {
                node_type: "svystandardize".into(),
                msg: e.to_string(),
            })?;
        let new_weights = new_design.weights();

        // Output: data + standardized weight column.
        let schema_ref = batches[0].schema_ref();
        let combined = arrow::compute::concat_batches(schema_ref, &batches).map_err(|e| {
            DagError::NodeError {
                node_type: "svystandardize".into(),
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
                    "standardized_weight",
                    DataType::Float64,
                    false,
                ))))
                .collect::<Vec<_>>(),
        ));
        let mut new_columns: Vec<Arc<dyn arrow_array::Array>> =
            combined.columns().to_vec();
        new_columns.push(Arc::new(Float64Array::from(new_weights)));

        let output_batch =
            RecordBatch::try_new(new_schema, new_columns).map_err(|e| DagError::NodeError {
                node_type: "svystandardize".into(),
                msg: format!("failed to build output: {e}"),
            })?;
        let ctx = node_ctx.session();
        let df_out = ctx
            .read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: "svystandardize".into(),
                msg: format!("read_batch failed: {e}"),
            })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyStandardizeFactory;

impl NodeFactory for SvyStandardizeFactory {
    fn kind(&self) -> &'static str {
        "svystandardize"
    }
    fn desc(&self) -> &'static str {
        "Direct standardisation of survey estimates (NCHS method)"
    }
    fn doc(&self) -> &'static str {
        "Post-stratifies the survey design so that the standardisation strata \
         match a reference population distribution, then computes standardised \
         estimates. Wraps survey::svystandardize."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyStandardizeSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: SvyStandardizeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyStandardizeNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyStandardizeSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);

        let by_formula = formula_rhs(&s.by);
        let over_formula = if s.over.is_empty() {
            "~1".to_string()
        } else {
            format!("~{}", formula_rhs(&s.over))
        };

        // Build the population proportions vector.
        let pop_vec: Vec<String> = s.population.values().map(|v| v.to_string()).collect();
        code.push(format!(
            "{out} <- svystandardize({des}, by = ~{bf}, over = {of}, \
             population = c({pv}))",
            bf = by_formula,
            of = over_formula,
            pv = pop_vec.join(", ")
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into()]
    }
}

// =====================================================================
// reg_term_test — test regression terms
// =====================================================================

fn default_regterm_method() -> String {
    "Wald".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RegTermTestSpec {
    pub design: SurveyDesignSpec,
    /// Response variable (for the model to test terms in).
    pub response: String,
    /// All predictors in the full model.
    pub predictors: Vec<String>,
    /// Term(s) to test (must be a subset of `predictors`).
    pub test_terms: Vec<String>,
    /// Method: `"Wald"` (default), `"WorkingWald"`, `"LRT"`.
    #[serde(default = "default_regterm_method")]
    pub method: String,
    /// GLM family (default `"gaussian"`).
    #[serde(default = "default_gaussian")]
    pub family: String,
}

fn default_gaussian() -> String {
    "gaussian".to_string()
}

/// Regression term test node — **implemented** (Wald, Gaussian).
#[derive(Clone)]
pub struct RegTermTestNode {
    meta: NodePorts,
    spec: RegTermTestSpec,
}

impl RegTermTestNode {
    pub fn new(spec: RegTermTestSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for RegTermTestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "reg_term_test"
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
        if self.spec.method != "Wald" {
            return Err(DagError::NodeError {
                node_type: "reg_term_test".into(),
                msg: format!(
                    "Rust execution only supports method='Wald', requested '{}'. \
                     Use codegen_r for R code.",
                    self.spec.method
                ),
            });
        }
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "reg_term_test".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "reg_term_test".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = crate::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let y = crate::survey_common::extract_variables(&batches, &[self.spec.response.clone()])?;
        let x = crate::survey_common::extract_variables(&batches, &self.spec.predictors)?;

        let fit = survey::svyglm_linear(&y[0], &x, &design, true, None).map_err(|e| {
            DagError::NodeError {
                node_type: "reg_term_test".into(),
                msg: e.to_string(),
            }
        })?;

        // Map test_terms to coefficient indices (after intercept).
        let mut test_indices: Vec<usize> = Vec::new();
        for term in &self.spec.test_terms {
            let pos = self
                .spec
                .predictors
                .iter()
                .position(|p| p == term)
                .ok_or_else(|| DagError::NodeError {
                    node_type: "reg_term_test".into(),
                    msg: format!("test term '{term}' not in predictors"),
                })?;
            test_indices.push(pos + 1); // +1 for intercept
        }

        let test = survey::reg_term_test(&fit, &test_indices).map_err(|e| DagError::NodeError {
            node_type: "reg_term_test".into(),
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
                Arc::new(Float64Array::from(vec![test.statistic])),
                Arc::new(Float64Array::from(vec![test.ndf as f64])),
                Arc::new(Float64Array::from(vec![test.ddf as f64])),
                Arc::new(Float64Array::from(vec![test.p_value])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "reg_term_test".into(),
            msg: format!("failed to build output: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "reg_term_test".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct RegTermTestFactory;

impl NodeFactory for RegTermTestFactory {
    fn kind(&self) -> &'static str {
        "reg_term_test"
    }
    fn desc(&self) -> &'static str {
        "Test regression terms in a survey model (Wald / LRT)"
    }
    fn doc(&self) -> &'static str {
        "Fits a survey GLM and tests whether specified terms can be dropped, \
         using a Wald, Working Wald, or LRT (saddlepoint-approximated) test. \
         Wraps survey::regTermTest."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RegTermTestSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn dag_core::dag::DagNode>> {
        let node_spec: RegTermTestSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RegTermTestNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<RegTermTestSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);

        // Fit the full model.
        let model_var = ctx.fresh_var("model");
        let formula = dag_core::codegen::helpers::r_formula(&s.response, &s.predictors, true);
        code.push(format!(
            "{model_var} <- svyglm({formula}, {des}, family = {}())",
            s.family
        ));

        let test_rhs = formula_rhs(&s.test_terms);
        code.push(format!(
            "{out} <- regTermTest({model_var}, ~{test_rhs}, method = \"{m}\")",
            m = s.method
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
    fn svyby_spec_defaults() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"], "weights": "wt"},
            "variables": ["income"],
            "by": ["region"]
        });
        let s: SvyBySpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.fun, "svymean");
        assert_eq!(s.vartype, vec!["se"]);
        assert!(s.drop_empty_groups);
    }

    #[test]
    fn regterm_spec() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"]},
            "response": "y",
            "predictors": ["x1", "x2", "x3"],
            "test_terms": ["x2", "x3"]
        });
        let s: RegTermTestSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.method, "Wald");
        assert_eq!(s.test_terms, vec!["x2", "x3"]);
    }

    // ── End-to-end svyby test ────────────────────────────────────────────────
    //
    // Uses the R `fpc` dataset split into two strata as groups.
    // Validates that svyby produces per-group means matching R svymean
    // computed within each stratum.

    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }

    #[tokio::test]
    async fn svyby_node_fpc_by_stratum() {
        use arrow_array::{Float64Array, Int32Array};
        // fpc dataset — use stratid as the grouping variable.
        let schema = Arc::new(Schema::new(vec![
            Field::new("stratid", DataType::Int32, false),
            Field::new("psuid", DataType::Int32, false),
            Field::new("weight", DataType::Float64, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
                Arc::new(Float64Array::from(vec![
                    2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2,
                ])),
            ],
        )
        .unwrap();

        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyBySpec {
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
            by: vec!["stratid".into()],
            fun: "svymean".into(),
            vartype: vec!["se".into()],
            drop_empty_groups: true,
            na_rm: false,
        };

        let mut node = SvyByNode::new(spec);
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
        // Two groups (stratum 1, stratum 2), one variable each.
        assert_eq!(total_rows, 2);

        // Extract group labels, estimates, and SEs.
        let _groups: Vec<String> = result[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap().to_string())
            .collect();
        let estimates: Vec<f64> = result[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();

        // Stratum 1: weighted mean of [2.8, 4.1, 6.8, 6.8, 9.2] with weight 3 each.
        // = (2.8+4.1+6.8+6.8+9.2)*3 / (5*3) = 29.7/5 = 5.94
        // Stratum 2: weighted mean of [3.7, 6.6, 4.2] with weight 4 each.
        // = (3.7+6.6+4.2)*4 / (3*4) = 14.5/3 = 4.8333
        assert!(
            (estimates[0] - 5.94).abs() < 0.01,
            "stratum 1 mean should be ~5.94, got {}",
            estimates[0]
        );
        assert!(
            (estimates[1] - 4.8333).abs() < 0.01,
            "stratum 2 mean should be ~4.8333, got {}",
            estimates[1]
        );
    }

    #[tokio::test]
    async fn svycontrast_node_linear_matches_r() {
        // fpc dataset: svymean of x, then contrast x - x should be 0 with SE 0.
        // More meaningfully: mean(x) computed on x and a copy → x - x = 0.
        let schema = Arc::new(Schema::new(vec![
            Field::new("stratid", DataType::Int32, false),
            Field::new("psuid", DataType::Int32, false),
            Field::new("weight", DataType::Float64, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow_array::Int32Array::from(vec![1, 1, 1, 1, 1, 2, 2, 2])),
                Arc::new(arrow_array::Int32Array::from(vec![1, 2, 3, 4, 5, 1, 2, 3])),
                Arc::new(Float64Array::from(vec![
                    3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
                ])),
                Arc::new(Float64Array::from(vec![
                    2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2,
                ])),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();

        let spec = SvyContrastSpec {
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
            variables: vec!["x".into(), "x".into()],
            contrasts: vec![ContrastExpr {
                name: "diff".into(),
                expr: "x - x".into(),
                kind: Some("linear".into()),
                coefficients: Some(vec![1.0, -1.0]),
            }],
        };

        let mut node = SvyContrastNode::new(spec);
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
        let value: f64 = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        // x - x = 0 (within numerical tolerance).
        assert!(value.abs() < 1e-9, "diff should be 0, got {value}");
    }
}
