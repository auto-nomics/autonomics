use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::PubChemClient;
use crate::types::{CompoundIdentifierType, CompoundProperties};

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct PubChemCompoundSpec {
    /// Identifier type: `name`, `cid`, or `inchikey`.
    #[serde(default = "default_identifier_type")]
    pub identifier_type: String,

    /// Compound identifier.
    pub identifier: String,
}

fn default_identifier_type() -> String {
    "name".to_owned()
}

#[derive(Clone)]
pub struct PubChemCompoundNode {
    meta: NodePorts,
    spec: PubChemCompoundSpec,
}

pub struct PubChemCompoundNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for PubChemCompoundNodeFactory {
    fn kind(&self) -> &'static str {
        "source_pubchem_compound"
    }

    fn desc(&self) -> &'static str {
        "Fetch compound metadata from PubChem as a table."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node that resolves one PubChem compound. Output columns: cid, \
        query, query_type, molecular_formula, molecular_weight, smiles, inchi, inchikey, \
        iupac_name, title."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PubChemCompoundSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value::<PubChemCompoundSpec>(spec)?;
        Ok(Box::new(PubChemCompoundNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for PubChemCompoundNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_pubchem_compound"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let identifier_type = CompoundIdentifierType::parse(&self.spec.identifier_type)
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "unsupported identifier_type {}",
                    self.spec.identifier_type
                ))
            })?;
        let compound = PubChemClient::new()
            .compound(identifier_type, &self.spec.identifier)
            .await
            .map_err(|error| DagError::Schedule(format!("PubChem request failed: {error}")))?
            .unwrap_or_default();
        let batch = record_batch(&compound, &self.spec.identifier, identifier_type)
            .map_err(|error| DagError::Schedule(format!("PubChem batch failed: {error}")))?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut output = PortOutputs::new();
        output.insert(0, dataframe);
        Ok(output)
    }
}

fn record_batch(
    compound: &CompoundProperties,
    query: &str,
    query_type: CompoundIdentifierType,
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("cid", DataType::UInt64, true),
        Field::new("query", DataType::Utf8, true),
        Field::new("query_type", DataType::Utf8, true),
        Field::new("molecular_formula", DataType::Utf8, true),
        Field::new("molecular_weight", DataType::Utf8, true),
        Field::new("smiles", DataType::Utf8, true),
        Field::new("inchi", DataType::Utf8, true),
        Field::new("inchikey", DataType::Utf8, true),
        Field::new("iupac_name", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
    ]));
    let smiles = compound
        .smiles
        .as_ref()
        .or(compound.connectivity_smiles.as_ref())
        .or(compound.canonical_smiles.as_ref());
    let columns = vec![
        Arc::new(UInt64Array::from(vec![compound.cid])) as Arc<dyn Array>,
        Arc::new(StringArray::from(vec![Some(query)])),
        Arc::new(StringArray::from(vec![Some(query_type.as_str())])),
        Arc::new(StringArray::from(vec![
            compound.molecular_formula.as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            compound.molecular_weight.as_deref(),
        ])),
        Arc::new(StringArray::from(vec![smiles.map(String::as_str)])),
        Arc::new(StringArray::from(vec![compound.inchi.as_deref()])),
        Arc::new(StringArray::from(vec![compound.inchikey.as_deref()])),
        Arc::new(StringArray::from(vec![compound.iupac_name.as_deref()])),
        Arc::new(StringArray::from(vec![compound.title.as_deref()])),
    ];
    RecordBatch::try_new(schema, columns)
}
