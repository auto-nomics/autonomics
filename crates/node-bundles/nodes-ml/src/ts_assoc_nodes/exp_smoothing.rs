use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Exponential Smoothing
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExpSmoothingSpec {
    pub value_column: String,
    #[serde(default = "d_es_alpha")]
    pub alpha: f64,
    #[serde(default = "d_es_forecast")]
    pub n_forecast: usize,
}
fn d_es_alpha() -> f64 {
    0.3
}
fn d_es_forecast() -> usize {
    5
}

pub struct ExpSmoothingFactory;
impl NodeFactory for ExpSmoothingFactory {
    fn kind(&self) -> &'static str {
        "ml_exp_smoothing"
    }
    fn desc(&self) -> &'static str {
        "Simple exponential smoothing + forecast."
    }
    fn doc(&self) -> &'static str {
        "ExpSmoothing: single exponential smoothing for time series without trend. Outputs fitted values + h-step forecast."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ExpSmoothingSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ExpSmoothingSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ExpSmoothingNode {
            value_column: s.value_column,
            alpha: s.alpha,
            n_forecast: s.n_forecast,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ExpSmoothingNode {
    value_column: String,
    alpha: f64,
    n_forecast: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ExpSmoothingNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_exp_smoothing"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_numeric_column(&batches, &self.value_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_exp_smoothing".into(),
                msg: e.to_string(),
            }
        })?;
        let result = ml::timeseries::exponential_smoothing(&data, self.alpha, self.n_forecast)
            .map_err(|e| DagError::NodeError {
                node_type: "ml_exp_smoothing".into(),
                msg: e.to_string(),
            })?;
        let n = data.len();
        let total = n + self.n_forecast;
        let fitted_padded: Vec<f64> = result
            .fitted
            .into_iter()
            .chain(std::iter::repeat_n(f64::NAN, self.n_forecast))
            .collect();
        let forecast_padded: Vec<f64> = std::iter::repeat_n(f64::NAN, n)
            .chain(result.forecast)
            .collect();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("index", DataType::UInt32, false),
                Field::new("fitted", DataType::Float64, true),
                Field::new("forecast", DataType::Float64, true),
            ])),
            vec![
                Arc::new(arrow_array::UInt32Array::from(
                    (0..total as u32).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(fitted_padded)),
                Arc::new(Float64Array::from(forecast_padded)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_exp_smoothing".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
