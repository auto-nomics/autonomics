//! Kaplan-Meier estimator + Log-rank test node.
//!
//! Port 0 (always): KM survival curve — one row per distinct event time.
//! Port 1 (optional): Log-rank test — single row, if `group_column` is set.
//!
//! KM output columns: `time`, `survival`, `std_error`, `n_at_risk`, `n_events`.
//! Log-rank output columns: `chi_squared`, `df`, `p_value`, `n_groups`,
//! `observed_g0`, `expected_g0`, `observed_g1`, `expected_g1`.

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use super::numeric_util::{ColumnError, extract_numeric_lenient};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum SurvivalError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for SurvivalError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl From<SurvivalError> for DagError {
    fn from(e: SurvivalError) -> Self {
        DagError::NodeError {
            node_type: "survival".to_string(),
            msg: e.to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SurvivalNodeSpec {
    /// Survival time column name.
    pub time_column: String,
    /// Event indicator column (1 = event, 0 = censored).
    pub event_column: String,
    /// Optional group column for log-rank test. When provided, port 1 carries
    /// the log-rank result. Values must be non-negative integers (0, 1, …, K−1).
    #[serde(default)]
    pub group_column: Option<String>,
}

#[derive(Clone)]
pub struct SurvivalNode {
    meta: NodePorts,
    time_column: String,
    event_column: String,
    group_column: Option<String>,
}

pub struct SurvivalNodeFactory {}

fn port_layout(has_group: bool) -> NodePorts {
    let ports = NodePorts::new().add_output_port(None).add_input_port(None);
    if has_group {
        ports.add_output_port(None)
    } else {
        ports
    }
}

impl NodeFactory for SurvivalNodeFactory {
    fn kind(&self) -> &'static str {
        "survival"
    }
    fn desc(&self) -> &'static str {
        "Kaplan-Meier survival curve with optional log-rank test."
    }
    fn doc(&self) -> &'static str {
        "Computes the Kaplan-Meier survival estimator (port 0: curve with \
        Greenwood SE). When a group_column is specified, also performs a \
        Mantel-Cox log-rank test comparing survival across groups (port 1)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SurvivalNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout(false)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: SurvivalNodeSpec = serde_json::from_value(spec)?;
        let has_group = s.group_column.is_some();
        Ok(Box::new(SurvivalNode {
            meta: port_layout(has_group),
            time_column: s.time_column,
            event_column: s.event_column,
            group_column: s.group_column,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let s = parse_spec::<SurvivalNodeSpec>(spec, "survival")?;
        let out = ctx.output_var.to_string();
        let formula = match &s.group_column {
            Some(g) => format!("Surv({}, {}) ~ {}", s.time_column, s.event_column, g),
            None => format!("Surv({}, {}) ~ 1", s.time_column, s.event_column),
        };
        let fit_var = ctx.fresh_var("surv_fit");
        let smry_var = ctx.fresh_var("surv_smry");
        let input = input_0(ctx).to_string();
        let has_group = s.group_column.is_some();
        let mut code = vec![
            format!("# Kaplan-Meier survival analysis"),
            format!("{fit_var} <- survfit(as.formula(\"{formula}\"), data = {input})"),
            format!("{smry_var} <- summary({fit_var})"),
            format!("{out} <- data.frame("),
            format!("  time = {smry_var}$time,"),
            format!("  survival = {smry_var}$surv,"),
            format!("  std_error = {smry_var}$std.err,"),
            format!("  n_at_risk = {smry_var}$n.risk,"),
            format!("  n_events = {smry_var}$n.event"),
            format!(")"),
        ];
        if has_group {
            code.push(format!(
                "# NOTE: group-stratified KM; log-rank test omitted in codegen"
            ));
        }
        code.push(format!("print(head({out}))"));
        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["survival".into()]
    }
}

#[async_trait]
impl DagNode for SurvivalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "survival"
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
        let input = inputs
            .first()
            .ok_or(SurvivalError::Column("no input connected".to_string()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| SurvivalError::Collect(e.to_string()))?;

        let time_raw = extract_numeric_lenient(&batches, &self.time_column)?;
        let event_raw = extract_numeric_lenient(&batches, &self.event_column)?;
        for &v in &event_raw {
            if !v.is_nan() && v != 0.0 && v != 1.0 {
                return Err(SurvivalError::Column(format!(
                    "event '{}' must be 0/1, found {v}",
                    self.event_column
                ))
                .into());
            }
        }

        let group_raw = match &self.group_column {
            Some(col) => Some(extract_numeric_lenient(&batches, col)?),
            None => None,
        };

        // Complete-case filter.
        let n = time_raw.len();
        let mut time = Vec::with_capacity(n);
        let mut event = Vec::with_capacity(n);
        let mut group = Vec::with_capacity(n);
        let has_group = group_raw.is_some();
        let g_data = group_raw.unwrap_or_default();
        for i in 0..n {
            if time_raw[i].is_nan() || event_raw[i].is_nan() {
                continue;
            }
            if has_group && g_data[i].is_nan() {
                continue;
            }
            time.push(time_raw[i]);
            event.push(event_raw[i]);
            if has_group {
                group.push(g_data[i] as u64);
            }
        }

        if time.is_empty() {
            return Err(SurvivalError::Column("no complete-case rows".to_string()).into());
        }

        // ── Kaplan-Meier ────────────────────────────────────────────────
        let km = epi::survival::kaplan_meier(&time, &event)
            .map_err(|e| SurvivalError::Computation(e.to_string()))?;

        let km_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("time", DataType::Float64, false),
                Field::new("survival", DataType::Float64, false),
                Field::new("std_error", DataType::Float64, false),
                Field::new("n_at_risk", DataType::Int32, false),
                Field::new("n_events", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(km.times)),
                Arc::new(Float64Array::from(km.survival)),
                Arc::new(Float64Array::from(km.std_error)),
                Arc::new(Int32Array::from(
                    km.n_at_risk.iter().map(|&v| v as i32).collect::<Vec<_>>(),
                )),
                Arc::new(Int32Array::from(
                    km.n_events.iter().map(|&v| v as i32).collect::<Vec<_>>(),
                )),
            ],
        )
        .expect("km schema");

        let ctx = node_ctx.session();
        let df0 = ctx
            .read_batch(km_batch)
            .map_err(|e| SurvivalError::ReadBatch(e.to_string()))?;

        let mut res = PortOutputs::new();
        res.insert(0, df0);

        // ── Log-rank test (optional) ────────────────────────────────────
        if has_group {
            let lr = epi::survival::log_rank_test(&time, &event, &group)
                .map_err(|e| SurvivalError::Computation(e.to_string()))?;

            let lr_batch = RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("chi_squared", DataType::Float64, false),
                    Field::new("df", DataType::Int32, false),
                    Field::new("p_value", DataType::Float64, false),
                    Field::new("n_groups", DataType::Int32, false),
                    Field::new("n_obs", DataType::Int32, false),
                    Field::new("n_events", DataType::Int32, false),
                ])),
                vec![
                    Arc::new(Float64Array::from(vec![lr.chi_squared])),
                    Arc::new(Int32Array::from(vec![lr.df as i32])),
                    Arc::new(Float64Array::from(vec![lr.p_value])),
                    Arc::new(Int32Array::from(vec![lr.n_groups as i32])),
                    Arc::new(Int32Array::from(vec![time.len() as i32])),
                    Arc::new(Int32Array::from(vec![
                        lr.observed.iter().sum::<f64>() as i32
                    ])),
                ],
            )
            .expect("log-rank schema");

            let df1 = ctx
                .read_batch(lr_batch)
                .map_err(|e| SurvivalError::ReadBatch(e.to_string()))?;
            res.insert(1, df1);
        }

        Ok(res)
    }
}
