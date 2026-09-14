use std::sync::Arc;

use arrow_array::{Array, BooleanArray, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::InterProClient;
use crate::types::EntryMetadata;

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct InterProEntrySpec {
    /// InterPro accession, for example `IPR017861`.
    pub accession: String,
}

#[derive(Clone)]
pub struct InterProEntryNode {
    meta: NodePorts,
    spec: InterProEntrySpec,
}

pub struct InterProEntryNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for InterProEntryNodeFactory {
    fn kind(&self) -> &'static str {
        "source_interpro_entry"
    }

    fn desc(&self) -> &'static str {
        "Fetch one InterPro entry and emit its metadata as a table."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node that calls the InterPro entry endpoint. Output columns: \
        accession, entry_id, name, short_name, entry_type, source_database, description, \
        go_terms, member_databases, integrated_accession, proteins, structures, \
        alphafold_models, representative_structure, is_llm."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(InterProEntrySpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value::<InterProEntrySpec>(spec)?;
        Ok(Box::new(InterProEntryNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for InterProEntryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_interpro_entry"
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
        let response = InterProClient::new()
            .entry(&self.spec.accession)
            .await
            .map_err(|error| DagError::Schedule(format!("InterPro request failed: {error}")))?;
        let batch = record_batch(&response.metadata)
            .map_err(|error| DagError::Schedule(format!("InterPro batch failed: {error}")))?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut output = PortOutputs::new();
        output.insert(0, dataframe);
        Ok(output)
    }
}

fn record_batch(metadata: &EntryMetadata) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("accession", DataType::Utf8, true),
        Field::new("entry_id", DataType::Utf8, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("short_name", DataType::Utf8, true),
        Field::new("entry_type", DataType::Utf8, true),
        Field::new("source_database", DataType::Utf8, true),
        Field::new("description", DataType::Utf8, true),
        Field::new("go_terms", DataType::Utf8, true),
        Field::new("member_databases", DataType::Utf8, true),
        Field::new("integrated_accession", DataType::Utf8, true),
        Field::new("proteins", DataType::UInt64, true),
        Field::new("structures", DataType::UInt64, true),
        Field::new("alphafold_models", DataType::UInt64, true),
        Field::new("representative_structure", DataType::Utf8, true),
        Field::new("is_llm", DataType::Boolean, true),
    ]));

    let description = metadata
        .description
        .iter()
        .filter_map(|item| item.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let go_terms = metadata
        .go_terms
        .iter()
        .map(|term| {
            term.category
                .as_ref()
                .map(|category| format!("{}:{}:{}", term.identifier, term.name, category))
                .unwrap_or_else(|| format!("{}:{}", term.identifier, term.name))
        })
        .collect::<Vec<_>>()
        .join(";");
    let member_databases = metadata
        .member_databases
        .iter()
        .flat_map(|(database, entries)| {
            entries
                .iter()
                .map(move |(accession, name)| format!("{database}:{accession}:{name}"))
        })
        .collect::<Vec<_>>()
        .join(";");

    let columns = vec![
        Arc::new(StringArray::from(vec![Some(metadata.accession.as_str())])) as Arc<dyn Array>,
        Arc::new(StringArray::from(vec![metadata.entry_id.as_deref()])),
        Arc::new(StringArray::from(vec![
            metadata.name.as_ref().map(|name| name.name.as_str()),
        ])),
        Arc::new(StringArray::from(vec![
            metadata
                .name
                .as_ref()
                .and_then(|name| name.short.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![metadata.entry_type.as_deref()])),
        Arc::new(StringArray::from(vec![metadata.source_database.as_deref()])),
        Arc::new(StringArray::from(vec![Some(description.as_str())])),
        Arc::new(StringArray::from(vec![Some(go_terms.as_str())])),
        Arc::new(StringArray::from(vec![Some(member_databases.as_str())])),
        Arc::new(StringArray::from(vec![
            metadata
                .integrated
                .as_ref()
                .map(|entry| entry.accession.as_str()),
        ])),
        Arc::new(UInt64Array::from(vec![
            metadata
                .counters
                .as_ref()
                .and_then(|counters| counters.proteins),
        ])),
        Arc::new(UInt64Array::from(vec![
            metadata
                .counters
                .as_ref()
                .and_then(|counters| counters.structures),
        ])),
        Arc::new(UInt64Array::from(vec![
            metadata.counters.as_ref().and_then(|counters| {
                counters
                    .structural_models
                    .as_ref()
                    .and_then(|models| models.alphafold)
            }),
        ])),
        Arc::new(StringArray::from(vec![
            metadata
                .representative_structure
                .as_ref()
                .map(|structure| structure.accession.as_str()),
        ])),
        Arc::new(BooleanArray::from(vec![metadata.is_llm])),
    ];
    RecordBatch::try_new(schema, columns)
}
