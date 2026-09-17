//! Cumulative incidence function node (Gray 1988).
//!
//! Wraps [`cmprsk::cuminc`] — the Rust port of R `cmprsk::cuminc()`. Estimates
//! the cumulative incidence of each competing event nonparametrically, with
//! Aalen-type variances, and (when a grouping column is supplied) Gray's
//! stratified k-sample test comparing those curves between groups.
//!
//! This is the descriptive counterpart to [`fine_gray`](super::fine_gray):
//! `cuminc` shows *what* the incidence curves look like, `fine_gray` models
//! covariate effects on them.
//!
//! **Port 0** — cumulative incidence curves, one row per step-function corner:
//!
//! | Column  | Type    | Description                                       |
//! |---------|---------|---------------------------------------------------|
//! | `group` | Utf8    | Group label (`"1"` when no grouping column)        |
//! | `cause` | Utf8    | Failure-type code                                  |
//! | `time`  | Float64 | Time                                               |
//! | `est`   | Float64 | Estimated cumulative incidence                     |
//! | `var`   | Float64 | Variance of the estimate                           |
//!
//! Both corners of every jump are emitted, so plotting the rows directly as a
//! line gives the correct step function — the same layout R returns.
//!
//! **Port 1** — Gray's k-sample tests, one row per cause (empty when there is
//! a single group):
//!
//! | Column    | Type    | Description                              |
//! |-----------|---------|------------------------------------------|
//! | `cause`   | Utf8    | Failure-type code                         |
//! | `stat`    | Float64 | Test statistic (`-1` if rank-deficient)   |
//! | `p_value` | Float64 | χ² p-value on `df` degrees of freedom     |
//! | `df`      | Int32   | `n_groups - 1`                            |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use cmprsk::CumincOptions;
use cmprsk::cuminc::{CumincInput, CumincResult};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::arrow_util::ColumnError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

/// Node kind string.
pub const CUMINC_NODE_KIND: &str = "cuminc";

#[derive(Debug, Error)]
pub enum CumincError {
    #[error("{0}")]
    Column(String),
    #[error("cuminc failed: {0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for CumincError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for CumincError {
    fn node_type(&self) -> &str {
        CUMINC_NODE_KIND
    }
}

// ── spec ────────────────────────────────────────────────────────────────────

fn default_cencode() -> f64 {
    0.0
}

/// Configuration for the `cuminc` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CumincNodeSpec {
    /// Failure / censoring time column.
    pub time_column: String,
    /// Failure-type code column; `cencode` marks censoring, every other
    /// distinct value is treated as a competing cause.
    pub status_column: String,
    /// Grouping column whose curves are compared. Without it a single curve
    /// per cause is produced and no test is run.
    #[serde(default)]
    pub group_column: Option<String>,
    /// Stratification column for Gray's test (the test is stratified, the
    /// curves are not).
    #[serde(default)]
    pub strata_column: Option<String>,
    /// Power `ρ` of the weight function `(1 - F(t-))^ρ` in Gray's test.
    /// `0` (the default) weights all times equally.
    #[serde(default)]
    pub rho: f64,
    /// Code of `status_column` denoting a censored observation.
    #[serde(default = "default_cencode")]
    pub cencode: f64,
}

#[derive(Clone)]
pub struct CumincNode {
    meta: NodePorts,
    spec: CumincNodeSpec,
}

pub struct CumincNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None) // 0: curves
        .add_output_port(None) // 1: Gray tests
        .add_input_port(None)
}

impl NodeFactory for CumincNodeFactory {
    fn kind(&self) -> &'static str {
        CUMINC_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Cumulative incidence functions for competing risks, with Gray's k-sample test."
    }
    fn doc(&self) -> &'static str {
        "Estimates the cumulative incidence function (CIF) for each competing \
        event, the correct nonparametric analogue of 1 - Kaplan-Meier when more \
        than one event type is possible. Naive 1-KM per cause overstates \
        incidence; this does not. Variances follow Aalen's estimator. When a \
        grouping column is given, Gray's (1988) k-sample test compares the \
        curves across groups for each cause; the test can be stratified and \
        re-weighted through `rho`. Output port 0 holds the curves (both corners \
        of every step, ready to plot); port 1 holds the test results."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CumincNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CumincNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CumincNode {
            meta: port_layout(),
            spec: s,
        }))
    }
}

// ── execution ───────────────────────────────────────────────────────────────

fn curves_schema() -> Schema {
    Schema::new(vec![
        Field::new("group", DataType::Utf8, false),
        Field::new("cause", DataType::Utf8, false),
        Field::new("time", DataType::Float64, false),
        Field::new("est", DataType::Float64, false),
        Field::new("var", DataType::Float64, false),
    ])
}

fn tests_schema() -> Schema {
    Schema::new(vec![
        Field::new("cause", DataType::Utf8, false),
        Field::new("stat", DataType::Float64, false),
        Field::new("p_value", DataType::Float64, false),
        Field::new("df", DataType::Int32, false),
    ])
}

/// Read a column as a factor: numeric columns keep their values (the crate
/// factorises them by sorted-unique, matching R's `as.factor`), string columns
/// are mapped to ordered codes with the strings kept as labels.
fn read_factor(
    batches: &[RecordBatch],
    name: &str,
) -> Result<(Vec<f64>, Vec<String>), ColumnError> {
    let dtype = dag_core::arrow_util::column_dtype(batches, name)?;
    match dtype {
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {
            let raw = dag_core::arrow_util::extract_string_column(batches, name)?;
            let mut levels: Vec<String> = raw.clone();
            levels.sort();
            levels.dedup();
            let codes = raw
                .iter()
                .map(|v| levels.iter().position(|l| l == v).unwrap_or(0) as f64)
                .collect();
            Ok((codes, levels))
        }
        _ => {
            let raw = dag_core::arrow_util::extract_numeric_lenient(batches, name)?;
            let mut levels: Vec<f64> = raw.iter().copied().filter(|v| v.is_finite()).collect();
            levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            levels.dedup();
            let labels = levels
                .iter()
                .map(|v| {
                    if v.fract() == 0.0 && v.abs() < 1e15 {
                        format!("{}", *v as i64)
                    } else {
                        format!("{v}")
                    }
                })
                .collect();
            Ok((raw, labels))
        }
    }
}

/// Build both output batches from a cuminc result.
pub(crate) fn build_batches(res: &CumincResult) -> Result<(RecordBatch, RecordBatch), CumincError> {
    let mut groups = Vec::new();
    let mut causes = Vec::new();
    let mut times = Vec::new();
    let mut ests = Vec::new();
    let mut vars = Vec::new();
    for c in &res.curves {
        for k in 0..c.time.len() {
            groups.push(c.group.clone());
            causes.push(c.cause.clone());
            times.push(c.time[k]);
            ests.push(c.est[k]);
            vars.push(c.var[k]);
        }
    }
    let curves = RecordBatch::try_new(
        Arc::new(curves_schema()),
        vec![
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(causes)),
            Arc::new(Float64Array::from(times)),
            Arc::new(Float64Array::from(ests)),
            Arc::new(Float64Array::from(vars)),
        ],
    )
    .map_err(|e| CumincError::Fit(format!("curves batch: {e}")))?;

    let tests = RecordBatch::try_new(
        Arc::new(tests_schema()),
        vec![
            Arc::new(StringArray::from(
                res.tests
                    .iter()
                    .map(|t| t.cause.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                res.tests.iter().map(|t| t.stat).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                res.tests.iter().map(|t| t.p_value).collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                res.tests.iter().map(|t| t.df as i32).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| CumincError::Fit(format!("tests batch: {e}")))?;

    Ok((curves, tests))
}

#[async_trait]
impl DagNode for CumincNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        CUMINC_NODE_KIND
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
        let input = inputs
            .first()
            .ok_or(CumincError::Column("no input connected".to_string()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| CumincError::Collect(e.to_string()))?;

        let s = &self.spec;
        let ftime = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.time_column)?;
        let fstatus = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.status_column)?;

        let group = match &s.group_column {
            None => None,
            Some(c) => Some(read_factor(&batches, c)?),
        };
        let strata = match &s.strata_column {
            None => None,
            Some(c) => Some(read_factor(&batches, c)?),
        };

        let res = cmprsk::cuminc(
            &CumincInput {
                ftime: &ftime,
                fstatus: &fstatus,
                group: group.as_ref().map(|(v, _)| v.as_slice()),
                group_labels: group.as_ref().map(|(_, l)| l.as_slice()),
                strata: strata.as_ref().map(|(v, _)| v.as_slice()),
            },
            &CumincOptions {
                rho: s.rho,
                cencode: s.cencode,
            },
        )
        .map_err(|e| CumincError::Fit(e.to_string()))?;

        let (curves, tests) = build_batches(&res)?;

        let ctx = node_ctx.session();
        let mut out = PortOutputs::new();
        out.insert(
            0,
            ctx.read_batch(curves)
                .map_err(|e| CumincError::ReadBatch(e.to_string()))?,
        );
        out.insert(
            1,
            ctx.read_batch(tests)
                .map_err(|e| CumincError::ReadBatch(e.to_string()))?,
        );
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults() {
        let s: CumincNodeSpec = serde_json::from_value(serde_json::json!({
            "time_column": "t",
            "status_column": "s"
        }))
        .expect("spec parses");
        assert_eq!(s.rho, 0.0);
        assert_eq!(s.cencode, 0.0);
        assert!(s.group_column.is_none());
        assert!(s.strata_column.is_none());
    }
}
