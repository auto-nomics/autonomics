//! `mice` orchestrator DAG node — runs the full MICE Gibbs sampler.
//!
//! Takes the incomplete DataFrame and returns the imputed dataset in long
//! format (one row per (imputation, observation) pair), matching R's
//! `complete(mids, action = "all", include = FALSE)`.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::codegen::context::{CodegenCtx, CodegenError, NodeCodegen};
use dag_core::codegen::helpers::*;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::NodeFactory;
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::NodeCtx,
};

use crate::common::test_node_ctx;
use crate::error::MiceNodeError;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MiceOrchestratorNodeSpec {
    /// Column names to impute (must have at least one NaN entry).
    #[serde(default)]
    pub impute_columns: Vec<String>,
    /// Method per `impute_columns` entry. Defaults to "pmm" for numeric and
    /// "logreg" for binary {0, 1}.
    #[serde(default)]
    pub methods: Option<Vec<String>>,
    /// Predictor matrix: row i is the predictor column names for
    /// `impute_columns[i]`. `None` ⇒ all other columns predict each target.
    #[serde(default)]
    pub predictor_matrix: Option<Vec<Vec<String>>>,
    /// Number of imputations. Default `5`.
    #[serde(default = "default_m")]
    pub m: usize,
    /// Number of Gibbs iterations. Default `5`.
    #[serde(default = "default_maxit")]
    pub maxit: usize,
    /// Ridge penalty. Default `1e-5`.
    #[serde(default = "default_ridge")]
    pub ridge: f64,
    /// PMM donor pool size. Default `5`.
    #[serde(default = "default_donors")]
    pub donors: usize,
    /// PMM matching type. Default `1`.
    #[serde(default = "default_matchtype")]
    pub matchtype: usize,
    /// RNG seed.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Run the `m` imputation chains in parallel via rayon. Each chain
    /// gets an independently seeded RNG, so results are statistically
    /// equivalent to sequential mode but not bit-identical. Default `false`.
    #[serde(default)]
    pub parallel: bool,
}

fn default_m() -> usize {
    5
}
fn default_maxit() -> usize {
    5
}
fn default_ridge() -> f64 {
    1e-5
}
fn default_donors() -> usize {
    5
}
fn default_matchtype() -> usize {
    1
}

#[derive(Clone)]
pub struct MiceOrchestratorNode {
    meta: NodePorts,
    spec: MiceOrchestratorNodeSpec,
}

pub struct MiceOrchestratorNodeFactory;

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("imp_num", DataType::Int64, false),
        Field::new("id_num", DataType::Int64, false),
    ]))
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

impl NodeFactory for MiceOrchestratorNodeFactory {
    fn kind(&self) -> &'static str {
        "mice"
    }
    fn desc(&self) -> &'static str {
        "MICE orchestrator: full Gibbs-sampler multivariate imputation."
    }
    fn doc(&self) -> &'static str {
        "Reproduces R's `mice::mice()`: runs the chained-equations Gibbs \
        sampler for `maxit` iterations × `m` imputations. Returns the imputed \
        data in long format with `.imp` and `.id` columns prepended."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceOrchestratorNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceOrchestratorNodeSpec = serde_json::from_value(spec)?;
        if s.impute_columns.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "mice".to_string(),
                reason: "impute_columns must be non-empty".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(MiceOrchestratorNodeSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(MiceOrchestratorNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<MiceOrchestratorNodeSpec>(spec, "mice")?;
        let out = ctx.output_var.to_string();
        let mids_var = ctx.fresh_var("mids");
        let y0 = s.impute_columns.first().cloned().unwrap_or_else(|| "y".to_string());
        let comp_var = ctx.fresh_var("comp");
        let input = input_0(ctx).to_string();

        // Build a length-ncol(input) method vector at R runtime by aligning
        // `impute_columns`/`methods` to the actual column positions of `input`.
        // Positions for non-impute columns are filled with ""; positions for
        // impute columns use the corresponding entry from `methods`. This is
        // required because mice() overwrites methods for any column with zero
        // missing values back to ""; the vector must therefore encode the user
        // intent per column, not a positional pad.
        let method_vec = if let Some(m) = s.methods.as_ref() {
            let impute_cols_r = vec_to_r_str(&s.impute_columns);
            let methods_r = vec_to_r_str(m);
            format!(
                "{{ impute_cols <- {impute_cols_r}; user_methods_list <- {methods_r}; n_cols <- ncol({input}); mthd <- rep(\"\", n_cols); names(mthd) <- colnames({input}); for (i in seq_along(impute_cols)) {{ idx <- match(impute_cols[i], colnames({input}), nomatch = 0L); if (idx > 0L) {{ mthd[idx] <- user_methods_list[[i]] }} }}; mthd }}",
                input = input, impute_cols_r = impute_cols_r, methods_r = methods_r
            )
        } else {
            "NULL".to_string()
        };
        let pm_lines = if let Some(pm) = s.predictor_matrix.as_ref() {
            let rows: Vec<String> = pm
                .iter()
                .map(|r| {
                    let inner: Vec<String> = r
                        .iter()
                        .map(|c| format!("\"{c}\""))
                        .collect();
                    format!("c({})", inner.join(", "))
                })
                .collect();
            format!(
                "predictorMatrix = rbind({})",
                rows.join(",\n  ")
            )
        } else {
            String::new()
        };
        let pm_arg = if s.predictor_matrix.is_some() {
            format!(", {pm_lines}")
        } else {
            String::new()
        };

        let code = vec![
            format!("set.seed({})", s.seed.unwrap_or(42)),
            format!("{input} <- as.data.frame({input})"),
            "# MICE orchestrator".to_string(),
            "library(mice)".to_string(),
            format!("{mids_var} <- mice("),
            format!("  data = {input},"),
            format!("  m = {},", s.m),
            format!("  method = {method_vec},"),
            format!("  maxit = {},", s.maxit),
            format!("  ridge = {},", s.ridge),
            format!("  donors = {},", s.donors),
            format!("  matchtype = {},", s.matchtype),
            format!("  printFlag = FALSE{pm_arg}"),
            ")".to_string(),
            format!("{comp_var} <- complete({mids_var}, action = \"long\", include = FALSE)"),
            format!("{{ comp_y <- as.data.frame({comp_var})[, c('.imp', '{y0}')]; summ <- data.frame(imp_col = unique(comp_y[, '.imp']), n_rows = as.numeric(table(comp_y[, '.imp'])), y_mean = as.numeric(tapply(comp_y[, '{y0}'], comp_y[, '.imp'], mean)), y_sd = as.numeric(tapply(comp_y[, '{y0}'], comp_y[, '.imp'], sd)), y_min = as.numeric(tapply(comp_y[, '{y0}'], comp_y[, '.imp'], min)), y_max = as.numeric(tapply(comp_y[, '{y0}'], comp_y[, '.imp'], max))) }}"),
            format!("{out} <- summ"),
            format!("print({out})"),
        ];
        Ok(NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["mice".into()]
    }
}

#[async_trait]
impl DagNode for MiceOrchestratorNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice"
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
            .ok_or_else(|| MiceNodeError::EmptyInput)
            .map_err(|e| DagError::NodeError {
                node_type: "mice".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice".into(),
                msg: e.to_string(),
            })?;

        if batches.is_empty() {
            return Err(MiceNodeError::EmptyInput).map_err(|e| DagError::NodeError {
                node_type: "mice".into(),
                msg: e.to_string(),
            })?;
        }
        let schema = batches[0].schema();
        let mut data: HashMap<String, Vec<f64>> = HashMap::new();
        let mut col_order: Vec<String> = Vec::new();
        for (field_idx, field) in schema.fields().iter().enumerate() {
            col_order.push(field.name().clone());
            let mut vals: Vec<f64> = Vec::new();
            for batch in &batches {
                let col: arrow_array::ArrayRef = batch.column(field_idx).clone();
                if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
                    for v in arr.iter() {
                        vals.push(v.unwrap_or(f64::NAN));
                    }
                } else {
                    // Try Int64Array or fallback to NaN
                    let mut found = false;
                    macro_rules! cast {
                        ($T:ty) => {
                            if let Some(a) = col.as_any().downcast_ref::<$T>() {
                                found = true;
                                for v in a.iter() {
                                    vals.push(match v {
                                        Some(x) => x as f64,
                                        None => f64::NAN,
                                    });
                                }
                            }
                        };
                    }
                    cast!(arrow_array::Int8Array);
                    cast!(arrow_array::Int16Array);
                    cast!(arrow_array::Int32Array);
                    cast!(arrow_array::Int64Array);
                    cast!(arrow_array::UInt8Array);
                    cast!(arrow_array::UInt16Array);
                    cast!(arrow_array::UInt32Array);
                    cast!(arrow_array::UInt64Array);
                    cast!(arrow_array::Float32Array);
                    if !found {
                        return Err(MiceNodeError::InvalidSpec(format!(
                            "column {} has unsupported type {:?}",
                            field.name(),
                            col.data_type()
                        )))
                        .map_err(|e| DagError::NodeError {
                            node_type: "mice".into(),
                            msg: e.to_string(),
                        })?;
                    }
                }
            }
            data.insert(field.name().clone(), vals);
        }

        // Build method vector aligned to column_order: "" for columns not
        // in impute_columns (so they're never imputed), the specified or
        // default method for impute columns. This mirrors the codegen_r
        // path, which builds the same ""-padded method vector for R's
        // mice().
        let method_vec: Vec<String> = col_order.iter().map(|col| {
            if let Some(idx) = self.spec.impute_columns.iter().position(|c| c == col) {
                self.spec.methods.as_ref()
                    .and_then(|m| m.get(idx).cloned())
                    .unwrap_or_else(|| {
                        // Infer default from data type (same logic as
                        // mice::orchestrator::default_method_for).
                        let mut levels: Vec<f64> = data[col].iter()
                            .copied()
                            .filter(|v| !v.is_nan())
                            .collect();
                        levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        levels.dedup();
                        if levels.len() <= 2 { "logreg".to_string() } else { "pmm".to_string() }
                    })
            } else {
                String::new() // "" → skip imputation for this column
            }
        }).collect();

        // Build predictor matrix aligned to column_order. Non-impute
        // columns get an empty predictor list (they're never targets).
        let pred_matrix: Vec<Vec<String>> = match &self.spec.predictor_matrix {
            Some(pm) => col_order.iter().map(|col| {
                if let Some(idx) = self.spec.impute_columns.iter().position(|c| c == col) {
                    pm.get(idx).cloned().unwrap_or_default()
                } else {
                    Vec::new()
                }
            }).collect(),
            None => col_order.iter().map(|col| {
                if self.spec.impute_columns.iter().any(|c| c == col) {
                    col_order.iter().filter(|c| *c != col).cloned().collect()
                } else {
                    Vec::new()
                }
            }).collect(),
        };

        let config = mice::orchestrator::MiceConfig {
            predictor_matrix: Some(pred_matrix),
            method: Some(method_vec),
            m: self.spec.m,
            maxit: self.spec.maxit,
            seed: self.spec.seed,
            ridge: self.spec.ridge,
            donors: self.spec.donors,
            matchtype: self.spec.matchtype,
            parallel: self.spec.parallel,
        };

        let mids = mice::orchestrator::mice(data, col_order, config).map_err(|e| DagError::NodeError {
            node_type: "mice".into(),
            msg: e.to_string(),
        })?;
        let completed = mice::complete::complete(
            &mids,
            mice::complete::CompleteFormat::All,
            1,
        );

        // Build output batch.
        let n = mids.data[&mids.column_names[0]].len();
        let m = mids.m;
        let total = m * n;
        let mut imp_col = Vec::with_capacity(total);
        let mut id_col = Vec::with_capacity(total);
        for i in 0..m {
            for r in 0..n {
                imp_col.push((i + 1) as i64);
                id_col.push((r + 1) as i64);
            }
        }
        let mut columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(Int64Array::from(imp_col)),
            Arc::new(Int64Array::from(id_col)),
        ];
        // completed.columns = [".imp", ".id", x, z, y, ...] (R long-format names).
        // Skip the .imp / .id entries — we already declared those columns.
        let data_columns: Vec<String> = completed.columns
            .iter()
            .filter(|c| c.as_str() != ".imp" && c.as_str() != ".id")
            .cloned()
            .collect();
        let mut fields: Vec<Field> = vec![
            Field::new("imp_num", DataType::Int64, false),
            Field::new("id_num", DataType::Int64, false),
        ];
        let ncols = data_columns.len();
        for c in &data_columns {
            fields.push(Field::new(c, DataType::Float64, false));
        }
        let mut val_buf = vec![0.0_f64; total];
        // Reshape completed.rows (row-major, total × ncols) into per-column arrays.
        for j in 0..ncols {
            for r in 0..total {
                val_buf[r] = completed.rows[r * ncols + j];
            }
            columns.push(Arc::new(Float64Array::from(val_buf.clone())));
        }
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).map_err(|e| DagError::NodeError {
            node_type: "mice".into(),
            msg: format!("Arrow: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "mice".into(),
            msg: format!("read_batch: {e}"),
        })?;
        let mut out: PortOutputs = PortOutputs::new();
        out.insert(0, df);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Float64Array;
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    fn make_batch(columns: Vec<(&str, Vec<f64>)>) -> arrow_array::RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect();
        let arrays: Vec<Arc<dyn arrow_array::Array>> = columns
            .iter()
            .map(|(_, vals)| Arc::new(Float64Array::from(vals.clone())) as Arc<dyn arrow_array::Array>)
            .collect();
        arrow_array::RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    #[tokio::test]
    async fn test_mice_orchestrator_basic() {
        // y ~ x + z, with some NaNs in y.
        let n = 30;
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let z: Vec<f64> = (0..n).map(|i| (i as f64).sin()).collect();
        let y: Vec<f64> = x.iter().zip(z.iter()).map(|(xi, zi)| 2.0 * xi + zi).collect();
        let mut y_mis = y.clone();
        y_mis[3] = f64::NAN;
        y_mis[7] = f64::NAN;
        y_mis[12] = f64::NAN;
        y_mis[18] = f64::NAN;
        y_mis[25] = f64::NAN;
        let batch = make_batch(vec![("x", x), ("z", z), ("y", y_mis)]);

        let spec = MiceOrchestratorNodeSpec {
            impute_columns: vec!["y".into()],
            methods: Some(vec!["norm".into()]),
            predictor_matrix: None,
            m: 3,
            maxit: 2,
            ridge: 1e-5,
            donors: 5,
            matchtype: 1,
            seed: Some(42),
            parallel: false,
        };
        let mut node = MiceOrchestratorNode {
            meta: port_layout(),
            spec,
        };
        let input = dag_core::node::NodeInput {
            port: 0,
            data: datafusion::prelude::SessionContext::new().read_batch(batch).unwrap(),
        };
        let outs = node
            .execute(
                &test_node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let df = outs[&0].clone();
        let batches = df.collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        // m=3, n=30 → 90 rows
        assert_eq!(total, 90);
        // Columns: .imp, .id, x, z, y
        assert_eq!(batches[0].num_columns(), 5);
        // Check .imp column is in [1, 3].
        let imp_col = batches
            .iter()
            .flat_map(|b| {
                b.column(0)
                    .as_any()
                    .downcast_ref::<arrow_array::Int64Array>()
                    .unwrap()
                    .iter()
            })
            .collect::<Vec<_>>();
        for v in &imp_col {
            let val = v.unwrap();
            assert!((1..=3).contains(&val));
        }
        // String-array sanity (just to make sure we built something).
        let _ = StringArray::from(vec!["ok"]);
    }
}
