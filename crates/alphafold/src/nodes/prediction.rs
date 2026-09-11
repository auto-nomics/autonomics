use std::sync::Arc;

use arrow_array::{
    Array, BooleanArray, Float64Array, RecordBatch, StringArray, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::AlphaFoldClient;
use crate::types::Prediction;

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AlphaFoldPredictionSpec {
    /// UniProt accessions to fetch. Models for all accessions are unioned.
    pub accessions: Vec<String>,
}

#[derive(Clone)]
pub struct AlphaFoldPredictionNode {
    meta: NodePorts,
    spec: AlphaFoldPredictionSpec,
}

pub struct AlphaFoldPredictionNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for AlphaFoldPredictionNodeFactory {
    fn kind(&self) -> &'static str {
        "source_alphafold_prediction"
    }

    fn desc(&self) -> &'static str {
        "Fetch AlphaFold prediction metadata for UniProt accessions."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node that calls the AlphaFold prediction API and emits one \
        row per predicted model. Output columns include accession, gene, organism, \
        model version, global pLDDT metric, sequence range, and structure-file URLs."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AlphaFoldPredictionSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value::<AlphaFoldPredictionSpec>(spec)?;
        Ok(Box::new(AlphaFoldPredictionNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for AlphaFoldPredictionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_alphafold_prediction"
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
        let client = AlphaFoldClient::new();
        let mut rows = Vec::new();
        for accession in &self.spec.accessions {
            let predictions = client.prediction(accession).await.map_err(|error| {
                DagError::Schedule(format!("AlphaFold request failed: {error}"))
            })?;
            rows.extend(predictions);
        }

        let batch = record_batch(&rows)
            .map_err(|error| DagError::Schedule(format!("AlphaFold batch failed: {error}")))?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut output = PortOutputs::new();
        output.insert(0, dataframe);
        Ok(output)
    }
}

fn option_string(value: Option<&str>) -> Option<String> {
    value.map(str::to_owned)
}

fn record_batch(rows: &[Prediction]) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("entry_id", DataType::Utf8, true),
        Field::new("model_entity_id", DataType::Utf8, true),
        Field::new("uniprot_accession", DataType::Utf8, true),
        Field::new("uniprot_id", DataType::Utf8, true),
        Field::new("description", DataType::Utf8, true),
        Field::new("gene", DataType::Utf8, true),
        Field::new("organism", DataType::Utf8, true),
        Field::new("tax_id", DataType::UInt64, true),
        Field::new("latest_version", DataType::UInt32, true),
        Field::new("global_metric_value", DataType::Float64, true),
        Field::new("sequence", DataType::Utf8, true),
        Field::new("sequence_start", DataType::UInt64, true),
        Field::new("sequence_end", DataType::UInt64, true),
        Field::new("is_reviewed", DataType::Boolean, true),
        Field::new("cif_url", DataType::Utf8, true),
        Field::new("pdb_url", DataType::Utf8, true),
        Field::new("bcif_url", DataType::Utf8, true),
        Field::new("confidence_json_url", DataType::Utf8, true),
    ]));

    let columns = vec![
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.entry_id.as_deref()))
                .collect::<Vec<_>>(),
        )) as Arc<dyn Array>,
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.model_entity_id.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.uniprot_accession.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.uniprot_id.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.uniprot_description.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.gene.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.organism_scientific_name.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt64Array::from(
            rows.iter().map(|r| r.tax_id).collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|r| r.latest_version).collect::<Vec<_>>(),
        )),
        Arc::new(Float64Array::from(
            rows.iter()
                .map(|r| r.global_metric_value)
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.sequence.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt64Array::from(
            rows.iter().map(|r| r.sequence_start).collect::<Vec<_>>(),
        )),
        Arc::new(UInt64Array::from(
            rows.iter().map(|r| r.sequence_end).collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter().map(|r| r.is_reviewed).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.cif_url.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.pdb_url.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.bcif_url.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| option_string(r.plddt_doc_url.as_deref()))
                .collect::<Vec<_>>(),
        )),
    ];
    RecordBatch::try_new(schema, columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_registers_schema_and_kind() {
        let factory = AlphaFoldPredictionNodeFactory;
        assert_eq!(factory.kind(), "source_alphafold_prediction");
        let schema = serde_json::to_string(&factory.spec_schema()).unwrap();
        assert!(schema.contains("accessions"));
    }
}
