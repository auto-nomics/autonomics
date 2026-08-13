//! Linear regression transform node.
//!
//! Takes a single upstream `DataFrame`, extracts the specified X and Y columns,
//! fits an OLS model via [`statkit::regression::ols`], and outputs a summary
//! `DataFrame` with coefficients, standard errors, t-statistics, and p-values.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum LinearRegressionError {
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not numeric (got type: {dtype})")]
    NonNumericColumn { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("regression failed: {0}")]
    Regression(String),
}

impl ::dag_core::dag::NodeError for LinearRegressionError {
    fn node_type(&self) -> &str {
        "linear_regression"
    }
}

// =====================================================================
// Column extraction — Arrow array → Vec<f64>
// =====================================================================

/// Extract a named column from `Vec<RecordBatch>` into a `Vec<f64>`,
/// casting numeric types and returning an error for unsupported types.
/// Null values are replaced with `NaN` (the caller or regression engine can
/// filter them if desired).
fn extract_column(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, LinearRegressionError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(LinearRegressionError::EmptyInput)?;
    let idx = schema
        .index_of(name)
        .map_err(|_| LinearRegressionError::MissingColumn {
            name: name.to_string(),
        })?;

    let dtype = schema.field(idx).data_type().clone();
    let is_numeric = matches!(
        dtype,
        DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    );
    if !is_numeric {
        return Err(LinearRegressionError::NonNumericColumn {
            name: name.to_string(),
            dtype: dtype.to_string(),
        });
    }

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        extract_numeric_column(col, &mut values);
    }
    Ok(values)
}

/// Push numeric values from a single column array into `out`, converting
/// nulls to NaN. Dispatches on the array type.
fn extract_numeric_column(col: &dyn Array, out: &mut Vec<f64>) {
    macro_rules! cast {
        ($arr:expr, $T:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(val) => val as f64,
                        None => f64::NAN,
                    });
                }
                return;
            }
        };
    }
    cast!(col, Int8Array);
    cast!(col, Int16Array);
    cast!(col, Int32Array);
    cast!(col, Int64Array);
    cast!(col, UInt8Array);
    cast!(col, UInt16Array);
    cast!(col, UInt32Array);
    cast!(col, UInt64Array);
    cast!(col, Float32Array);
    cast!(col, Float64Array);
    // Float16 and any numeric type not explicitly handled above is intentionally
    // left as NaN — `extract_column` already rejects non-numeric columns before
    // dispatching here, so this only fills remaining slots defensively.
    for i in 0..col.len() {
        if col.is_null(i) {
            out.push(f64::NAN);
        }
    }
}

use statkit::regression;

// =====================================================================
// Output DataFrame construction
// =====================================================================

/// Build a summary `RecordBatch` from the regression result.
///
/// Columns: `term` (Utf8), `coefficient`, `std_error`, `t_stat`, `p_value`,
/// `r_squared`, `n_obs` (all Float64 except term).
fn build_result_batch(reg: &regression::Regression, intercept: bool) -> RecordBatch {
    let n = reg.n_params;
    let mut terms = Vec::with_capacity(n);
    let mut coefficients = Vec::with_capacity(n);
    let mut std_errors = Vec::with_capacity(n);
    let mut t_stats = Vec::with_capacity(n);
    let mut p_values = Vec::with_capacity(n);

    for i in 0..n {
        let term = if intercept && i == 0 {
            "intercept".to_string()
        } else {
            format!("x{}", if intercept { i } else { i + 1 })
        };
        terms.push(term);
        coefficients.push(reg.coefficients[i]);
        std_errors.push(reg.std_errors[i]);
        t_stats.push(reg.t_stats[i]);
        p_values.push(reg.p_values[i]);
    }

    // Broadcast global summary columns.
    let r_squared = vec![reg.r_squared; n];
    let n_obs = vec![reg.n_obs as f64; n];

    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("term", DataType::Utf8, false),
            Field::new("coefficient", DataType::Float64, false),
            Field::new("std_error", DataType::Float64, false),
            Field::new("t_stat", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("r_squared", DataType::Float64, false),
            Field::new("n_obs", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(terms)),
            Arc::new(Float64Array::from(coefficients)),
            Arc::new(Float64Array::from(std_errors)),
            Arc::new(Float64Array::from(t_stats)),
            Arc::new(Float64Array::from(p_values)),
            Arc::new(Float64Array::from(r_squared)),
            Arc::new(Float64Array::from(n_obs)),
        ],
    )
    .expect("failed to build regression result batch")
}

// =====================================================================
// Node
// =====================================================================

/// A transform node that fits an OLS linear regression on the input DataFrame.
#[derive(Clone)]
pub struct LinearRegressionNode {
    meta: NodePorts,
    x_columns: Vec<String>,
    y_column: String,
    intercept: bool,
}

impl LinearRegressionNode {
    pub fn new(x_columns: Vec<String>, y_column: String, intercept: bool) -> Self {
        Self {
            meta: port_layout(),
            x_columns,
            y_column,
            intercept,
        }
    }
}

#[derive(Debug, JsonSchema, Deserialize)]
pub struct LinearRegressionNodeSpec {
    pub x_columns: Vec<String>,
    pub y_column: String,
    #[serde(default = "default_intercept")]
    pub intercept: bool,
}

fn default_intercept() -> bool {
    true
}

pub struct LinearRegressionNodeFactory {}

/// Static port layout for every [`LinearRegressionNode`]: one untyped input
/// (the data to regress) and one untyped output (the summary table).
fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for LinearRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "linear_regression"
    }

    fn desc(&self) -> &'static str {
        "Fits an OLS linear regression and outputs coefficients, SE, t-stats, and p-values."
    }

    fn doc(&self) -> &'static str {
        "A transform node that fits an OLS linear regression on the input \
        DataFrame. Accepts named X columns and a Y column, optionally with an \
        intercept term. Outputs a summary table with coefficients, standard \
        errors, t-statistics, p-values, R², and the number of observations."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LinearRegressionNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: LinearRegressionNodeSpec = serde_json::from_value(spec)?;
        let node =
            LinearRegressionNode::new(node_spec.x_columns, node_spec.y_column, node_spec.intercept);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<LinearRegressionNodeSpec>(spec, "linear_regression")?;
        let out = ctx.output_var.to_string();
        let fit = ctx.fresh_var("lm_fit");
        let smry = ctx.fresh_var("lm_smry");
        let coefs = ctx.fresh_var("lm_coefs");
        let input = input_0(ctx).to_string();
        let formula = r_formula(&s.y_column, &s.x_columns, s.intercept);
        let code = vec![
            format!("# Linear regression: {formula}"),
            format!("{fit} <- lm({formula}, data = {input})"),
            format!("{smry} <- summary({fit})"),
            format!("{coefs} <- as.data.frame({smry}$coefficients)"),
            format!("names({coefs}) <- c(\"coefficient\", \"std_error\", \"t_stat\", \"p_value\")"),
            format!("{coefs}$term <- rownames({coefs})"),
            format!(
                "{out} <- {coefs}[, c(\"term\", \"coefficient\", \"std_error\", \"t_stat\", \"p_value\")]"
            ),
            format!("{out}$r_squared <- {smry}$r.squared"),
            format!("{out}$n_obs <- {fit}$df + length({fit}$residuals) - {fit}$df"),
            format!("{out}$n_obs <- length({fit}$residuals)"),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
}

#[async_trait]
impl DagNode for LinearRegressionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "linear_regression"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(LinearRegressionError::EmptyInput)?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "linear_regression".to_string(),
                msg: format!("collect failed: {e}"),
            })?;

        // Extract Y column.
        let y = extract_column(&batches, &self.y_column)?;

        // Extract each X column.
        let mut x_owned: Vec<Vec<f64>> = Vec::with_capacity(self.x_columns.len());
        for col_name in &self.x_columns {
            let x = extract_column(&batches, col_name)?;
            x_owned.push(x);
        }
        let x_slices: Vec<&[f64]> = x_owned.iter().map(|v| v.as_slice()).collect();

        // Run OLS.
        let reg = regression::ols(&x_slices, &y, self.intercept)
            .map_err(|e| LinearRegressionError::Regression(e.to_string()))?;

        // Build output batch → DataFrame.
        let batch = build_result_batch(&reg, self.intercept);
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "linear_regression".to_string(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
            resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
                std::path::PathBuf::from("."),
            )),
            global_sem: None,
        }
    }
    use super::*;
    use arrow_array::{Float64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    /// Helper: build a simple `RecordBatch` from named float64 columns.
    fn make_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect();
        let arrays: Vec<Arc<dyn Array>> = columns
            .iter()
            .map(|(_, vals)| Arc::new(Float64Array::from(vals.clone())) as Arc<dyn Array>)
            .collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    #[tokio::test]
    async fn test_perfect_fit() {
        // y = 2·x + 1, no noise
        let x: Vec<f64> = (0..5).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|v| 2.0 * v + 1.0).collect();
        let batch = make_batch(vec![("x", x), ("y", y)]);

        let mut node = LinearRegressionNode::new(vec!["x".to_string()], "y".to_string(), true);
        let input = dag_core::node::NodeInput {
            port: 0,
            // df_name: "src".to_string(),
            data: datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
        };
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(outs.len(), 1);
        let df = outs[&0].clone();
        let rows = df.collect().await.unwrap();
        let total: usize = rows.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 2); // intercept + x1

        // Verify the fitted coefficients: intercept ≈ 1.0, slope ≈ 2.0.
        let coeff_col = rows
            .iter()
            .flat_map(|b| {
                b.column(1)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .iter()
            })
            .map(|v| v.unwrap())
            .collect::<Vec<f64>>();
        assert!(
            (coeff_col[0] - 1.0).abs() < 1e-9,
            "intercept should be ~1.0, got {}",
            coeff_col[0]
        );
        assert!(
            (coeff_col[1] - 2.0).abs() < 1e-9,
            "slope should be ~2.0, got {}",
            coeff_col[1]
        );

        // R² for a perfect fit is 1.0.
        let r2 = rows
            .iter()
            .flat_map(|b| {
                b.column(5)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .iter()
            })
            .map(|v| v.unwrap())
            .next()
            .unwrap();
        assert!((r2 - 1.0).abs() < 1e-9, "R² should be ~1.0, got {}", r2);
    }

    #[tokio::test]
    async fn test_no_intercept() {
        let x: Vec<f64> = (1..6).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|v| 3.0 * v).collect();
        let batch = make_batch(vec![("x", x), ("y", y)]);

        let mut node = LinearRegressionNode::new(vec!["x".to_string()], "y".to_string(), false);
        let input = dag_core::node::NodeInput {
            port: 0,
            // df_name: "src".to_string(),
            data: datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
        };
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let rows = outs[&0].clone().collect().await.unwrap();
        assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 1); // only slope
    }
}
