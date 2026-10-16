//! Nodes over the official TISCH2 server-rendered pages.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::TischClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, request_error, schedule_error, str_array, u64, u64_array,
    utf8,
};
use crate::types::{CellType, Dataset};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_tisch_factory {
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
pub struct TischDatasetsSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub species: Option<String>,
    #[serde(default)]
    pub treatment: Option<String>,
    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Clone)]
pub struct TischDatasetsNode {
    meta: NodePorts,
    spec: TischDatasetsSpec,
}

pub struct TischDatasetsNodeFactory;

impl_tisch_factory!(
    TischDatasetsNodeFactory,
    TischDatasetsNode,
    TischDatasetsSpec,
    "source_tisch_datasets",
    "Lists TISCH2 tumor microenvironment datasets.",
    "Parses the official TISCH2 Dataset page with an HTML parser. The current \
     deployment has no JSON API, so this degraded query source uses the \
     server-rendered table. Output includes TISCH ID, dataset name, species, \
     treatment, patients, cells, platform, primary/metastatic status, and PMID."
);

#[async_trait]
impl DagNode for TischDatasetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_tisch_datasets"
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
        let filter = crate::types::DatasetFilter {
            query: self.spec.query.clone(),
            species: self.spec.species.clone(),
            treatment: self.spec.treatment.clone(),
            dataset: self.spec.dataset.clone(),
            max_results: self.spec.max_results.map(|value| value as usize),
        };
        let datasets = TischClient::new()
            .datasets(&filter)
            .await
            .map_err(|error| request_error(error, "source_tisch_datasets"))?;
        let batch = build_dataset_batch(&datasets)?;
        dataframe_output(ctx, batch, "source_tisch_datasets")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct TischCellTypesSpec {}

#[derive(Clone)]
pub struct TischCellTypesNode {
    meta: NodePorts,
    #[allow(dead_code)]
    spec: TischCellTypesSpec,
}

pub struct TischCellTypesNodeFactory;

impl_tisch_factory!(
    TischCellTypesNodeFactory,
    TischCellTypesNode,
    TischCellTypesSpec,
    "source_tisch_cell_types",
    "Lists TISCH2 cell-type abbreviations and labels.",
    "Parses the official cell-type abbreviation table from the TISCH2 \
     documentation page. This provides the controlled labels used by TISCH's \
     major- and minor-lineage annotations."
);

#[async_trait]
impl DagNode for TischCellTypesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_tisch_cell_types"
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
        let cell_types = TischClient::new()
            .cell_types()
            .await
            .map_err(|error| request_error(error, "source_tisch_cell_types"))?;
        let batch = build_cell_type_batch(&cell_types)?;
        dataframe_output(ctx, batch, "source_tisch_cell_types")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct TischAssetsSpec {
    pub dataset: String,
}

#[derive(Clone)]
pub struct TischAssetsNode {
    meta: NodePorts,
    spec: TischAssetsSpec,
}

pub struct TischAssetsNodeFactory;

impl_tisch_factory!(
    TischAssetsNodeFactory,
    TischAssetsNode,
    TischAssetsSpec,
    "source_tisch_assets",
    "Lists official TISCH2 static assets for one dataset.",
    "Returns the official dataset archive URL from the TISCH table and the \
     corresponding UMAP image URL. The current TISCH deployment has removed \
     some archive files, so this is a source manifest rather than a download."
);

#[async_trait]
impl DagNode for TischAssetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_tisch_assets"
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
        let dataset = self.spec.dataset.trim();
        if dataset.is_empty() {
            return Err(schedule_error(
                "source_tisch_assets",
                "dataset must not be empty",
            ));
        }
        let client = TischClient::new();
        let (data_url, umap_url) = crate::types::dataset_assets(client.endpoint(), dataset);
        let rows = vec![("dataset_archive", data_url), ("umap_image", umap_url)];
        let batch = build_asset_batch(dataset, &rows)?;
        dataframe_output(ctx, batch, "source_tisch_assets")
    }
}

fn build_dataset_batch(rows: &[Dataset]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("tisch_id"),
        utf8("dataset"),
        utf8("species"),
        utf8("treatment"),
        u64("patients"),
        u64("cells"),
        utf8("platform"),
        utf8("primary_or_metastatic"),
        utf8("pubmed_id"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.tisch_id.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.dataset.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.species.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.treatment.clone())).collect()),
            u64_array(rows.iter().map(|row| row.patients).collect()),
            u64_array(rows.iter().map(|row| row.cells).collect()),
            str_array(rows.iter().map(|row| Some(row.platform.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.primary_or_metastatic.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.pubmed_id.clone())).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_tisch_datasets", error))
}

fn build_cell_type_batch(rows: &[CellType]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![utf8("abbreviation"), utf8("label")]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(
                rows.iter()
                    .map(|row| Some(row.abbreviation.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.label.clone())).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_tisch_cell_types", error))
}

fn build_asset_batch(
    dataset: &str,
    rows: &[(&'static str, String)],
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("dataset"),
        utf8("asset_type"),
        utf8("url"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(dataset.to_owned()); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.0.to_owned())).collect()),
            str_array(rows.iter().map(|row| Some(row.1.clone())).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_tisch_assets", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_batches() {
        let dataset = Dataset::default();
        assert_eq!(build_dataset_batch(&[dataset]).unwrap().num_columns(), 9);
        let cell_type = CellType::default();
        assert_eq!(
            build_cell_type_batch(&[cell_type]).unwrap().num_columns(),
            2
        );
        let rows = vec![("umap_image", "https://example.test/x.png".into())];
        assert_eq!(
            build_asset_batch("AEL_GSE142213", &rows)
                .unwrap()
                .num_columns(),
            3
        );
    }
}
