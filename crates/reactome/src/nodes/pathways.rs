//! `source_reactome_pathways` — fetch top-level pathways for a species.

use std::sync::Arc;

use arrow_array::{Array, BooleanArray, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::ReactomeClient;
use crate::types::Pathway;

/// Spec for [`ReactomePathwaysNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct ReactomePathwaysSpec {
    /// Species name, e.g. `"Homo sapiens"`.
    #[serde(default)]
    pub species: String,
}

#[derive(Clone)]
pub struct ReactomePathwaysNode {
    meta: NodePorts,
    spec: ReactomePathwaysSpec,
}

pub struct ReactomePathwaysNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for ReactomePathwaysNodeFactory {
    fn kind(&self) -> &'static str {
        "source_reactome_pathways"
    }

    fn desc(&self) -> &'static str {
        "Fetch top-level Reactome pathways for a species."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the Reactome Content Service \
        `/data/pathways/top/{species}` endpoint and emits the top-level \
        pathways as a DataFrame.\n\n\
        Output schema: `db_id, stable_id, display_name, schema_class, \
        species_name, is_in_disease, is_inferred, has_diagram, has_ehld`.\n\n\
        Pipe into `sql_node` for filtering or `dataframe_to_file` for export."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ReactomePathwaysSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: ReactomePathwaysSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ReactomePathwaysNode {
            meta: port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for ReactomePathwaysNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_reactome_pathways"
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
        if self.spec.species.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_reactome_pathways: `species` is required".into(),
            ));
        }

        let client = ReactomeClient::new();
        let pathways = client
            .top_level_pathways(&self.spec.species)
            .await
            .map_err(|e| DagError::Schedule(format!("Reactome pathways request failed: {e}")))?;

        let session = ctx.session();
        let batch = build_pathways_batch(&pathways)?;
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read Reactome pathways batch: {e}")))?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

pub fn build_pathways_batch(rows: &[Pathway]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("db_id", DataType::UInt64, true),
        Field::new("stable_id", DataType::Utf8, true),
        Field::new("display_name", DataType::Utf8, true),
        Field::new("schema_class", DataType::Utf8, true),
        Field::new("species_name", DataType::Utf8, true),
        Field::new("is_in_disease", DataType::Boolean, true),
        Field::new("is_inferred", DataType::Boolean, true),
        Field::new("has_diagram", DataType::Boolean, true),
        Field::new("has_ehld", DataType::Boolean, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(UInt64Array::from(rows.iter().map(|p| p.db_id).collect::<Vec<_>>())),
            str_array(rows.iter().map(|p| p.stable_id.clone()).collect()),
            str_array(rows.iter().map(|p| Some(p.display_name.clone())).collect()),
            str_array(rows.iter().map(|p| p.schema_class.clone()).collect()),
            str_array(rows.iter().map(|p| p.species_name.clone()).collect()),
            Arc::new(BooleanArray::from(rows.iter().map(|p| p.is_in_disease).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|p| p.is_inferred).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|p| p.has_diagram).collect::<Vec<_>>())),
            Arc::new(BooleanArray::from(rows.iter().map(|p| p.has_ehld).collect::<Vec<_>>())),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build Reactome pathways batch: {e}")))
}

pub(crate) fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|o| o.as_deref()).collect();
    Arc::new(StringArray::from(refs))
}
