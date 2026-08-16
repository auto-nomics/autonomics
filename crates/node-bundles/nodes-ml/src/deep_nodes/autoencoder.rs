use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Autoencoder
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AutoencoderSpec {
    pub features: Vec<String>,
    #[serde(default = "d_latent")]
    pub latent_dim: usize,
    #[serde(default = "d_ae_epochs")]
    pub n_epochs: usize,
    #[serde(default = "d_ae_lr")]
    pub learning_rate: f64,
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_latent() -> usize {
    2
}
fn d_ae_epochs() -> usize {
    100
}
fn d_ae_lr() -> f64 {
    0.01
}
fn d_seed() -> u64 {
    42
}

pub struct AutoencoderFactory;
impl NodeFactory for AutoencoderFactory {
    fn kind(&self) -> &'static str {
        "ml_autoencoder"
    }
    fn desc(&self) -> &'static str {
        "Linear autoencoder for dimensionality reduction."
    }
    fn doc(&self) -> &'static str {
        "Autoencoder: learns a compressed latent representation by training encoder-decoder weight matrices to reconstruct input. Outputs ae_0, ae_1, ... latent coordinates."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AutoencoderSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: AutoencoderSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AutoencoderNode {
            features: s.features,
            latent_dim: s.latent_dim,
            n_epochs: s.n_epochs,
            learning_rate: s.learning_rate,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct AutoencoderNode {
    features: Vec<String>,
    latent_dim: usize,
    n_epochs: usize,
    learning_rate: f64,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for AutoencoderNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_autoencoder"
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
                node_type: "ml_autoencoder".into(),
                msg: e.to_string(),
            })?;
        let result = ml::deep::autoencoder(
            &data,
            self.latent_dim,
            self.n_epochs,
            self.learning_rate,
            self.seed,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_autoencoder".into(),
            msg: e.to_string(),
        })?;

        let (_schema, mut fields, mut arrays) = common::concat_input(&batches)?;
        for d in 0..self.latent_dim {
            let col: Vec<f64> = result.encoded.iter().map(|row| row[d]).collect();
            fields.push(Arc::new(Field::new(
                format!("ae_{d}"),
                DataType::Float64,
                true,
            )));
            arrays.push(Arc::new(Float64Array::from(col)));
        }
        // Add reconstruction error as a constant column
        let err_col = vec![result.reconstruction_error; result.encoded.len()];
        fields.push(Arc::new(Field::new(
            "reconstruction_error",
            DataType::Float64,
            true,
        )));
        arrays.push(Arc::new(Float64Array::from(err_col)));

        emit_batch(
            ctx,
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_autoencoder".into(),
                    msg: e.to_string(),
                }
            })?,
        )
    }
}
