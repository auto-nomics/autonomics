use super::*;

// ═══════════════════════════════════════════════════════════════════════
// ElasticNet (covers Ridge/Lasso)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ElasticNetSpec {
    pub features: Vec<String>,
    pub target_column: String,
    #[serde(default = "d_en_penalty")]
    pub penalty: f64,
    #[serde(default = "d_en_l1")]
    pub l1_ratio: f64,
    #[serde(default = "d_en_iter")]
    pub max_iter: usize,
    #[serde(default = "d_en_tol")]
    pub tol: f64,
}
fn d_en_penalty() -> f64 {
    0.01
}
fn d_en_l1() -> f64 {
    0.5
}
fn d_en_iter() -> usize {
    1000
}
fn d_en_tol() -> f64 {
    1e-4
}

pub struct ElasticNetFactory;
impl NodeFactory for ElasticNetFactory {
    fn kind(&self) -> &'static str {
        "ml_elastic_net"
    }
    fn desc(&self) -> &'static str {
        "Elastic Net regression (covers Ridge/Lasso via l1_ratio)."
    }
    fn doc(&self) -> &'static str {
        "ElasticNet: L1+L2 regularised regression. l1_ratio=0 → Ridge, l1_ratio=1 → Lasso, 0<l1_ratio<1 → Elastic Net."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ElasticNetSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ElasticNetSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ElasticNetNode {
            features: s.features,
            target_column: s.target_column,
            penalty: s.penalty,
            l1_ratio: s.l1_ratio,
            max_iter: s.max_iter,
            tol: s.tol,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ElasticNetNode {
    features: Vec<String>,
    target_column: String,
    penalty: f64,
    l1_ratio: f64,
    max_iter: usize,
    tol: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ElasticNetNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_elastic_net"
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
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_elastic_net".into(),
                msg: e.to_string(),
            })?;
        let target =
            common::extract_numeric_column(&batches, &self.target_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_elastic_net".into(),
                    msg: e.to_string(),
                }
            })?;
        let result = ml::regress::elastic_net(
            &data,
            &target,
            self.penalty,
            self.l1_ratio,
            self.max_iter,
            self.tol,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_elastic_net".into(),
            msg: e.to_string(),
        })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(result.predictions)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_elastic_net".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
