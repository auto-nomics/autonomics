//! `mice_impute_logreg` DAG node — Bayesian logistic regression imputation.

use std::sync::Arc;

use async_trait::async_trait;
use rand::rngs::StdRng;
use rand::SeedableRng;
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
pub struct MiceImputeLogregNodeSpec {
    /// Binary outcome column name (values 0 or 1, NaN for missing).
    pub y_column: String,
    /// Predictor column names.
    #[serde(default)]
    pub predictors: Vec<String>,
    /// RNG seed.
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Clone)]
pub struct MiceImputeLogregNode {
    meta: NodePorts,
    spec: MiceImputeLogregNodeSpec,
}

pub struct MiceImputeLogregNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(imputation_output_schema()))
}

impl NodeFactory for MiceImputeLogregNodeFactory {
    fn kind(&self) -> &'static str {
        "mice_impute_logreg"
    }
    fn desc(&self) -> &'static str {
        "MICE imputation by Bayesian logistic regression (logreg)."
    }
    fn doc(&self) -> &'static str {
        "Reproduces R's `mice.impute.logreg`: augments the data with the \
        White-Daniel-Royston (2010) scheme to evade perfect prediction, fits \
        `glm.fit` with binomial/logit, draws β* from N(β̂, V), and imputes \
        missing binary outcomes by thresholding predicted scores against \
        uniform random deviates."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceImputeLogregNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceImputeLogregNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MiceImputeLogregNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<MiceImputeLogregNodeSpec>(spec, "mice_impute_logreg")?;
        let out = ctx.output_var.to_string();
        let fit_var = ctx.fresh_var("logreg_fit");
        let input = input_0(ctx).to_string();
        let preds_expr = if s.predictors.is_empty() {
            String::new()
        } else {
            format!("cbind({})", s.predictors.iter().map(|c| format!("{input}${c}")).collect::<Vec<_>>().join(", "))
        };
        let yc = s.y_column.clone();
        let code = vec![
            "src <- as.data.frame(src)".to_string(),
            "# Logistic regression imputation".to_string(),
            "library(mice)".to_string(),
            format!("{fit_var} <- mice.impute.logreg("),
            format!("  y = {input}${},", yc),
            format!("  ry = !is.na({input}${}),", yc),
            format!("  x = {preds_expr},"),
            format!("  wy = is.na({input}${})", yc),
            ")".to_string(),
            "# Observed-set statistics + glm predicted probability for xval".to_string(),
            {
                let formula_part = format!(
                    "as.formula(paste(\"{yc} ~\", paste(setdiff(names({input}), \"{yc}\"), collapse = \" + \")))",
                    yc = yc, input = input
                );
                format!(
                    "{{ obs_y <- {input}${yc}[!is.na({input}${yc})]; obs_mean <- mean(obs_y); formula_glm <- {formula}; fit_glm <- glm(formula_glm, family = binomial, data = {input}); pred_obs <- predict(fit_glm, newdata = {input}[is.na({input}${yc}), ], type = \"response\"); binarised_pred <- as.numeric(pred_obs > 0.5); {out} <- data.frame(imputed = as.numeric({fit_var}), is_binary = as.numeric({fit_var}) %in% c(0, 1), predicted_prob = as.numeric(pred_obs), binarised_pred = binarised_pred, observed_mean = obs_mean) }}",
                    input = input, yc = yc, formula = formula_part, fit_var = fit_var, out = out
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
impl DagNode for MiceImputeLogregNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice_impute_logreg"
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
                node_type: "mice_impute_logreg".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_logreg".into(),
                msg: e.to_string(),
            })?;
        let y = extract_f64(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_logreg".into(),
            msg: e.to_string(),
        })?;
        let ry = extract_observed(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_logreg".into(),
            msg: e.to_string(),
        })?;
        let wy: Vec<bool> = ry.iter().map(|r| !*r).collect();
        let (x, _n) = build_predictor_matrix(&batches, &self.spec.predictors).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_logreg".into(),
            msg: e.to_string(),
        })?;

        let mut rng = match self.spec.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let imputed = mice::logreg::impute_logreg(&y, &ry, &x, Some(&wy), &mut rng)
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_logreg".into(),
                msg: e.to_string(),
            })?;

        let batch = build_imputation_batch(&imputed).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_logreg".into(),
            msg: format!("Arrow: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_logreg".into(),
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
    async fn test_logreg_basic() {
        // Binary outcome y in {0, 1}, predictor x.
        let n = 50;
        let x: Vec<f64> = (0..n).map(|i| (i as f64 - 25.0) / 10.0).collect();
        let linpred: Vec<f64> = x.iter().map(|&xi| 0.5 + 1.0 * xi).collect();
        let mut y = Vec::with_capacity(n);
        for i in 0..n {
            let p = 1.0 / (1.0 + (-linpred[i]).exp());
            y.push(if (i as f64 * 0.123).fract() < p { 1.0 } else { 0.0 });
        }
        let mut y_mis = y.clone();
        y_mis[5] = f64::NAN;
        y_mis[10] = f64::NAN;
        y_mis[15] = f64::NAN;
        y_mis[20] = f64::NAN;
        let batch = make_batch(vec![("x", x.clone()), ("y", y_mis)]);

        let spec = MiceImputeLogregNodeSpec {
            y_column: "y".into(),
            predictors: vec!["x".into()],
            seed: Some(42),
        };
        let mut node = MiceImputeLogregNode {
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
        assert_eq!(imputed.len(), 4);
        for &v in &imputed {
            assert!(v == 0.0 || v == 1.0, "imputed {v} not binary");
        }
    }
}
