use super::*;

// ═══════════════════════════════════════════════════════════════════════
// SVM Classifier
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SvmSpec {
    pub features: Vec<String>,
    pub label_column: String,
    #[serde(default = "d_svm_kernel")]
    pub kernel: String,
    #[serde(default = "d_svm_c")]
    pub c: f64,
}
fn d_svm_kernel() -> String {
    "rbf".into()
}
fn d_svm_c() -> f64 {
    1.0
}

pub struct SvmFactory;
impl NodeFactory for SvmFactory {
    fn kind(&self) -> &'static str {
        "ml_svm"
    }
    fn desc(&self) -> &'static str {
        "Support Vector Machine classifier."
    }
    fn doc(&self) -> &'static str {
        "SVM: binary classification via SMO solver. Supports linear, RBF, and polynomial kernels."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SvmSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: SvmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SvmNode {
            features: s.features,
            label_column: s.label_column,
            kernel: s.kernel,
            c: s.c,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct SvmNode {
    features: Vec<String>,
    label_column: String,
    kernel: String,
    c: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for SvmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_svm"
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
                node_type: "ml_svm".into(),
                msg: e.to_string(),
            })?;
        let labels_f =
            common::extract_numeric_column(&batches, &self.label_column).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_svm".into(),
                    msg: e.to_string(),
                }
            })?;
        let labels: Vec<usize> = labels_f.into_iter().map(|v| v as usize).collect();
        let result =
            ml::svm_ensemble::svm_classify(&data, &labels, &self.kernel, self.c).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_svm".into(),
                    msg: e.to_string(),
                }
            })?;
        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        fields.push(Arc::new(Field::new(
            "probability",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.probabilities)));
        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_svm".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
