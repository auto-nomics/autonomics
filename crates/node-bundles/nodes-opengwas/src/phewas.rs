//! `opengwas_phewas` — PheWAS hits across all GWAS datasets from
//! OpenGWAS `/phewas`, with the variant list read from an upstream
//! DataFrame column instead of being embedded in the node spec.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use opengwas::types::PhewasRequest;

use crate::shared::{extract_rows, json_to_output, make_client, string_column_values};

const KIND_PHEWAS: &str = "opengwas_phewas";

fn default_phewas_pval() -> f64 {
    0.01
}

fn default_variant_column() -> String {
    "rsid".into()
}

fn default_batch_size() -> usize {
    100
}

/// Spec for [`OpengwasPhewasNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasPhewasSpec {
    /// Name of the input DataFrame column holding variant identifiers
    /// (rsID, chr:pos, or chr:pos range on hg19/b37). Default `rsid`.
    #[serde(default = "default_variant_column")]
    pub variant_column: String,
    /// P-value threshold (must ≤ 0.01). Default `0.01`.
    #[serde(default = "default_phewas_pval")]
    pub pval: f64,
    /// Restrict to specific study indexes. If empty, searches all.
    #[serde(default)]
    pub index_list: Option<Vec<String>>,
    /// Variants per `/phewas` request; large inputs are queried in chunks
    /// of this size and the hits merged. Default `100`.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

/// PheWAS node: significant trait associations from OpenGWAS `/phewas`,
/// variants taken from an upstream DataFrame.
#[derive(Clone)]
pub struct OpengwasPhewasNode {
    meta: NodePorts,
    spec: OpengwasPhewasSpec,
}

pub struct OpengwasPhewasNodeFactory;

impl NodeFactory for OpengwasPhewasNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_PHEWAS
    }
    fn desc(&self) -> &'static str {
        "Performs PheWAS across all GWAS datasets via OpenGWAS /phewas, \
        reading variant IDs from an upstream DataFrame column."
    }
    fn doc(&self) -> &'static str {
        "Takes a DataFrame with a variant-ID column (rsID, chr:pos or chr:pos \
        range on hg19/b37 — `variant_column`, default `rsid`) and scans those \
        variants across all available GWAS datasets via the OpenGWAS \
        `/phewas` endpoint, returning significant associations (p ≤ \
        threshold). Large variant lists are queried in `batch_size` chunks \
        and the hits merged.\n\n\
        Output schema is inferred — typically `id, rsid, chr, position, ea, \
        nea, beta, se, pval, n, trait`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasPhewasSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port_of_type_with_label(None, PortType::DataFrame, "variants")
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasPhewasSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasPhewasNode {
            meta: self.ports(),
            spec: s,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasPhewasNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_PHEWAS
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let err = |msg: String| DagError::NodeError {
            node_type: KIND_PHEWAS.into(),
            msg,
        };

        let input = inputs.first().ok_or_else(|| {
            err("no input: connect a DataFrame with variant IDs to the 'variants' port".into())
        })?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| err(format!("failed to collect input DataFrame: {e}")))?;

        let variants = string_column_values(KIND_PHEWAS, &batches, &self.spec.variant_column)?;
        if variants.is_empty() {
            return Err(err(format!(
                "column '{}' contains no usable variant IDs",
                self.spec.variant_column
            )));
        }
        tracing::info!(
            "OpenGWAS /phewas: querying {} unique variants (threshold p ≤ {})",
            variants.len(),
            self.spec.pval
        );

        let client = make_client()?;
        let batch_size = self.spec.batch_size.max(1);
        let mut all_rows: Vec<Value> = Vec::new();
        for chunk in variants.chunks(batch_size) {
            let resp = client
                .phewas(&PhewasRequest {
                    variant: chunk.to_vec(),
                    pval: Some(self.spec.pval),
                    index_list: self.spec.index_list.clone(),
                    commercial_approval_received: None,
                })
                .await
                .map_err(|e| DagError::Schedule(format!("OpenGWAS /phewas failed: {e}")))?;
            all_rows.extend(extract_rows(&resp));
        }

        if all_rows.is_empty() {
            return Err(DagError::Schedule(format!(
                "OpenGWAS /phewas returned no associations at p ≤ {} for {} variants",
                self.spec.pval,
                variants.len()
            )));
        }
        json_to_output(node_ctx, &Value::Array(all_rows), "/phewas").await
    }
}
