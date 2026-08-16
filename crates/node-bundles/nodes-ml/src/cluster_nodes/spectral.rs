//! Spectral clustering node placeholder.

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::NodePorts;
use dag_core::registry::{NodeCtx, NodeFactory};

pub struct SpectralClusteringFactory;

impl NodeFactory for SpectralClusteringFactory {
    fn kind(&self) -> &'static str {
        "ml_spectral_cluster"
    }

    fn desc(&self) -> &'static str {
        "Spectral clustering (normalised Laplacian + K-means)."
    }

    fn doc(&self) -> &'static str {
        "SpectralClustering: builds affinity matrix, embeds via normalised Laplacian eigenvectors, runs K-means in embedding space. [Coming soon]"
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SpectralSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn dag_core::node::DagNode>, dag_core::registry::error::Error> {
        let _s: SpectralSpec = serde_json::from_value(spec)?;
        Err(dag_core::registry::error::Error::Unknown(
            "ml_spectral_cluster is not yet implemented".into(),
        ))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[allow(dead_code)] // Schema fields are accepted even though the node is not implemented.
struct SpectralSpec {
    features: Vec<String>,
    k: usize,
}
