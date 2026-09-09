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
use crate::types::{Entry, EntryRecord};

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct RcsbEntrySpec {
    /// Four-character PDB entry IDs, e.g. `["4HHB", "2HHB"]`.
    #[serde(default)]
    pub entry_ids: Vec<String>,
}

#[derive(Clone)]
pub struct RcsbEntryNode {
    meta: NodePorts,
    spec: RcsbEntrySpec,
}

pub struct RcsbEntryNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for RcsbEntryNodeFactory {
    fn kind(&self) -> &'static str {
        "source_rcsb_entry"
    }

    fn desc(&self) -> &'static str {
        "Fetch RCSB PDB entry metadata as a structured table."
    }

    fn doc(&self) -> &'static str {
        "Calls the RCSB Data API once per requested entry and emits a flat \
        Arrow/DataFrame row with experimental method, resolution, atom and \
        entity counts, dates, identifiers, and primary-citation metadata. \
        No input ports; one DataFrame output."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RcsbEntrySpec)
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
        Ok(Box::new(RcsbEntryNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for RcsbEntryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_rcsb_entry"
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
                "source_rcsb_entry requires at least one `entry_ids` item".into(),
            ));
        }

        let entries = RcsbClient::new()
            .entries(&self.spec.entry_ids)
            .await
            .map_err(|e| DagError::Schedule(format!("RCSB entry request failed: {e}")))?;
        let records = entries.iter().map(Entry::to_record).collect::<Vec<_>>();
        let batch = build_entry_batch(&records)?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read RCSB entry batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

pub(crate) fn build_entry_batch(records: &[EntryRecord]) -> Result<RecordBatch, DagError> {
    fn strings(rows: &[EntryRecord], get: fn(&EntryRecord) -> String) -> Vec<Option<String>> {
        rows.iter().map(get).map(Some).collect()
    }

    fn optional_strings(
        rows: &[EntryRecord],
        get: fn(&EntryRecord) -> &Option<String>,
    ) -> Vec<Option<String>> {
        rows.iter().map(get).cloned().collect()
    }

    fn optional_u64(
        rows: &[EntryRecord],
        get: fn(&EntryRecord) -> Option<u64>,
    ) -> Vec<Option<u64>> {
        rows.iter().map(get).collect()
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("entry_id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("experimental_method", DataType::Utf8, false),
        Field::new("resolution", DataType::Float64, true),
        Field::new("deposited_atom_count", DataType::UInt64, true),
        Field::new("deposited_model_count", DataType::UInt64, true),
        Field::new("assembly_count", DataType::UInt64, true),
        Field::new("polymer_entity_count", DataType::UInt64, true),
        Field::new("protein_entity_count", DataType::UInt64, true),
        Field::new("molecular_weight", DataType::Float64, true),
        Field::new("polymer_composition", DataType::Utf8, true),
        Field::new("space_group", DataType::Utf8, true),
        Field::new("deposit_date", DataType::Utf8, true),
        Field::new("release_date", DataType::Utf8, true),
        Field::new("doi", DataType::Utf8, true),
        Field::new("pubmed_id", DataType::UInt64, true),
        Field::new("authors", DataType::Utf8, false),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::util::str_array(strings(records, |r| r.entry_id.clone())),
            crate::nodes::util::str_array(strings(records, |r| r.title.clone())),
            crate::nodes::util::str_array(strings(records, |r| r.experimental_method.clone())),
            Arc::new(Float64Array::from(
                records.iter().map(|r| r.resolution).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.deposited_atom_count
            }))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.deposited_model_count
            }))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.assembly_count
            }))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.polymer_entity_count
            }))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.protein_entity_count
            }))),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|r| r.molecular_weight)
                    .collect::<Vec<_>>(),
            )),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.polymer_composition)),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.space_group)),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.deposit_date)),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.release_date)),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.doi)),
            Arc::new(UInt64Array::from(optional_u64(records, |r| r.pubmed_id))),
            crate::nodes::util::str_array(strings(records, |r| r.authors.clone())),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build RCSB entry batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_record_batch_contains_expected_columns() {
        let record = EntryRecord {
            entry_id: "4HHB".into(),
            title: "Hemoglobin".into(),
            experimental_method: "X-RAY DIFFRACTION".into(),
            resolution: Some(1.74),
            deposited_atom_count: Some(4779),
            deposited_model_count: Some(1),
            assembly_count: Some(1),
            polymer_entity_count: Some(2),
            protein_entity_count: Some(2),
            molecular_weight: Some(64.74),
            polymer_composition: Some("heteromeric protein".into()),
            space_group: Some("P 1 21 1".into()),
            deposit_date: Some("1984-03-07".into()),
            release_date: Some("1984-07-17".into()),
            doi: Some("10.1/example".into()),
            pubmed_id: Some(6726807),
            authors: "Fermi, G.".into(),
        };
        let batch = build_entry_batch(&[record]).unwrap();
        assert_eq!(batch.num_columns(), 17);
        assert_eq!(batch.num_rows(), 1);
    }
}
