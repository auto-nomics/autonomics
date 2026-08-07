//! `mice_impute_sample` DAG node — random draw from observed values.

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
pub struct MiceImputeSampleNodeSpec {
    pub y_column: String,
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Clone)]
pub struct MiceImputeSampleNode {
    meta: NodePorts,
    spec: MiceImputeSampleNodeSpec,
}

pub struct MiceImputeSampleNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(imputation_output_schema()))
}

impl NodeFactory for MiceImputeSampleNodeFactory {
    fn kind(&self) -> &'static str {
        "mice_impute_sample"
    }
    fn desc(&self) -> &'static str {
        "MICE imputation by sampling from observed values."
    }
    fn doc(&self) -> &'static str {
        "Reproduces R's `mice.impute.sample`: imputes each missing location \
        with a uniform draw (with replacement) from the observed values."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceImputeSampleNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceImputeSampleNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MiceImputeSampleNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<MiceImputeSampleNodeSpec>(spec, "mice_impute_sample")?;
        let out = ctx.output_var.to_string();
        let fit_var = ctx.fresh_var("sample_fit");
        let input = input_0(ctx).to_string();
        let yc = s.y_column.clone();
        let code = vec![
            "src <- as.data.frame(src)".to_string(),
            "# Sample imputation".to_string(),
            "library(mice)".to_string(),
            format!("{fit_var} <- mice.impute.sample(y = {input}${}, ry = !is.na({input}${}))", yc, yc),
            "# Observed-set statistics for xval".to_string(),
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
impl DagNode for MiceImputeSampleNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice_impute_sample"
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
                node_type: "mice_impute_sample".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice_impute_sample".into(),
                msg: e.to_string(),
            })?;
        let y = extract_f64(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_sample".into(),
            msg: e.to_string(),
        })?;
        let ry = extract_observed(&batches, &self.spec.y_column).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_sample".into(),
            msg: e.to_string(),
        })?;
        let mut rng = match self.spec.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let imputed = mice::sample::impute_sample(&y, &ry, None, &mut rng);

        let batch = build_imputation_batch(&imputed).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_sample".into(),
            msg: format!("Arrow: {e}"),
        })?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "mice_impute_sample".into(),
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
    async fn test_sample_basic() {
        let mut y = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        y[1] = f64::NAN;
        y[3] = f64::NAN;
        let observed: Vec<f64> = y.iter().filter(|v| !v.is_nan()).copied().collect();
        let batch = make_batch(y);
        let spec = MiceImputeSampleNodeSpec {
            y_column: "y".into(),
            seed: Some(42),
        };
        let mut node = MiceImputeSampleNode {
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
            assert!(observed.contains(&v), "sample {v} not from observed");
        }
    }
}
