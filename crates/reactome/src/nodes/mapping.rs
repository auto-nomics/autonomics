//! `source_reactome_mapping` — map an external identifier to pathways.

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::ReactomeClient;
use crate::types::MappedPathway;
use super::pathways::str_array;

/// Spec for [`ReactomeMappingNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct ReactomeMappingSpec {
    /// External database name: `UniProt`, `Ensembl`, `EntrezGene`, `ChEBI`.
    #[serde(default)]
    pub resource: String,

    /// External identifier, e.g. `"P04637"` (UniProt TP53).
    #[serde(default)]
    pub identifier: String,

    /// Set to `true` to map to reactions instead of pathways.
    #[serde(default)]
    pub to_reactions: Option<bool>,
}

#[derive(Clone)]
pub struct ReactomeMappingNode {
    meta: NodePorts,
    spec: ReactomeMappingSpec,
}

pub struct ReactomeMappingNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for ReactomeMappingNodeFactory {
    fn kind(&self) -> &'static str {
        "source_reactome_mapping"
    }

    fn desc(&self) -> &'static str {
        "Map an external identifier to Reactome pathways or reactions."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the Reactome Content Service \
        `/data/mapping/{resource}/{identifier}/pathways` endpoint and emits \
        the mapped pathways as a DataFrame.\n\n\
        Common `resource` values: UniProt, Ensembl, EntrezGene, ChEBI.\n\n\
        Output schema: `db_id, stable_id, display_name, schema_class`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ReactomeMappingSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: ReactomeMappingSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ReactomeMappingNode {
            meta: port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for ReactomeMappingNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_reactome_mapping"
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
        if self.spec.resource.trim().is_empty() || self.spec.identifier.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_reactome_mapping: `resource` and `identifier` are required".into(),
            ));
        }

        let client = ReactomeClient::new();
        let pathways = if self.spec.to_reactions.unwrap_or(false) {
            client
                .map_to_reactions(&self.spec.resource, &self.spec.identifier)
                .await
        } else {
            client
                .map_to_pathways(&self.spec.resource, &self.spec.identifier)
                .await
        }
        .map_err(|e| DagError::Schedule(format!("Reactome mapping request failed: {e}")))?;

        let session = ctx.session();
        let batch = build_mapping_batch(&pathways)?;
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read Reactome mapping batch: {e}")))?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

pub fn build_mapping_batch(rows: &[MappedPathway]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("db_id", DataType::UInt64, true),
        Field::new("stable_id", DataType::Utf8, true),
        Field::new("display_name", DataType::Utf8, true),
        Field::new("schema_class", DataType::Utf8, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(arrow_array::UInt64Array::from(
                rows.iter().map(|p| p.db_id).collect::<Vec<_>>(),
            )),
            str_array(rows.iter().map(|p| p.stable_id.clone()).collect()),
            str_array(rows.iter().map(|p| Some(p.display_name.clone())).collect()),
            str_array(rows.iter().map(|p| p.schema_class.clone()).collect()),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build Reactome mapping batch: {e}")))
}
