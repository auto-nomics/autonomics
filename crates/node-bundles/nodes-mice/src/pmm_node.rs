//! `mice_impute_pmm` DAG node — Predictive Mean Matching for numeric `y`.
//!
//! Wraps [`mice::pmm::impute_pmm`]. The node takes a single upstream
//! DataFrame containing the response column (with `NaN` marking missing
//! entries) and the predictor columns, runs PMM imputation, and emits a
//! single-column DataFrame with the imputed values for the missing rows.

use std::sync::Arc;

use async_trait::async_trait;
use rand::rngs::StdRng;
use rand::SeedableRng;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::codegen::CodegenError;
use dag_core::codegen::context::{CodegenCtx, NodeCodegen};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::NodeFactory;
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::NodeCtx,
};

use crate::common::{
    build_imputation_batch, build_predictor_matrix, extract_f64, extract_observed,
    imputation_output_schema, test_node_ctx,
};
use crate::error::MiceNodeError;

/// Spec for [`MiceImputePmmNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MiceImputePmmNodeSpec {
    /// Response column name. Missing entries (NaN) are imputed.
    pub y_column: String,
    /// Predictor column names. Numeric columns are accepted; non-numeric
    /// columns are rejected.
    #[serde(default)]
    pub predictors: Vec<String>,
    /// Donor pool size. Default `5`.
    #[serde(default = "default_donors")]
    pub donors: usize,
    /// Matching type: 0 (predicted obs × predicted mis), 1 (predicted obs ×
    /// drawn mis — R default), 2 (drawn obs × drawn mis).
    #[serde(default = "default_matchtype")]
    pub matchtype: usize,
    /// Ridge penalty for the underlying `.norm.draw` regression.
    /// Default `1e-5`.
    #[serde(default = "default_ridge")]
    pub ridge: f64,
    /// RNG seed for reproducibility.
    #[serde(default)]
    pub seed: Option<u64>,
}

fn default_donors() -> usize {
    5
}
fn default_matchtype() -> usize {
    1
}
fn default_ridge() -> f64 {
    1e-5
}

#[derive(Clone)]
pub struct MiceImputePmmNode {
    meta: NodePorts,
    spec: MiceImputePmmNodeSpec,
}

pub struct MiceImputePmmNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(imputation_output_schema()))
}

impl NodeFactory for MiceImputePmmNodeFactory {
    fn kind(&self) -> &'static str {
        "mice_impute_pmm"
    }
    fn desc(&self) -> &'static str {
        "MICE imputation by Predictive Mean Matching (PMM)."
    }
    fn doc(&self) -> &'static str {
        "Reproduces R's `mice.impute.pmm`. Fits a Bayesian linear regression \
        on (response, predictors) for observed rows, draws a posterior draw \
        of the coefficients, then imputes each missing row by selecting a \
        donor value from the observed responses whose predicted value is \
        closest to the missing row's predicted value."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceImputePmmNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceImputePmmNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MiceImputePmmNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<MiceImputePmmNodeSpec>(spec, "mice_impute_pmm")?;
        let out = ctx.output_var.to_string();
        let fit_var = ctx.fresh_var("pmm_fit");
        let input = input_0(ctx).to_string();

        let preds_expr = if s.predictors.is_empty() {
            String::new()
        } else {
            format!("cbind({})", s.predictors.iter().map(|c| format!("{input}${c}")).collect::<Vec<_>>().join(", "))
        };

        let yc = s.y_column.clone();
        let code = vec![
            "src <- as.data.frame(src)".to_string(),
            "# PMM imputation (mice::mice.impute.pmm)".to_string(),
            "library(mice)".to_string(),
            format!("{fit_var} <- mice.impute.pmm("),
            format!("  y = {input}${},", yc),
            format!("  ry = !is.na({input}${}),", yc),
            format!("  x = {preds_expr},"),
            format!("  wy = is.na({input}${}),", yc),
            format!("  donors = {},", s.donors),
            format!("  matchtype = {}", s.matchtype),
            ")".to_string(),
            "# Augment with observed-set statistics for cross-validation".to_string(),
            format!("{{ obs_y <- {input}${yc}[!is.na({input}${yc})]; obs_mean <- mean(obs_y); obs_sd <- sd(obs_y); obs_min <- min(obs_y); obs_max <- max(obs_y); {out} <- data.frame(imputed = as.numeric({fit_var}), in_observed_set = as.numeric({fit_var}) %in% obs_y, observed_mean = obs_mean, observed_sd = obs_sd, observed_min = obs_min, observed_max = obs_max) }}"),
            format!("print({out})"),
        ];
        Ok(NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["mice".into()]
    }
}

#[async_trait]
impl DagNode for MiceImputePmmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice_impute_pmm"
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
                node_type: "mice_impute_pmm".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_pmm".into(),
                msg: e.to_string(),
            })?;

        let y = extract_f64(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_pmm".into(),
            msg: e.to_string(),
        })?;
        let ry = extract_observed(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_pmm".into(),
            msg: e.to_string(),
        })?;
        let wy: Vec<bool> = ry.iter().map(|r| !*r).collect();
        let (x, _n) = build_predictor_matrix(&batches, &self.spec.predictors).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_pmm".into(),
            msg: e.to_string(),
        })?;

        let mut rng = match self.spec.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let imputed = mice::pmm::impute_pmm(
            &y,
            &ry,
            &x,
            Some(&wy),
            self.spec.donors,
            self.spec.matchtype,
            self.spec.ridge,
            &mut rng,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "mice_impute_pmm".into(),
            msg: e.to_string(),
        })?;

        let batch = build_imputation_batch(&imputed).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_pmm".into(),
            msg: format!("Arrow: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_pmm".into(),
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
    async fn test_pmm_basic() {
        // Simple dataset: y = 2*x + noise; mark 3 values as missing.
        let x: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|v| 2.0 * v + 1.0).collect();
        let mut y_mis = y.clone();
        y_mis[2] = f64::NAN;
        y_mis[5] = f64::NAN;
        y_mis[7] = f64::NAN;
        let batch = make_batch(vec![("x", x.clone()), ("y", y_mis)]);

        let spec = MiceImputePmmNodeSpec {
            y_column: "y".into(),
            predictors: vec!["x".into()],
            donors: 5,
            matchtype: 1,
            ridge: 1e-5,
            seed: Some(42),
        };
        let mut node = MiceImputePmmNode {
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
        let imputed: Vec<f64> = batches
            .iter()
            .flat_map(|b| b.column(0).as_any().downcast_ref::<Float64Array>().unwrap().iter())
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(imputed.len(), 3);
        // PMM picks from observed values, so each imputed value must come
        // from the observed y values.
        for &v in &imputed {
            assert!(y.contains(&v), "PMM imputation {v} not in observed y");
        }
    }
}
