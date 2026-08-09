//! `mice_impute_norm` DAG node — Bayesian linear regression imputation.

use std::sync::Arc;

use async_trait::async_trait;
use rand::SeedableRng;
use rand::rngs::StdRng;
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

use crate::common::{
    build_imputation_batch, build_predictor_matrix, extract_f64, extract_observed,
    imputation_output_schema, test_node_ctx,
};
use crate::error::MiceNodeError;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MiceImputeNormNodeSpec {
    pub y_column: String,
    #[serde(default)]
    pub predictors: Vec<String>,
    #[serde(default = "default_ridge")]
    pub ridge: f64,
    #[serde(default)]
    pub seed: Option<u64>,
}

fn default_ridge() -> f64 {
    1e-5
}

#[derive(Clone)]
pub struct MiceImputeNormNode {
    meta: NodePorts,
    spec: MiceImputeNormNodeSpec,
}

pub struct MiceImputeNormNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(imputation_output_schema()))
}

impl NodeFactory for MiceImputeNormNodeFactory {
    fn kind(&self) -> &'static str {
        "mice_impute_norm"
    }
    fn desc(&self) -> &'static str {
        "MICE imputation by Bayesian linear regression (norm)."
    }
    fn doc(&self) -> &'static str {
        "Reproduces R's `mice.impute.norm`. Fits Bayesian linear regression \
        on observed rows, draws a posterior sample of (β*, σ*), and imputes \
        missing rows as x · β* + N(0, σ*) draws."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceImputeNormNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceImputeNormNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MiceImputeNormNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<MiceImputeNormNodeSpec>(spec, "mice_impute_norm")?;
        let out = ctx.output_var.to_string();
        let fit_var = ctx.fresh_var("norm_fit");
        let input = input_0(ctx).to_string();
        let preds_expr = if s.predictors.is_empty() {
            String::new()
        } else {
            format!(
                "cbind({})",
                s.predictors
                    .iter()
                    .map(|c| format!("{input}${c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let yc = s.y_column.clone();
        let code = vec![
            "src <- as.data.frame(src)".to_string(),
            "# Bayesian linear regression imputation".to_string(),
            "library(mice)".to_string(),
            format!("{fit_var} <- mice.impute.norm("),
            format!("  y = {input}${},", yc),
            format!("  ry = !is.na({input}${}),", yc),
            format!("  x = {preds_expr},"),
            format!("  wy = is.na({input}${}),", yc),
            format!("  ridge = {}", s.ridge),
            ")".to_string(),
            "# Augment with predicted-mean + observed-set stats for xval".to_string(),
            {
                let formula_part = format!(
                    "as.formula(paste(\"{yc} ~\", paste(setdiff(names({input}), \"{yc}\"), collapse = \" + \")))",
                    yc = yc,
                    input = input
                );
                format!(
                    "{{ obs_y <- {input}${yc}[!is.na({input}${yc})]; obs_mean <- mean(obs_y); obs_sd <- sd(obs_y); formula_lm <- {formula}; fit_lm <- lm(formula_lm, data = {input}); pred_obs <- predict(fit_lm, newdata = {input}[is.na({input}${yc}), ]); {out} <- data.frame(imputed = as.numeric({fit_var}), predicted_mean = as.numeric(pred_obs), deviation_from_mean = as.numeric({fit_var}) - as.numeric(pred_obs), observed_mean = obs_mean, observed_sd = obs_sd) }}",
                    input = input,
                    yc = yc,
                    formula = formula_part,
                    fit_var = fit_var,
                    out = out
                )
            },
            format!("print({out})"),
        ];
        Ok(NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["mice".into()]
    }
}

#[async_trait]
impl DagNode for MiceImputeNormNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice_impute_norm"
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
                node_type: "mice_impute_norm".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_norm".into(),
                msg: e.to_string(),
            })?;

        let y = extract_f64(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_norm".into(),
            msg: e.to_string(),
        })?;
        let ry =
            extract_observed(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
                node_type: "mice_impute_norm".into(),
                msg: e.to_string(),
            })?;
        let wy: Vec<bool> = ry.iter().map(|r| !*r).collect();
        let (x, _n) = build_predictor_matrix(&batches, &self.spec.predictors).map_err(|e| {
            DagError::NodeError {
                node_type: "mice_impute_norm".into(),
                msg: e.to_string(),
            }
        })?;

        let mut rng = match self.spec.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let imputed = mice::norm::impute_norm(&y, &ry, &x, Some(&wy), self.spec.ridge, &mut rng)
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_norm".into(),
                msg: e.to_string(),
            })?;

        let batch = build_imputation_batch(&imputed).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_norm".into(),
            msg: format!("Arrow: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_norm".into(),
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
            .map(|(_, vals)| {
                Arc::new(Float64Array::from(vals.clone())) as Arc<dyn arrow_array::Array>
            })
            .collect();
        arrow_array::RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    #[tokio::test]
    async fn test_norm_basic() {
        // y = 2*x + noise
        let x: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|v| 2.0 * v + 1.0).collect();
        let mut y_mis = y.clone();
        y_mis[3] = f64::NAN;
        y_mis[8] = f64::NAN;
        y_mis[15] = f64::NAN;
        let batch = make_batch(vec![("x", x.clone()), ("y", y_mis)]);

        let spec = MiceImputeNormNodeSpec {
            y_column: "y".into(),
            predictors: vec!["x".into()],
            ridge: 1e-5,
            seed: Some(42),
        };
        let mut node = MiceImputeNormNode {
            meta: port_layout(),
            spec,
        };

        let input = dag_core::node::NodeInput {
            port: 0,
            data: datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
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
            .flat_map(|b| {
                b.column(0)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .iter()
            })
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(imputed.len(), 3);
        // All imputed values should be finite and within a reasonable range
        // given the model y ~ 2x + 1.
        for &v in &imputed {
            assert!(v.is_finite(), "imputed value {v} not finite");
            assert!((0.0..50.0).contains(&v), "imputed {v} out of range");
        }
    }
}
