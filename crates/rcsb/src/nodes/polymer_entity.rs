use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::RcsbClient;
use crate::types::{PolymerEntity, PolymerEntityRecord};

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct RcsbPolymerEntitySpec {
    /// Entry IDs to inspect. When `entity_ids` is empty, every polymer entity
    /// in each entry is fetched.
    #[serde(default)]
    pub entry_ids: Vec<String>,

    /// Optional entity IDs. If present, each ID is fetched for every entry.
    /// Omit it to use each entry's complete polymer-entity list.
    #[serde(default)]
    pub entity_ids: Option<Vec<u32>>,
}

#[derive(Clone)]
pub struct RcsbPolymerEntityNode {
    meta: NodePorts,
    spec: RcsbPolymerEntitySpec,
}

pub struct RcsbPolymerEntityNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for RcsbPolymerEntityNodeFactory {
    fn kind(&self) -> &'static str {
        "source_rcsb_polymer_entity"
    }

    fn desc(&self) -> &'static str {
        "Fetch RCSB polymer entities, sequences, and cross-references."
    }

    fn doc(&self) -> &'static str {
        "Calls the RCSB Data API and emits one row per polymer entity with \
        entry/entity identifiers, description, canonical sequence, length, \
        polymer type, copy count, strand IDs, UniProt mappings, genes, and \
        source organism. The output is directly usable by sequence and SQL \
        nodes."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RcsbPolymerEntitySpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(RcsbPolymerEntityNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for RcsbPolymerEntityNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_rcsb_polymer_entity"
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
        if self.spec.entry_ids.is_empty() {
            return Err(DagError::Schedule(
                "source_rcsb_polymer_entity requires `entry_ids`".into(),
            ));
        }

        let client = RcsbClient::new();
        let entities = match self.spec.entity_ids.as_deref() {
            Some([]) | None => {
                let mut entities = Vec::new();
                for entry_id in &self.spec.entry_ids {
                    entities.extend(client.polymer_entities_for_entry(entry_id).await.map_err(
                        |e| DagError::Schedule(format!("RCSB polymer request failed: {e}")),
                    )?);
                }
                entities
            }
            Some(entity_ids) => {
                let mut entities = Vec::new();
                for entry_id in &self.spec.entry_ids {
                    for entity_id in entity_ids {
                        entities.push(client.polymer_entity(entry_id, *entity_id).await.map_err(
                            |e| DagError::Schedule(format!("RCSB polymer request failed: {e}")),
                        )?);
                    }
                }
                entities
            }
        };

        let records = entities
            .iter()
            .map(PolymerEntity::to_record)
            .collect::<Vec<_>>();
        let batch = build_polymer_batch(&records)?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read RCSB polymer batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

fn build_polymer_batch(records: &[PolymerEntityRecord]) -> Result<RecordBatch, DagError> {
    fn required(
        rows: &[PolymerEntityRecord],
        get: fn(&PolymerEntityRecord) -> String,
    ) -> Vec<Option<String>> {
        rows.iter().map(get).map(Some).collect()
    }

    fn optional(
        rows: &[PolymerEntityRecord],
        get: fn(&PolymerEntityRecord) -> &Option<String>,
    ) -> Vec<Option<String>> {
        rows.iter().map(get).cloned().collect()
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("entry_id", DataType::Utf8, false),
        Field::new("entity_id", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, false),
        Field::new("sequence", DataType::Utf8, false),
        Field::new("sequence_length", DataType::UInt64, false),
        Field::new("polymer_type", DataType::Utf8, true),
        Field::new("copies", DataType::UInt64, true),
        Field::new("formula_weight", DataType::Float64, true),
        Field::new("strand_ids", DataType::Utf8, true),
        Field::new("uniprot_ids", DataType::Utf8, false),
        Field::new("gene_names", DataType::Utf8, false),
        Field::new("organism_scientific", DataType::Utf8, true),
        Field::new("taxonomy_id", DataType::UInt64, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::util::str_array(required(records, |r| r.entry_id.clone())),
            crate::nodes::util::str_array(required(records, |r| r.entity_id.clone())),
            crate::nodes::util::str_array(required(records, |r| r.description.clone())),
            crate::nodes::util::str_array(required(records, |r| r.sequence.clone())),
            Arc::new(UInt64Array::from(
                records
                    .iter()
                    .map(|r| r.sequence_length)
                    .collect::<Vec<_>>(),
            )),
            crate::nodes::util::str_array(optional(records, |r| &r.polymer_type)),
            Arc::new(UInt64Array::from(
                records.iter().map(|r| r.copies).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                records.iter().map(|r| r.formula_weight).collect::<Vec<_>>(),
            )),
            crate::nodes::util::str_array(optional(records, |r| &r.strand_ids)),
            crate::nodes::util::str_array(required(records, |r| r.uniprot_ids.clone())),
            crate::nodes::util::str_array(required(records, |r| r.gene_names.clone())),
            crate::nodes::util::str_array(optional(records, |r| &r.organism_scientific)),
            Arc::new(UInt64Array::from(
                records.iter().map(|r| r.taxonomy_id).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build RCSB polymer batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polymer_record_batch_contains_expected_columns() {
        let record = PolymerEntityRecord {
            entry_id: "4HHB".into(),
            entity_id: "1".into(),
            description: "Hemoglobin subunit alpha".into(),
            sequence: "VLSPADKTNVK".into(),
            sequence_length: 11,
            polymer_type: Some("Protein".into()),
            copies: Some(2),
            formula_weight: Some(15.15),
            strand_ids: Some("A,C".into()),
            uniprot_ids: "P69905".into(),
            gene_names: "HBA1,HBA2".into(),
            organism_scientific: Some("Homo sapiens".into()),
            taxonomy_id: Some(9606),
        };
        let batch = build_polymer_batch(&[record]).unwrap();
        assert_eq!(batch.num_columns(), 13);
        assert_eq!(batch.num_rows(), 1);
    }
}
