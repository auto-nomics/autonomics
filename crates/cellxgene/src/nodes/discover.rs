//! CELLxGENE Discover source nodes.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::CellxgeneClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, f64, f64_array, request_error, schedule_error, str_array,
    u64, u64_array, utf8,
};
use crate::types::{Collection, Dataset, DatasetFilter};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_cellxgene_factory {
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
pub struct CellxgeneCollectionsSpec {
    /// Case-insensitive text filter over collection name and citation.
    #[serde(default)]
    pub query: Option<String>,
    /// Maximum rows retained after local filtering.
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Clone)]
pub struct CellxgeneCollectionsNode {
    meta: NodePorts,
    spec: CellxgeneCollectionsSpec,
}

pub struct CellxgeneCollectionsNodeFactory;

impl_cellxgene_factory!(
    CellxgeneCollectionsNodeFactory,
    CellxgeneCollectionsNode,
    CellxgeneCollectionsSpec,
    "source_cellxgene_collections",
    "Lists CZ CELLxGENE Discover collections as a table.",
    "A zero-input source node over the official CELLxGENE Discover \
     collections/index API. Output columns include collection_id, name, \
     consortia, journal, publication_year, citation, published_at, and \
     revised_at. Filtering is local because the public index endpoint does \
     not currently expose server-side search."
);

#[async_trait]
impl DagNode for CellxgeneCollectionsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_cellxgene_collections"
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
        let collections = CellxgeneClient::new()
            .collections()
            .await
            .map_err(|error| request_error(error, "source_cellxgene_collections"))?;
        let collections = filter_collections(
            &collections,
            self.spec.query.as_deref(),
            self.spec.max_results.map(|value| value as usize),
        );
        let batch = build_collection_batch(&collections)?;
        dataframe_output(ctx, batch, "source_cellxgene_collections")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct CellxgeneCollectionSpec {
    /// CELLxGENE collection UUID.
    pub collection_id: String,
}

#[derive(Clone)]
pub struct CellxgeneCollectionNode {
    meta: NodePorts,
    spec: CellxgeneCollectionSpec,
}

pub struct CellxgeneCollectionNodeFactory;

impl_cellxgene_factory!(
    CellxgeneCollectionNodeFactory,
    CellxgeneCollectionNode,
    CellxgeneCollectionSpec,
    "source_cellxgene_collection",
    "Fetches one CZ CELLxGENE collection and dataset summary.",
    "Resolves a CELLxGENE collection UUID through collections/{id}. Output \
     columns include collection identity, contact and publication metadata, \
     dataset_count, total_cell_count, links_json, and datasets_json. No H5AD \
     files are downloaded."
);

#[async_trait]
impl DagNode for CellxgeneCollectionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_cellxgene_collection"
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
        let collection = CellxgeneClient::new()
            .collection(&self.spec.collection_id)
            .await
            .map_err(|error| request_error(error, "source_cellxgene_collection"))?;
        let batch = build_collection_detail_batch(&collection)?;
        dataframe_output(ctx, batch, "source_cellxgene_collection")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct CellxgeneDatasetsSpec {
    /// Case-insensitive text filter over dataset name, UUID, and ontology labels.
    #[serde(default)]
    pub query: Option<String>,
    /// Exact collection UUID filter.
    #[serde(default)]
    pub collection_id: Option<String>,
    /// Organism label or NCBITaxon ontology ID substring.
    #[serde(default)]
    pub organism: Option<String>,
    /// Tissue label or UBERON ontology ID substring.
    #[serde(default)]
    pub tissue: Option<String>,
    /// Disease label or MONDO ontology ID substring.
    #[serde(default)]
    pub disease: Option<String>,
    /// Cell type label or Cell Ontology ID substring.
    #[serde(default)]
    pub cell_type: Option<String>,
    /// Assay label or EFO ontology ID substring.
    #[serde(default)]
    pub assay: Option<String>,
    /// Maximum rows retained after local filtering.
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Clone)]
pub struct CellxgeneDatasetsNode {
    meta: NodePorts,
    spec: CellxgeneDatasetsSpec,
}

pub struct CellxgeneDatasetsNodeFactory;

impl_cellxgene_factory!(
    CellxgeneDatasetsNodeFactory,
    CellxgeneDatasetsNode,
    CellxgeneDatasetsSpec,
    "source_cellxgene_datasets",
    "Searches CZ CELLxGENE Discover datasets and emits metadata.",
    "A source node over datasets/index. It supports collection, organism, \
     tissue, disease, cell type, assay, and free-text filters. Output is a \
     metadata DataFrame and includes cell counts, schema version, explorer \
     URL, ontology terms, and asset JSON. Matrix files are not downloaded."
);

#[async_trait]
impl DagNode for CellxgeneDatasetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_cellxgene_datasets"
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
        let filter = DatasetFilter {
            collection_id: self.spec.collection_id.clone(),
            query: self.spec.query.clone(),
            organism: self.spec.organism.clone(),
            tissue: self.spec.tissue.clone(),
            disease: self.spec.disease.clone(),
            cell_type: self.spec.cell_type.clone(),
            assay: self.spec.assay.clone(),
            max_results: self.spec.max_results.map(|value| value as usize),
        };
        let datasets = CellxgeneClient::new()
            .datasets(&filter)
            .await
            .map_err(|error| request_error(error, "source_cellxgene_datasets"))?;
        let batch = build_dataset_batch(&datasets)?;
        dataframe_output(ctx, batch, "source_cellxgene_datasets")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct CellxgeneAssetsSpec {
    /// CELLxGENE dataset UUID.
    pub dataset_id: String,
    /// Optional asset type filter, such as `H5AD`, `RAW_H5AD`, or `CXG`.
    #[serde(default)]
    pub filetype: Option<String>,
}

#[derive(Clone)]
pub struct CellxgeneAssetsNode {
    meta: NodePorts,
    spec: CellxgeneAssetsSpec,
}

pub struct CellxgeneAssetsNodeFactory;

impl_cellxgene_factory!(
    CellxgeneAssetsNodeFactory,
    CellxgeneAssetsNode,
    CellxgeneAssetsSpec,
    "source_cellxgene_assets",
    "Lists downloadable CZ CELLxGENE dataset assets without fetching them.",
    "Resolves the official datasets/{dataset_id}/assets endpoint. Rows \
     describe asset ID, type, filename, S3 URI, timestamps, and provenance. \
     The node returns metadata only; H5AD transfer belongs to a dedicated \
     download/VFS node."
);

#[async_trait]
impl DagNode for CellxgeneAssetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_cellxgene_assets"
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
        let mut assets = CellxgeneClient::new()
            .dataset_assets(&self.spec.dataset_id)
            .await
            .map_err(|error| request_error(error, "source_cellxgene_assets"))?;
        if let Some(filetype) = self
            .spec
            .filetype
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            assets.retain(|asset| {
                asset
                    .filetype
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(filetype))
            });
        }
        let batch = build_asset_batch(&assets)?;
        dataframe_output(ctx, batch, "source_cellxgene_assets")
    }
}

fn filter_collections(
    collections: &[Collection],
    query: Option<&str>,
    max_results: Option<usize>,
) -> Vec<Collection> {
    let query = query
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let mut result: Vec<Collection> = collections
        .iter()
        .filter(|collection| {
            query.as_ref().is_none_or(|query| {
                let haystack = format!(
                    "{} {}",
                    collection.name,
                    collection.summary_citation.as_deref().unwrap_or_default()
                )
                .to_ascii_lowercase();
                haystack.contains(query)
            })
        })
        .cloned()
        .collect();
    if let Some(max_results) = max_results {
        result.truncate(max_results.max(1));
    }
    result
}

fn ontology_text(terms: &[crate::types::OntologyTerm]) -> Option<String> {
    (!terms.is_empty()).then(|| {
        terms
            .iter()
            .map(|term| format!("{} ({})", term.label, term.ontology_term_id))
            .collect::<Vec<_>>()
            .join("; ")
    })
}

fn joined(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join("; "))
}

fn json<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn build_collection_batch(rows: &[Collection]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("collection_id"),
        utf8("name"),
        utf8("consortia"),
        utf8("journal"),
        u64("publication_year"),
        utf8("citation"),
        f64("published_at"),
        f64("revised_at"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.id.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(rows.iter().map(|row| joined(&row.consortia)).collect()),
            str_array(
                rows.iter()
                    .map(|row| {
                        row.publisher_metadata
                            .as_ref()
                            .and_then(|metadata| metadata.journal.clone())
                    })
                    .collect(),
            ),
            u64_array(
                rows.iter()
                    .map(|row| {
                        row.publisher_metadata
                            .as_ref()
                            .and_then(|metadata| metadata.published_year.map(u64::from))
                    })
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| row.summary_citation.clone())
                    .collect(),
            ),
            f64_array(rows.iter().map(|row| row.published_at).collect()),
            f64_array(rows.iter().map(|row| row.revised_at).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_cellxgene_collections", error))
}

fn build_collection_detail_batch(
    collection: &crate::types::CollectionDetail,
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("collection_id"),
        utf8("name"),
        utf8("description"),
        utf8("contact_name"),
        utf8("contact_email"),
        utf8("consortia"),
        utf8("citation"),
        utf8("journal"),
        u64("dataset_count"),
        u64("total_cell_count"),
        utf8("links_json"),
        utf8("datasets_json"),
    ]));
    let total_cell_count: u64 = collection
        .datasets
        .iter()
        .filter_map(|dataset| dataset.cell_count)
        .sum();
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(collection.id.clone())]),
            str_array(vec![Some(collection.name.clone())]),
            str_array(vec![collection.description.clone()]),
            str_array(vec![collection.contact_name.clone()]),
            str_array(vec![collection.contact_email.clone()]),
            str_array(vec![joined(&collection.consortia)]),
            str_array(vec![collection.summary_citation.clone()]),
            str_array(vec![
                collection
                    .publisher_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.journal.clone()),
            ]),
            u64_array(vec![Some(collection.datasets.len() as u64)]),
            u64_array(vec![Some(total_cell_count)]),
            str_array(vec![json(&collection.links)]),
            str_array(vec![json(&collection.datasets)]),
        ],
    )
    .map_err(|error| schedule_error("source_cellxgene_collection", error))
}

fn build_dataset_batch(rows: &[Dataset]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("dataset_id"),
        utf8("collection_id"),
        utf8("name"),
        utf8("organism"),
        utf8("tissue"),
        utf8("disease"),
        utf8("cell_type"),
        utf8("assay"),
        u64("cell_count"),
        u64("primary_cell_count"),
        f64("mean_genes_per_cell"),
        utf8("schema_version"),
        utf8("suspension_type"),
        utf8("explorer_url"),
        utf8("assets_json"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.id.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.collection_id.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| ontology_text(&row.organism))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| ontology_text(&row.tissue)).collect()),
            str_array(rows.iter().map(|row| ontology_text(&row.disease)).collect()),
            str_array(
                rows.iter()
                    .map(|row| ontology_text(&row.cell_type))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| ontology_text(&row.assay)).collect()),
            u64_array(rows.iter().map(|row| row.cell_count).collect()),
            u64_array(rows.iter().map(|row| row.primary_cell_count).collect()),
            f64_array(rows.iter().map(|row| row.mean_genes_per_cell).collect()),
            str_array(rows.iter().map(|row| row.schema_version.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.suspension_type))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.explorer_url.clone()).collect()),
            str_array(rows.iter().map(|row| json(&row.dataset_assets)).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_cellxgene_datasets", error))
}

fn build_asset_batch(rows: &[crate::types::DatasetAsset]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("asset_id"),
        utf8("dataset_id"),
        utf8("filetype"),
        utf8("filename"),
        utf8("s3_uri"),
        f64("created_at"),
        f64("updated_at"),
        utf8("user_submitted"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.id.clone())).collect()),
            str_array(rows.iter().map(|row| row.dataset_id.clone()).collect()),
            str_array(rows.iter().map(|row| row.filetype.clone()).collect()),
            str_array(rows.iter().map(|row| row.filename.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| (!row.s3_uri.is_empty()).then(|| row.s3_uri.clone()))
                    .collect(),
            ),
            f64_array(rows.iter().map(|row| row.created_at).collect()),
            f64_array(rows.iter().map(|row| row.updated_at).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.user_submitted.map(|flag| flag.to_string()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_cellxgene_assets", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OntologyTerm;

    fn dataset(name: &str, label: &str) -> Dataset {
        Dataset {
            id: format!("id-{name}"),
            collection_id: "collection-1".into(),
            name: name.into(),
            organism: vec![OntologyTerm {
                label: label.into(),
                ontology_term_id: "NCBITaxon:9606".into(),
                tissue_type: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn filters_datasets_by_query_and_ontology() {
        let rows = vec![
            dataset("Retina", "Homo sapiens"),
            dataset("Brain", "Mus musculus"),
        ];
        let filter = DatasetFilter {
            query: Some("retina".into()),
            organism: Some("NCBITaxon:9606".into()),
            ..Default::default()
        };
        let result = crate::types::filter_datasets(&rows, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Retina");
    }

    #[test]
    fn caps_dataset_results() {
        let rows = vec![dataset("A", "Homo sapiens"), dataset("B", "Homo sapiens")];
        let filter = DatasetFilter {
            max_results: Some(1),
            ..Default::default()
        };
        assert_eq!(crate::types::filter_datasets(&rows, &filter).len(), 1);
    }

    #[test]
    fn builds_collection_and_dataset_batches() {
        let collection = Collection {
            id: "collection-1".into(),
            name: "Reference".into(),
            consortia: vec!["CZI".into()],
            ..Default::default()
        };
        assert_eq!(build_collection_batch(&[collection]).unwrap().num_rows(), 1);
        assert_eq!(
            build_dataset_batch(&[dataset("Retina", "Homo sapiens")])
                .unwrap()
                .num_rows(),
            1
        );
    }
}
