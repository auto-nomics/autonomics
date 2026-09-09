use std::sync::Arc;

use arrow_array::{Float64Array, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::RcsbClient;
use crate::search::{SearchQuery, SearchRequest, SearchReturnType};

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct RcsbSearchSpec {
    /// Full-text query. Required unless `raw_query` is supplied.
    #[serde(default)]
    pub query: String,

    /// Raw RCSB Search API query-node object (expert mode). The object must
    /// contain `type`, and for terminal nodes `service` and `parameters`.
    #[serde(default)]
    pub raw_query: Option<serde_json::Value>,

    /// Page size, 1..=10000. Default: 25.
    #[serde(default)]
    pub rows: Option<u32>,

    /// Zero-based result offset.
    #[serde(default)]
    pub start: Option<u32>,

    /// `entry` (default), `polymer_entity`, `polymer_entity_instance`,
    /// `assembly`, or `chem_comp`.
    #[serde(default)]
    pub return_type: Option<String>,
}

#[derive(Clone)]
pub struct RcsbSearchNode {
    meta: NodePorts,
    spec: RcsbSearchSpec,
}

pub struct RcsbSearchNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for RcsbSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_rcsb_search"
    }

    fn desc(&self) -> &'static str {
        "Search RCSB PDB and emit identifiers and relevance scores."
    }

    fn doc(&self) -> &'static str {
        "Calls the RCSB Search API and emits a one-row-per-hit table with \
        `identifier` and `score` columns. Use `query` for full-text search, or \
        `raw_query` for an attribute, sequence, structure, or chemical query. \
        Feed the result into `source_rcsb_entry` or other ID-driven nodes for \
        detailed metadata."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RcsbSearchSpec)
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
        Ok(Box::new(RcsbSearchNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for RcsbSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_rcsb_search"
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
        let query = if let Some(raw) = self.spec.raw_query.clone() {
            serde_json::from_value::<SearchQuery>(raw)
                .map_err(|e| DagError::Schedule(format!("invalid RCSB raw_query: {e}")))?
        } else if !self.spec.query.trim().is_empty() {
            SearchQuery::full_text(self.spec.query.trim())
        } else {
            return Err(DagError::Schedule(
                "source_rcsb_search requires `query` or `raw_query`".into(),
            ));
        };

        let return_type = parse_return_type(self.spec.return_type.as_deref())?;
        let mut request = SearchRequest {
            query,
            return_type,
            request_options: crate::search::SearchRequestOptions {
                paginate: Default::default(),
                results_content_type: vec!["experimental".into()],
            },
        };
        request.request_options.paginate.start = self.spec.start.unwrap_or(0);
        request.request_options.paginate.rows = self.spec.rows.unwrap_or(25);

        let response = RcsbClient::new()
            .search(&request)
            .await
            .map_err(|e| DagError::Schedule(format!("RCSB search failed: {e}")))?;

        let schema = Arc::new(Schema::new(vec![
            Field::new("identifier", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
        ]));
        let identifiers: Vec<String> = response
            .result_set
            .iter()
            .map(|result| result.identifier.clone())
            .collect();
        let scores: Vec<f64> = response
            .result_set
            .iter()
            .map(|result| result.score)
            .collect();
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(identifiers)),
                Arc::new(Float64Array::from(scores)),
            ],
        )
        .map_err(|e| DagError::Schedule(format!("failed to build RCSB search batch: {e}")))?;

        let session = ctx.session();
        let dataframe = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read RCSB search batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

fn parse_return_type(value: Option<&str>) -> Result<SearchReturnType, DagError> {
    let Some(value) = value else {
        return Ok(SearchReturnType::Entry);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "entry" => Ok(SearchReturnType::Entry),
        "polymer_entity" | "polymer" => Ok(SearchReturnType::PolymerEntity),
        "polymer_entity_instance" | "instance" => Ok(SearchReturnType::PolymerEntityInstance),
        "assembly" => Ok(SearchReturnType::Assembly),
        "chem_comp" | "chemical_component" => Ok(SearchReturnType::ChemComp),
        other => Err(DagError::Schedule(format!(
            "unsupported RCSB return_type {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_return_type_parse() {
        let spec: RcsbSearchSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.query, "");
        assert_eq!(
            parse_return_type(Some("polymer_entity")).unwrap(),
            SearchReturnType::PolymerEntity
        );
    }
}
