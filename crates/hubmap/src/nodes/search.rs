//! Nodes over HuBMAP Search and Entity APIs.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::HubmapClient;
use crate::nodes::util::{
    bool_array, dataframe_output, dataframe_port, f64, f64_array, request_error, schedule_error,
    str_array, u64, u64_array, utf8,
};
use crate::types::{DatasetFile, Entity, HubmapSearchFilter, SearchResponse};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_hubmap_factory {
    ($factory:ident, $node:ident, $spec:ident, $kind:literal, $description:literal, $documentation:literal) => {
        impl NodeFactory for $factory {
            fn kind(&self) -> &'static str {
                $kind
            }

            fn desc(&self) -> &'static str {
                $description
            }

            fn doc(&self) -> &'static str {
                $documentation
            }

            fn spec_schema(&self) -> schemars::Schema {
                schema_for!($spec)
            }

            fn ports(&self) -> NodePorts {
                dataframe_port()
            }

            fn build(
                &self,
                spec: serde_json::Value,
                _node_ctx: NodeCtx,
            ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
                let spec = serde_json::from_value::<$spec>(spec)?;
                Ok(Box::new($node {
                    meta: dataframe_port(),
                    spec,
                }))
            }
        }
    };
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HubmapSearchSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub entity_type: Option<String>,
    #[serde(default)]
    pub organ: Option<String>,
    #[serde(default)]
    pub assay: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub from: Option<u32>,
    #[serde(default = "default_size")]
    pub size: u32,
}

fn default_size() -> u32 {
    20
}

#[derive(Clone)]
pub struct HubmapSearchNode {
    meta: NodePorts,
    spec: HubmapSearchSpec,
}

pub struct HubmapSearchNodeFactory;

impl_hubmap_factory!(
    HubmapSearchNodeFactory,
    HubmapSearchNode,
    HubmapSearchSpec,
    "source_hubmap_search",
    "Searches HuBMAP entities and emits normalized metadata.",
    "A source node over the official HuBMAP Search API v3 `/search` DSL. \
     Supports free text, entity type, organ, assay, publication status, and \
     from/size pagination. Public entities are returned without a token; set \
     `HUBMAP_TOKEN` to extend access where permitted."
);

#[async_trait]
impl DagNode for HubmapSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hubmap_search"
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
        let filter = HubmapSearchFilter {
            query: self.spec.query.clone(),
            entity_type: self.spec.entity_type.clone(),
            organ: self.spec.organ.clone(),
            assay: self.spec.assay.clone(),
            status: self.spec.status.clone(),
            from: self.spec.from,
            size: self.spec.size,
        };
        let response = HubmapClient::new()
            .search(&filter)
            .await
            .map_err(|error| request_error(error, "source_hubmap_search"))?;
        let batch = build_search_batch(&response)?;
        dataframe_output(ctx, batch, "source_hubmap_search")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HubmapDatasetSpec {
    pub identifier: String,
}

#[derive(Clone)]
pub struct HubmapDatasetNode {
    meta: NodePorts,
    spec: HubmapDatasetSpec,
}

pub struct HubmapDatasetNodeFactory;

impl_hubmap_factory!(
    HubmapDatasetNodeFactory,
    HubmapDatasetNode,
    HubmapDatasetSpec,
    "source_hubmap_dataset",
    "Fetches one HuBMAP dataset and its metadata.",
    "Resolves a HuBMAP UUID or HBM identifier through the public Search API \
     index. Output includes title, description, status, organ, assay, donor, \
     metadata JSON, file count, and timestamps. No data files are fetched."
);

#[async_trait]
impl DagNode for HubmapDatasetNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hubmap_dataset"
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
        let entity = HubmapClient::new()
            .entity_from_search(&self.spec.identifier)
            .await
            .map_err(|error| request_error(error, "source_hubmap_dataset"))?;
        let batch = build_dataset_batch(&entity)?;
        dataframe_output(ctx, batch, "source_hubmap_dataset")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HubmapFilesSpec {
    pub identifier: String,
    #[serde(default)]
    pub file_type: Option<String>,
}

#[derive(Clone)]
pub struct HubmapFilesNode {
    meta: NodePorts,
    spec: HubmapFilesSpec,
}

pub struct HubmapFilesNodeFactory;

impl_hubmap_factory!(
    HubmapFilesNodeFactory,
    HubmapFilesNode,
    HubmapFilesSpec,
    "source_hubmap_files",
    "Lists HuBMAP dataset files without downloading them.",
    "Uses the public Search API file manifest for a dataset identifier. Rows \
     include relative path, type, EDAM term, description, size, QA/QC and \
     data-product flags. Use the Globus URL node for bulk transfer."
);

#[async_trait]
impl DagNode for HubmapFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hubmap_files"
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
        let entity = HubmapClient::new()
            .entity_from_search(&self.spec.identifier)
            .await
            .map_err(|error| request_error(error, "source_hubmap_files"))?;
        let mut files = entity.files.clone();
        if let Some(file_type) = self
            .spec
            .file_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            files.retain(|file| {
                file.file_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(file_type))
            });
        }
        let batch = build_file_batch(&files, &entity.uuid)?;
        dataframe_output(ctx, batch, "source_hubmap_files")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HubmapGlobusSpec {
    pub identifier: String,
}

#[derive(Clone)]
pub struct HubmapGlobusNode {
    meta: NodePorts,
    spec: HubmapGlobusSpec,
}

pub struct HubmapGlobusNodeFactory;

impl_hubmap_factory!(
    HubmapGlobusNodeFactory,
    HubmapGlobusNode,
    HubmapGlobusSpec,
    "source_hubmap_globus",
    "Resolves a HuBMAP dataset's Globus transfer URL.",
    "Calls the Entity API `entities/{id}/globus-url` endpoint and returns a \
     one-row DataFrame containing the entity identifier and transfer URL. It \
     does not perform the transfer or cache any files."
);

#[async_trait]
impl DagNode for HubmapGlobusNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hubmap_globus"
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
        let client = HubmapClient::new();
        let url = client
            .globus_url(&self.spec.identifier)
            .await
            .map_err(|error| request_error(error, "source_hubmap_globus"))?;
        let batch = build_globus_batch(self.spec.identifier.trim(), &url)?;
        dataframe_output(ctx, batch, "source_hubmap_globus")
    }
}

fn organ(entity: &Entity) -> Option<String> {
    let values: Vec<String> = entity
        .origin_samples
        .iter()
        .flat_map(|sample| sample.organ.clone())
        .collect();
    (!values.is_empty()).then(|| values.join("; "))
}

fn joined(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join("; "))
}

fn json(value: &Option<Value>) -> Option<String> {
    value
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
}

fn metadata_string(entity: &Entity, field: &str) -> Option<String> {
    entity
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(field))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

fn build_search_batch(response: &SearchResponse) -> Result<RecordBatch, DagError> {
    let rows = &response.hits.hits;
    let count = rows.len();
    let total = response.hits.total.as_ref().and_then(|total| total.value);
    let schema = Arc::new(Schema::new(vec![
        utf8("total_count"),
        f64("score"),
        utf8("uuid"),
        utf8("hubmap_id"),
        utf8("entity_type"),
        utf8("title"),
        utf8("description"),
        utf8("status"),
        utf8("data_access_level"),
        utf8("dataset_type"),
        utf8("data_types"),
        utf8("organ"),
        utf8("assay_type"),
        utf8("group_name"),
        utf8("doi_url"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![total.map(|value| value.to_string()); count]),
            f64_array(rows.iter().map(|row| row.score).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.source.uuid.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.source.hubmap_id.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.source.entity_type.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.source.title.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.source.description.clone())
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.source.status.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.source.data_access_level.clone())
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| row.source.dataset_type.clone())
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.source.data_types))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| organ(&row.source)).collect()),
            str_array(
                rows.iter()
                    .map(|row| metadata_string(&row.source, "assay_type"))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| row.source.group_name.clone())
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.source.doi_url.clone()).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_hubmap_search", error))
}

fn build_dataset_batch(entity: &Entity) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("uuid"),
        utf8("hubmap_id"),
        utf8("entity_type"),
        utf8("title"),
        utf8("description"),
        utf8("status"),
        utf8("data_access_level"),
        utf8("dataset_type"),
        utf8("data_types"),
        utf8("organ"),
        utf8("assay_type"),
        utf8("group_name"),
        utf8("doi_url"),
        utf8("donor_json"),
        utf8("metadata_json"),
        utf8("ingest_metadata_json"),
        u64("file_count"),
        u64("created_timestamp"),
        u64("last_modified_timestamp"),
        u64("published_timestamp"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(entity.uuid.clone())]),
            str_array(vec![Some(entity.hubmap_id.clone())]),
            str_array(vec![Some(entity.entity_type.clone())]),
            str_array(vec![entity.title.clone()]),
            str_array(vec![entity.description.clone()]),
            str_array(vec![entity.status.clone()]),
            str_array(vec![entity.data_access_level.clone()]),
            str_array(vec![entity.dataset_type.clone()]),
            str_array(vec![joined(&entity.data_types)]),
            str_array(vec![organ(entity)]),
            str_array(vec![metadata_string(entity, "assay_type")]),
            str_array(vec![entity.group_name.clone()]),
            str_array(vec![entity.doi_url.clone()]),
            str_array(vec![json(&entity.donor)]),
            str_array(vec![json(&entity.metadata)]),
            str_array(vec![json(&entity.ingest_metadata)]),
            u64_array(vec![Some(entity.files.len() as u64)]),
            u64_array(vec![entity.created_timestamp]),
            u64_array(vec![entity.last_modified_timestamp]),
            u64_array(vec![entity.published_timestamp]),
        ],
    )
    .map_err(|error| schedule_error("source_hubmap_dataset", error))
}

fn build_file_batch(rows: &[DatasetFile], uuid: &str) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("dataset_uuid"),
        utf8("relative_path"),
        utf8("file_type"),
        utf8("edam_term"),
        utf8("description"),
        u64("size"),
        arrow_schema::Field::new("is_data_product", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("is_qa_qc", arrow_schema::DataType::Boolean, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(uuid.to_owned()); rows.len()]),
            str_array(
                rows.iter()
                    .map(|row| (!row.rel_path.is_empty()).then(|| row.rel_path.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.file_type.clone()).collect()),
            str_array(rows.iter().map(|row| row.edam_term.clone()).collect()),
            str_array(rows.iter().map(|row| row.description.clone()).collect()),
            u64_array(rows.iter().map(|row| row.size).collect()),
            bool_array(rows.iter().map(|row| row.is_data_product).collect()),
            bool_array(rows.iter().map(|row| row.is_qa_qc).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_hubmap_files", error))
}

fn build_globus_batch(identifier: &str, url: &str) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![utf8("identifier"), utf8("globus_url")]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(identifier.to_owned())]),
            str_array(vec![Some(url.to_owned())]),
        ],
    )
    .map_err(|error| schedule_error("source_hubmap_globus", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_search_and_dataset_batches() {
        let response = SearchResponse::default();
        assert_eq!(build_search_batch(&response).unwrap().num_columns(), 15);
        let entity = Entity::default();
        assert_eq!(build_dataset_batch(&entity).unwrap().num_columns(), 20);
    }

    #[test]
    fn builds_file_and_globus_batches() {
        let file = DatasetFile {
            rel_path: "data.csv".into(),
            ..Default::default()
        };
        assert_eq!(
            build_file_batch(&[file], "0188cbfed79e0714b0e6ea6466ab8977")
                .unwrap()
                .num_columns(),
            8
        );
        assert_eq!(
            build_globus_batch("HBM537.HTKP.258", "https://globus.example")
                .unwrap()
                .num_columns(),
            2
        );
    }

    #[test]
    fn extracts_organs_from_both_representations() {
        let mut entity = Entity::default();
        entity.origin_samples = vec![crate::types::OriginSample {
            organ: Some("SP".into()),
        }];
        assert_eq!(organ(&entity).as_deref(), Some("SP"));
    }
}
