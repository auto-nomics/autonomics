//! `source_reactome_participants` — fetch participants of a pathway.

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::pathways::str_array;
use crate::ReactomeClient;
use crate::types::Participant;

/// Spec for [`ReactomeParticipantsNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct ReactomeParticipantsSpec {
    /// Reactome stable ID (e.g. `"R-HSA-1640170"`) or numeric dbId.
    #[serde(default)]
    pub id: String,
}

#[derive(Clone)]
pub struct ReactomeParticipantsNode {
    meta: NodePorts,
    spec: ReactomeParticipantsSpec,
}

pub struct ReactomeParticipantsNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for ReactomeParticipantsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_reactome_participants"
    }

    fn desc(&self) -> &'static str {
        "Fetch physical entity participants of a Reactome pathway or reaction."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the Reactome Content Service \
        `/data/participants/{id}` endpoint and emits the participants as a \
        DataFrame with cross-referenced identifiers.\n\n\
        Output schema: `db_id, display_name, stable_id, schema_class, \
        reference_identifier, reference_database`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ReactomeParticipantsSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: ReactomeParticipantsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ReactomeParticipantsNode {
            meta: port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for ReactomeParticipantsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_reactome_participants"
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
        if self.spec.id.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_reactome_participants: `id` is required".into(),
            ));
        }

        let client = ReactomeClient::new();
        let participants = client.participants(&self.spec.id).await.map_err(|e| {
            DagError::Schedule(format!("Reactome participants request failed: {e}"))
        })?;

        let session = ctx.session();
        let batch = build_participants_batch(&participants)?;
        let df = session.read_batch(batch).map_err(|e| {
            DagError::Schedule(format!("failed to read Reactome participants batch: {e}"))
        })?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

pub fn build_participants_batch(rows: &[Participant]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("db_id", DataType::UInt64, true),
        Field::new("display_name", DataType::Utf8, true),
        Field::new("stable_id", DataType::Utf8, true),
        Field::new("schema_class", DataType::Utf8, true),
        Field::new("reference_identifier", DataType::Utf8, true),
        Field::new("reference_database", DataType::Utf8, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(UInt64Array::from(
                rows.iter().map(|p| p.db_id).collect::<Vec<_>>(),
            )),
            str_array(rows.iter().map(|p| p.display_name.clone()).collect()),
            str_array(rows.iter().map(|p| p.stable_id.clone()).collect()),
            str_array(rows.iter().map(|p| p.schema_class.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|p| {
                        p.reference_entity
                            .as_ref()
                            .and_then(|r| r.identifier.clone())
                    })
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|p| {
                        p.reference_entity
                            .as_ref()
                            .and_then(|r| r.database_name.clone())
                    })
                    .collect(),
            ),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build Reactome participants batch: {e}")))
}
