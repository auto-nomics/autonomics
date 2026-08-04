//! Chi-squared independence test node.
//!
//! Wraps [`epi::chisq::chi_squared_test`]. Cross-tabulates two categorical
//! columns into a contingency table, then tests for independence.
//!
//! Output schema (single row):
//!
//! | Column         | Type    | Description                              |
//! |----------------|---------|------------------------------------------|
//! | `chi_squared`  | Float64 | Test statistic                            |
//! | `df`           | Int32   | Degrees of freedom `(r−1)(c−1)`           |
//! | `p_value`      | Float64 | p-value from χ² distribution              |
//! | `n`            | Int32   | Total observations                         |
//! | `small_expected` | Int32 | Number of cells with expected count < 5  |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use super::numeric_util::{ColumnError, crosstab, extract_string_column};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum ChiSquareError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Test(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for ChiSquareError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl From<ChiSquareError> for DagError {
    fn from(e: ChiSquareError) -> Self {
        DagError::NodeError {
            node_type: "chi_square".to_string(),
            msg: e.to_string(),
        }
    }
}

/// Spec for [`ChiSquareNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChiSquareNodeSpec {
    /// Name of the column defining contingency-table rows (e.g. "pd_status").
    pub row_column: String,
    /// Name of the column defining contingency-table columns (e.g. "sleep_duration").
    pub col_column: String,
}

#[derive(Clone)]
pub struct ChiSquareNode {
    meta: NodePorts,
    row_column: String,
    col_column: String,
}

pub struct ChiSquareNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for ChiSquareNodeFactory {
    fn kind(&self) -> &'static str {
        "chi_square"
    }
    fn desc(&self) -> &'static str {
        "Pearson χ² independence test on two categorical columns."
    }
    fn doc(&self) -> &'static str {
        "Cross-tabulates two string/categorical columns into a contingency \
        table and performs the Pearson chi-squared independence test. Both \
        columns must be string-typed (Utf8/LargeUtf8/Utf8View). Outputs the \
        χ² statistic, degrees of freedom, p-value, sample size, and the \
        count of cells with expected count < 5 (a small-sample warning)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ChiSquareNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: ChiSquareNodeSpec = serde_json::from_value(spec)?;
        if s.row_column == s.col_column {
            return Err(crate::node_registry::error::Error::SpecRejection {
                kind: "chi_square".to_string(),
                reason: "row_column and col_column must be different".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(ChiSquareNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(ChiSquareNode {
            meta: port_layout(),
            row_column: s.row_column,
            col_column: s.col_column,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let s = parse_spec::<ChiSquareNodeSpec>(spec, "chi_square")?;
        let out = ctx.output_var.to_string();
        let test_var = ctx.fresh_var("chisq");
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# Chi-square test"),
            format!(
                "{test_var} <- chisq.test(table({input}${}, {input}${}))",
                s.row_column, s.col_column
            ),
            format!("{out} <- data.frame("),
            format!("  chi_squared = as.numeric({test_var}$statistic),"),
            format!("  df = as.integer({test_var}$parameter),"),
            format!("  p_value = {test_var}$p.value,"),
            format!("  n = sum({test_var}$observed),"),
            format!("  small_expected = sum({test_var}$expected < 5)"),
            format!(")"),
            format!("print({out})"),
        ];
        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }
}

#[async_trait]
impl DagNode for ChiSquareNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "chi_square"
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
            .ok_or(ChiSquareError::Column("no input connected".to_string()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| ChiSquareError::Collect(e.to_string()))?;

        let row_vals = extract_string_column(&batches, &self.row_column)?;
        let col_vals = extract_string_column(&batches, &self.col_column)?;

        if row_vals.len() != col_vals.len() {
            return Err(ChiSquareError::Column(
                "row_column and col_column have different lengths".to_string(),
            )
            .into());
        }

        let (counts, _row_labels, _col_labels) = crosstab(&row_vals, &col_vals);

        let result = epi::chisq::chi_squared_test(&counts)
            .map_err(|e| ChiSquareError::Test(e.to_string()))?;

        let n_total: u64 = counts.iter().flat_map(|r| r.iter()).sum();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("chi_squared", DataType::Float64, false),
                Field::new("df", DataType::Int32, false),
                Field::new("p_value", DataType::Float64, false),
                Field::new("n", DataType::Int32, false),
                Field::new("small_expected", DataType::Int32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.chi_squared])),
                Arc::new(Int32Array::from(vec![result.df as i32])),
                Arc::new(Float64Array::from(vec![result.p_value])),
                Arc::new(Int32Array::from(vec![n_total as i32])),
                Arc::new(Int32Array::from(vec![result.small_expected_count as i32])),
            ],
        )
        .expect("schema mismatch in chi_square output");

        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| ChiSquareError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
