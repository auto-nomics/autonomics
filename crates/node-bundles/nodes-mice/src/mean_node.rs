//! `mice_impute_mean` DAG node — unconditional mean imputation.

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
    build_imputation_batch, extract_f64, extract_observed, imputation_output_schema, test_node_ctx,
};
use crate::error::MiceNodeError;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MiceImputeMeanNodeSpec {
    pub y_column: String,
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Clone)]
pub struct MiceImputeMeanNode {
    meta: NodePorts,
    spec: MiceImputeMeanNodeSpec,
}

pub struct MiceImputeMeanNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(imputation_output_schema()))
}

impl NodeFactory for MiceImputeMeanNodeFactory {
    fn kind(&self) -> &'static str {
        "mice_impute_mean"
    }
    fn desc(&self) -> &'static str {
        "MICE imputation by unconditional mean."
    }
    fn doc(&self) -> &'static str {
        "Reproduces R's `mice.impute.mean`: imputes the arithmetic mean of \
        observed values into every missing location. Use only for baselines."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceImputeMeanNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceImputeMeanNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MiceImputeMeanNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<MiceImputeMeanNodeSpec>(spec, "mice_impute_mean")?;
        let out = ctx.output_var.to_string();
        let fit_var = ctx.fresh_var("mean_fit");
        let input = input_0(ctx).to_string();
        let yc = s.y_column.clone();
        let code = vec![
            "src <- as.data.frame(src)".to_string(),
            "# Mean imputation".to_string(),
            "library(mice)".to_string(),
            format!("{fit_var} <- mice.impute.mean(y = {input}${}, ry = !is.na({input}${}))", yc, yc),
            "# Observed mean for cross-validation".to_string(),
            format!("{{ obs_mean <- mean({input}${yc}[!is.na({input}${yc})]); {out} <- data.frame(imputed = as.numeric({fit_var}), observed_mean = obs_mean, deviation_from_mean = as.numeric({fit_var}) - obs_mean) }}"),
            format!("print({out})"),
        ];
        Ok(NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["mice".into()]
    }
}

#[async_trait]
impl DagNode for MiceImputeMeanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice_impute_mean"
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
                node_type: "mice_impute_mean".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_mean".into(),
                msg: e.to_string(),
            })?;
        let y = extract_f64(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_mean".into(),
            msg: e.to_string(),
        })?;
        let ry = extract_observed(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_mean".into(),
            msg: e.to_string(),
        })?;
        let mut rng = match self.spec.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let imputed = mice::mean::impute_mean(&y, &ry, None, &mut rng);

        let batch = build_imputation_batch(&imputed).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_mean".into(),
            msg: format!("Arrow: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_mean".into(),
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

    fn make_batch(values: Vec<f64>) -> arrow_array::RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("y", DataType::Float64, false)]));
        arrow_array::RecordBatch::try_new(schema, vec![Arc::new(Float64Array::from(values))]).unwrap()
    }

    #[tokio::test]
    async fn test_mean_basic() {
        let mut y = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        y[1] = f64::NAN;
        y[3] = f64::NAN;
        let batch = make_batch(y.clone());
        let observed: Vec<f64> = y.iter().filter(|v| !v.is_nan()).copied().collect();
        let expected_mean: f64 = observed.iter().sum::<f64>() / observed.len() as f64;

        let spec = MiceImputeMeanNodeSpec {
            y_column: "y".into(),
            seed: Some(42),
        };
        let mut node = MiceImputeMeanNode {
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
        assert_eq!(imputed.len(), 2);
        for &v in &imputed {
            assert!((v - expected_mean).abs() < 1e-12, "got {v}, expected {expected_mean}");
        }
    }
}
