use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodeInput, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::RcsbClient;
use crate::types::{Assembly, AssemblyRecord};

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct RcsbAssemblySpec {
    /// Four-character PDB entry ID used when no input table is connected.
    #[serde(default)]
    pub entry_id: String,

    /// Assembly IDs to fetch for `entry_id`. Defaults to `[1]`.
    #[serde(default)]
    pub assembly_ids: Option<Vec<u32>>,

    /// Input-table column containing assembly identifiers such as `4HHB-1`.
    /// Defaults to `identifier`.
    #[serde(default)]
    pub identifier_column: Option<String>,
}

#[derive(Clone)]
pub struct RcsbAssemblyNode {
    meta: NodePorts,
    spec: RcsbAssemblySpec,
}

pub struct RcsbAssemblyNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::DataFrame)
        .add_output_port(None)
}

impl NodeFactory for RcsbAssemblyNodeFactory {
    fn kind(&self) -> &'static str {
        "source_rcsb_assembly"
    }

    fn desc(&self) -> &'static str {
        "Fetch RCSB PDB assembly composition as a structured table."
    }

    fn doc(&self) -> &'static str {
        "Calls the RCSB Data API and emits one row per assembly with atom, \
        polymer, monomer, interface, molecular-weight, and buried-surface-area \
        metadata. One optional DataFrame input; one DataFrame output.\n\n\
        With no input, set `entry_id` and optionally `assembly_ids`. With an \
        input table, read identifiers such as `4HHB-1` from `identifier_column`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RcsbAssemblySpec)
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
        Ok(Box::new(RcsbAssemblyNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for RcsbAssemblyNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_rcsb_assembly"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let identifiers = match inputs.first() {
            Some(input) => {
                assembly_identifiers_from_column(
                    input.dataframe()?,
                    self.spec
                        .identifier_column
                        .as_deref()
                        .unwrap_or("identifier"),
                )
                .await?
            }
            None => {
                if self.spec.entry_id.trim().is_empty() {
                    return Err(DagError::Schedule(
                        "source_rcsb_assembly requires `entry_id` or an input table".into(),
                    ));
                }
                self.spec
                    .assembly_ids
                    .clone()
                    .unwrap_or_else(|| vec![1])
                    .into_iter()
                    .map(|assembly_id| (self.spec.entry_id.trim().to_owned(), assembly_id))
                    .collect::<Vec<_>>()
            }
        };
        if identifiers.is_empty() {
            return Err(DagError::Schedule(
                "source_rcsb_assembly: no assembly identifiers to fetch".into(),
            ));
        }

        let client = RcsbClient::new();
        let mut assemblies = Vec::with_capacity(identifiers.len());
        for (entry_id, assembly_id) in identifiers {
            assemblies.push(
                client.assembly(&entry_id, assembly_id).await.map_err(|e| {
                    DagError::Schedule(format!("RCSB assembly request failed: {e}"))
                })?,
            );
        }

        let records = assemblies
            .iter()
            .map(Assembly::to_record)
            .collect::<Vec<_>>();
        let batch = build_assembly_batch(&records)?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read RCSB assembly batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

async fn assembly_identifiers_from_column(
    dataframe: &DataFrame,
    column: &str,
) -> Result<Vec<(String, u32)>, DagError> {
    let index = dataframe
        .schema()
        .index_of_column_by_name(None, column)
        .ok_or_else(|| {
            DagError::Schedule(format!(
                "source_rcsb_assembly: input table has no column named {column:?}"
            ))
        })?;
    let batches = dataframe
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::Schedule(format!("failed to collect assembly IDs: {e}")))?;

    let mut identifiers = Vec::new();
    for batch in &batches {
        let array = batch
            .column(index)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "source_rcsb_assembly: column {column:?} must be a string column"
                ))
            })?;
        for row in 0..array.len() {
            if array.is_null(row) {
                continue;
            }
            let value = array.value(row).trim().to_ascii_uppercase();
            let Some((entry_id, assembly_id)) = value.split_once('-') else {
                return Err(DagError::Schedule(format!(
                    "source_rcsb_assembly: invalid assembly identifier {value:?}; \
                     expected ENTRY-ASSEMBLY, e.g. 4HHB-1"
                )));
            };
            let assembly_id = assembly_id.parse::<u32>().map_err(|_| {
                DagError::Schedule(format!(
                    "source_rcsb_assembly: invalid assembly number in {value:?}"
                ))
            })?;
            identifiers.push((entry_id.to_owned(), assembly_id));
        }
    }
    Ok(identifiers)
}

pub(crate) fn build_assembly_batch(records: &[AssemblyRecord]) -> Result<RecordBatch, DagError> {
    fn optional_strings(
        rows: &[AssemblyRecord],
        get: fn(&AssemblyRecord) -> &Option<String>,
    ) -> Vec<Option<String>> {
        rows.iter().map(get).cloned().collect()
    }

    fn optional_u64(
        rows: &[AssemblyRecord],
        get: fn(&AssemblyRecord) -> Option<u64>,
    ) -> Vec<Option<u64>> {
        rows.iter().map(get).collect()
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("entry_id", DataType::Utf8, true),
        Field::new("assembly_id", DataType::Utf8, false),
        Field::new("oligomeric_count", DataType::UInt64, true),
        Field::new("oligomeric_details", DataType::Utf8, true),
        Field::new("atom_count", DataType::UInt64, true),
        Field::new("polymer_entity_count", DataType::UInt64, true),
        Field::new("polymer_entity_instance_count", DataType::UInt64, true),
        Field::new("modeled_polymer_monomer_count", DataType::UInt64, true),
        Field::new("polymer_composition", DataType::Utf8, true),
        Field::new("formula_weight", DataType::Float64, true),
        Field::new("num_interfaces", DataType::UInt64, true),
        Field::new("buried_surface_area", DataType::Float64, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::util::str_array(optional_strings(records, |r| &r.entry_id)),
            crate::nodes::util::str_array(
                records
                    .iter()
                    .map(|record| Some(record.assembly_id.clone()))
                    .collect::<Vec<_>>(),
            ),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.oligomeric_count
            }))),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.oligomeric_details)),
            Arc::new(UInt64Array::from(optional_u64(records, |r| r.atom_count))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.polymer_entity_count
            }))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.polymer_entity_instance_count
            }))),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.modeled_polymer_monomer_count
            }))),
            crate::nodes::util::str_array(optional_strings(records, |r| &r.polymer_composition)),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|record| record.formula_weight)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(optional_u64(records, |r| {
                r.num_interfaces
            }))),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|record| record.buried_surface_area)
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build RCSB assembly batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_record_batch_contains_computation_columns() {
        let record = AssemblyRecord {
            assembly_id: "1".into(),
            entry_id: Some("4HHB".into()),
            oligomeric_count: Some(4),
            oligomeric_details: Some("tetrameric".into()),
            atom_count: Some(4779),
            polymer_entity_count: Some(2),
            polymer_entity_instance_count: Some(4),
            modeled_polymer_monomer_count: Some(574),
            polymer_composition: Some("heteromeric protein".into()),
            formula_weight: Some(64.74),
            num_interfaces: Some(5),
            buried_surface_area: Some(3305.349),
        };
        let batch = build_assembly_batch(&[record]).unwrap();
        assert_eq!(batch.num_columns(), 12);
        assert_eq!(batch.num_rows(), 1);
    }
}
