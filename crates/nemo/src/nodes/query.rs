//! Query nodes over the public NeMO Analytics API.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::NemoClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, request_error, schedule_error, str_array, u8, u8_array, u64,
    u64_array, utf8,
};
use crate::types::{Collection, Dataset};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_nemo_factory {
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
pub struct NemoCollectionsSpec {}

#[derive(Clone)]
pub struct NemoCollectionsNode {
    meta: NodePorts,
    #[allow(dead_code)]
    spec: NemoCollectionsSpec,
}

pub struct NemoCollectionsNodeFactory;

impl_nemo_factory!(
    NemoCollectionsNodeFactory,
    NemoCollectionsNode,
    NemoCollectionsSpec,
    "source_nemo_collections",
    "Lists NeMO Analytics dataset collections.",
    "Queries the official public layout endpoint without collection members. \
     Rows contain collection share ID, label, domain flag, current flag, and \
     the portal URL."
);

#[async_trait]
impl DagNode for NemoCollectionsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_nemo_collections"
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
        let collections = NemoClient::new()
            .collections()
            .await
            .map_err(|error| request_error(error, "source_nemo_collections"))?;
        let batch = build_collection_batch(&collections)?;
        dataframe_output(ctx, batch, "source_nemo_collections")
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct NemoDatasetsSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub dataset_type: Option<String>,
}

fn default_page() -> u32 {
    1
}

fn default_limit() -> u32 {
    20
}

#[derive(Clone)]
pub struct NemoDatasetsNode {
    meta: NodePorts,
    spec: NemoDatasetsSpec,
}

pub struct NemoDatasetsNodeFactory;

impl_nemo_factory!(
    NemoDatasetsNodeFactory,
    NemoDatasetsNode,
    NemoDatasetsSpec,
    "source_nemo_datasets",
    "Searches public NeMO Analytics datasets.",
    "Uses the public dataset search endpoint used by the official dataset \
     explorer. Rows include organism, dtype, access flags, publication and GEO \
     accession, load status, H5AD/tarball availability, and pagination totals."
);

#[async_trait]
impl DagNode for NemoDatasetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_nemo_datasets"
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
        let search = crate::DatasetSearch {
            query: self.spec.query.clone(),
            dataset_type: self.spec.dataset_type.clone(),
            page: self.spec.page,
            limit: self.spec.limit,
        };
        let (datasets, pagination) = NemoClient::new()
            .datasets(search)
            .await
            .map_err(|error| request_error(error, "source_nemo_datasets"))?;
        let total = pagination.total_results.unwrap_or(datasets.len() as u64);
        let batch = build_dataset_batch(&datasets, total)?;
        dataframe_output(ctx, batch, "source_nemo_datasets")
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct NemoMatricesSpec {
    pub dataset_id: String,
}

#[derive(Clone)]
pub struct NemoMatricesNode {
    meta: NodePorts,
    spec: NemoMatricesSpec,
}

pub struct NemoMatricesNodeFactory;

impl_nemo_factory!(
    NemoMatricesNodeFactory,
    NemoMatricesNode,
    NemoMatricesSpec,
    "source_nemo_matrices",
    "Fetches NeMO matrix metadata without downloading matrices.",
    "Calls the official dataset info endpoint with shape expansion. Output \
     includes H5AD and tarball availability, cell and gene counts, public and \
     downloadable flags, share ID, date added, and portal URL. The matrix \
     itself is not fetched."
);

#[async_trait]
impl DagNode for NemoMatricesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_nemo_matrices"
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
        let dataset = NemoClient::new()
            .dataset(&self.spec.dataset_id)
            .await
            .map_err(|error| request_error(error, "source_nemo_matrices"))?;
        let batch = build_matrix_batch(&dataset)?;
        dataframe_output(ctx, batch, "source_nemo_matrices")
    }
}

fn portal_url(dataset_id: &str) -> String {
    format!("https://nemoanalytics.org/dataset_explorer.html?dataset_id={dataset_id}")
}

fn collection_url(share_id: &str) -> String {
    format!("https://nemoanalytics.org/dataset_explorer.html?layout_share_id={share_id}")
}

fn build_collection_batch(rows: &[Collection]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("share_id"),
        utf8("label"),
        u64("dataset_count"),
        u8("is_current"),
        u8("is_domain"),
        utf8("portal_url"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.share_id.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.label.clone())).collect()),
            u64_array(rows.iter().map(|row| Some(row.dataset_count)).collect()),
            u8_array(rows.iter().map(|row| Some(row.is_current)).collect()),
            u8_array(rows.iter().map(|row| Some(row.is_domain)).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(collection_url(&row.share_id)))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_nemo_collections", error))
}

fn build_dataset_batch(rows: &[Dataset], total_results: u64) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("dataset_id"),
        utf8("title"),
        utf8("share_id"),
        utf8("dataset_type"),
        utf8("organism"),
        u64("organism_id"),
        utf8("pubmed_id"),
        utf8("geo_id"),
        u8("is_public"),
        u8("is_downloadable"),
        utf8("date_added"),
        utf8("load_status"),
        u8("has_h5ad"),
        u8("has_tarball"),
        utf8("annotation_release"),
        u64("total_results"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.id.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.title.clone())).collect()),
            str_array(rows.iter().map(|row| row.share_id.clone()).collect()),
            str_array(rows.iter().map(|row| row.dtype.clone()).collect()),
            str_array(rows.iter().map(|row| row.organism.clone()).collect()),
            u64_array(rows.iter().map(|row| row.organism_id).collect()),
            str_array(rows.iter().map(|row| row.pubmed_id.clone()).collect()),
            str_array(rows.iter().map(|row| row.geo_id.clone()).collect()),
            u8_array(rows.iter().map(|row| row.is_public).collect()),
            u8_array(rows.iter().map(|row| row.is_downloadable).collect()),
            str_array(rows.iter().map(|row| row.date_added.clone()).collect()),
            str_array(rows.iter().map(|row| row.load_status.clone()).collect()),
            u8_array(rows.iter().map(|row| row.has_h5ad).collect()),
            u8_array(rows.iter().map(|row| row.has_tarball).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.annotation_release.clone())
                    .collect(),
            ),
            u64_array(vec![Some(total_results); rows.len()]),
        ],
    )
    .map_err(|error| schedule_error("source_nemo_datasets", error))
}

fn build_matrix_batch(dataset: &Dataset) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("dataset_id"),
        utf8("title"),
        utf8("share_id"),
        utf8("dataset_type"),
        u64("organism_id"),
        u64("gene_count"),
        u64("obs_count"),
        u8("has_h5ad"),
        u8("has_tarball"),
        u8("is_public"),
        u8("is_downloadable"),
        utf8("date_added"),
        utf8("portal_url"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(dataset.id.clone())]),
            str_array(vec![Some(dataset.title.clone())]),
            str_array(vec![dataset.share_id.clone()]),
            str_array(vec![dataset.dtype.clone()]),
            u64_array(vec![dataset.organism_id]),
            u64_array(vec![dataset.gene_count]),
            u64_array(vec![dataset.obs_count]),
            u8_array(vec![dataset.has_h5ad]),
            u8_array(vec![dataset.has_tarball]),
            u8_array(vec![dataset.is_public]),
            u8_array(vec![dataset.is_downloadable]),
            str_array(vec![dataset.date_added.clone()]),
            str_array(vec![Some(portal_url(&dataset.id))]),
        ],
    )
    .map_err(|error| schedule_error("source_nemo_matrices", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_query_batches() {
        let collection = Collection {
            label: "BICCN Mini-Atlas".into(),
            share_id: "1b58cf00".into(),
            dataset_count: 4,
            is_domain: 1,
            ..Collection::default()
        };
        assert_eq!(
            build_collection_batch(&[collection]).unwrap().num_columns(),
            6
        );
        let dataset = Dataset {
            id: "4fbd43e2-f301-42d8-a61d-a5dd5bf720e7".into(),
            title: "Primary Motor Cortex".into(),
            dtype: Some("single-cell-rnaseq".into()),
            gene_count: Some(31_053),
            obs_count: Some(10_000),
            ..Dataset::default()
        };
        assert_eq!(
            build_dataset_batch(std::slice::from_ref(&dataset), 2168)
                .unwrap()
                .num_columns(),
            16
        );
        assert_eq!(build_matrix_batch(&dataset).unwrap().num_columns(), 13);
    }
}
