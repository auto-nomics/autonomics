//! Survey-weighted survival analysis nodes.
//!
//! Nodes: `svykm` (Kaplan-Meier curve), `svylogrank` (logrank test).
//!
//! R package reference: `survey::svykm`, `svylogrank`.

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use async_trait::async_trait;

use arrow_array::Float64Array;

use super::meta::{DagNode, NodeInput, NodePorts};
use super::survey_common::{SurveyDesignSpec, gen_design_r, one_in_one_out};
use crate::codegen::helpers::{input_0, parse_spec};
use crate::codegen::{CodegenCtx, CodegenError, NodeCodegen};
use crate::dag::{DagError, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};

// =====================================================================
// svykm
// =====================================================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyKmSpec {
    pub design: SurveyDesignSpec,
    /// Time-to-event column name.
    pub time_column: String,
    /// Event indicator column name (1 = event, 0 = censored).
    pub event_column: String,
    /// Optional grouping variable for stratified curves.
    #[serde(default)]
    pub group: Option<String>,
    /// If `true`, compute standard errors for the curve.
    #[serde(default)]
    pub se: bool,
}

/// Kaplan-Meier node — **implemented** (no-SE product-limit estimator).
#[derive(Clone)]
pub struct SvyKmNode {
    meta: NodePorts,
    spec: SvyKmSpec,
}

impl SvyKmNode {
    pub fn new(spec: SvyKmSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyKmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "svykm"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svykm".into(),
            msg: "no input data".into(),
        })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "svykm".into(),
                msg: format!("collect failed: {e}"),
            })?;

        let design = super::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let t = super::survey_common::extract_variables(&batches, &[self.spec.time_column.clone()])?;
        let e =
            super::survey_common::extract_variables(&batches, &[self.spec.event_column.clone()])?;

        let km = survey::svy_km(&t[0], &e[0], &design).map_err(|err| DagError::NodeError {
            node_type: "svykm".into(),
            msg: err.to_string(),
        })?;

        use arrow_array::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![
            Field::new("time", DataType::Float64, false),
            Field::new("survival", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(km.time)),
                Arc::new(Float64Array::from(km.survival)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "svykm".into(),
            msg: format!("failed to build output: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svykm".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyKmFactory;

impl NodeFactory for SvyKmFactory {
    fn kind(&self) -> &'static str {
        "svykm"
    }
    fn desc(&self) -> &'static str {
        "Survey-weighted Kaplan-Meier survival curve"
    }
    fn doc(&self) -> &'static str {
        "Estimates a survey-weighted Kaplan-Meier survival curve, optionally \
         stratified by a group variable, with linearised standard errors. \
         Wraps survey::svykm."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyKmSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        let node_spec: SvyKmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvyKmNode::new(node_spec)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyKmSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rhs = match &s.group {
            Some(g) => format!("Surv({}, {}) ~ {}", s.time_column, s.event_column, g),
            None => format!("Surv({}, {}) ~ 1", s.time_column, s.event_column),
        };
        let se_arg = if s.se { ", se = TRUE" } else { "" };
        code.push(format!("{out} <- svykm({rhs}, {des}{se_arg})"));
        code.push(format!("plot({out})"));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into(), "survival".into()]
    }
}

// =====================================================================
// svylogrank
// =====================================================================

fn default_logrank_method() -> String {
    "small".to_string()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvyLogrankSpec {
    pub design: SurveyDesignSpec,
    pub time_column: String,
    pub event_column: String,
    /// Grouping variable (must be specified for a logrank test).
    pub group: String,
    /// Method: `"small"` (default), `"large"`, `"score"`.
    #[serde(default = "default_logrank_method")]
    pub method: String,
    /// Fleming-Harrington ρ parameter (weighting).
    #[serde(default)]
    pub rho: Option<f64>,
}

/// Logrank node — **implemented** (score test from Cox at β=0).
#[derive(Clone)]
pub struct SvyLogrankNode {
    meta: NodePorts,
    spec: SvyLogrankSpec,
}

impl SvyLogrankNode {
    pub fn new(spec: SvyLogrankSpec) -> Self {
        Self {
            meta: one_in_one_out(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for SvyLogrankNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "svylogrank" }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| DagError::NodeError {
            node_type: "svylogrank".into(), msg: "no input data".into(),
        })?;
        let batches = input.data.clone().collect().await.map_err(|e| DagError::NodeError {
            node_type: "svylogrank".into(), msg: format!("collect failed: {e}"),
        })?;
        let design = super::survey_common::build_survey_design(&self.spec.design, &batches)?;
        let t = super::survey_common::extract_variables(&batches, &[self.spec.time_column.clone()])?;
        let e = super::survey_common::extract_variables(&batches, &[self.spec.event_column.clone()])?;
        let g_str = super::survey_common::extract_string_column_pub(&batches, &self.spec.group)
            .map_err(|e| DagError::NodeError { node_type: "svylogrank".into(), msg: e.0 })?;
        // Map group to 0/1.
        let g: Vec<f64> = g_str.iter().enumerate().map(|(i, s)| {
            // Use first two unique levels as 0 and 1.
            if i == 0 { 0.0 } else if s == &g_str[0] { 0.0 } else { 1.0 }
        }).collect();
        let r = survey::svy_logrank(&t[0], &e[0], &g, &design).map_err(|e| DagError::NodeError {
            node_type: "svylogrank".into(), msg: e.to_string(),
        })?;
        use arrow_array::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![
            Field::new("chisq", DataType::Float64, false),
            Field::new("df", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(schema, vec![
            Arc::new(Float64Array::from(vec![r.chisq])),
            Arc::new(Float64Array::from(vec![r.df as f64])),
            Arc::new(Float64Array::from(vec![r.p_value])),
        ]).map_err(|e| DagError::NodeError { node_type: "svylogrank".into(), msg: format!("output: {e}") })?;
        let ctx = node_ctx.session();
        let df_out = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "svylogrank".into(), msg: format!("read: {e}"),
        })?;
        let mut res = PortOutputs::new();
        res.insert(0, df_out);
        Ok(res)
    }
}

pub struct SvyLogrankFactory;

impl NodeFactory for SvyLogrankFactory {
    fn kind(&self) -> &'static str {
        "svylogrank"
    }
    fn desc(&self) -> &'static str {
        "Design-based logrank test for survival curves"
    }
    fn doc(&self) -> &'static str {
        "Performs a design-based logrank test comparing survival curves across \
         groups. Wraps survey::svylogrank."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvyLogrankSpec)
    }
    fn ports(&self) -> NodePorts {
        one_in_one_out()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn crate::dag::DagNode>> {
        { let s: SvyLogrankSpec = serde_json::from_value(spec)?; Ok(Box::new(SvyLogrankNode::new(s))) }
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<SvyLogrankSpec>(spec, "survey_node")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let (des, mut code) = gen_design_r(&s.design, &input, ctx);
        let rho_arg = match s.rho {
            Some(r) => format!(", rho = {}", r),
            None => String::new(),
        };
        code.push(format!(
            "{out} <- svylogrank(Surv({t}, {e}) ~ {g}, {des}, method = \"{m}\"{rho})",
            t = s.time_column,
            e = s.event_column,
            g = s.group,
            m = s.method,
            rho = rho_arg
        ));
        code.push(format!("print({out})"));
        Ok(NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["survey".into(), "survival".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svykm_spec_grouped() {
        let json = serde_json::json!({
            "design": {"ids": ["psu"]},
            "time_column": "time",
            "event_column": "event",
            "group": "treatment",
            "se": true
        });
        let s: SvyKmSpec = serde_json::from_value(json).unwrap();
        assert_eq!(s.group.as_deref(), Some("treatment"));
        assert!(s.se);
    }
}
