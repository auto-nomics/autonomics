//! Nodes over the official Tabula Sapiens figshare release.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::TabulaSapiensClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, request_error, schedule_error, str_array, u32, u32_array,
    u64, u64_array, utf8,
};
use crate::types::{Article, ArticleFile};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_tabula_factory {
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
pub struct TabulaSapiensVersionsSpec {}

#[derive(Clone)]
pub struct TabulaSapiensVersionsNode {
    meta: NodePorts,
    #[allow(dead_code)]
    spec: TabulaSapiensVersionsSpec,
}

pub struct TabulaSapiensVersionsNodeFactory;

impl_tabula_factory!(
    TabulaSapiensVersionsNodeFactory,
    TabulaSapiensVersionsNode,
    TabulaSapiensVersionsSpec,
    "source_tabula_sapiens_versions",
    "Lists official Tabula Sapiens release versions.",
    "Queries the figshare versions endpoint for the official Tabula Sapiens \
     Single-Cell Dataset article. Each row contains a version number and the \
     version-specific API URL."
);

#[async_trait]
impl DagNode for TabulaSapiensVersionsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_tabula_sapiens_versions"
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
        let versions = TabulaSapiensClient::new()
            .versions()
            .await
            .map_err(|error| request_error(error, "source_tabula_sapiens_versions"))?;
        let batch = build_version_batch(&versions)?;
        dataframe_output(ctx, batch, "source_tabula_sapiens_versions")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct TabulaSapiensReleaseSpec {
    #[serde(default)]
    pub version: Option<u32>,
}

#[derive(Clone)]
pub struct TabulaSapiensReleaseNode {
    meta: NodePorts,
    spec: TabulaSapiensReleaseSpec,
}

pub struct TabulaSapiensReleaseNodeFactory;

impl_tabula_factory!(
    TabulaSapiensReleaseNodeFactory,
    TabulaSapiensReleaseNode,
    TabulaSapiensReleaseSpec,
    "source_tabula_sapiens_release",
    "Fetches metadata for one Tabula Sapiens release.",
    "Resolves the official figshare article, or a specified version. Output \
     includes DOI, version, dates, license, portal URL, file count, total \
     size, and description. No H5AD files are downloaded."
);

#[async_trait]
impl DagNode for TabulaSapiensReleaseNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_tabula_sapiens_release"
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
        let article = TabulaSapiensClient::new()
            .article(self.spec.version)
            .await
            .map_err(|error| request_error(error, "source_tabula_sapiens_release"))?;
        let batch = build_article_batch(&article)?;
        dataframe_output(ctx, batch, "source_tabula_sapiens_release")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct TabulaSapiensFilesSpec {
    #[serde(default)]
    pub version: Option<u32>,
    /// `organ_h5ad`, `compartment_h5ad`, `full_atlas_h5ad`, or `metadata`.
    #[serde(default)]
    pub asset_type: Option<String>,
}

#[derive(Clone)]
pub struct TabulaSapiensFilesNode {
    meta: NodePorts,
    spec: TabulaSapiensFilesSpec,
}

pub struct TabulaSapiensFilesNodeFactory;

impl_tabula_factory!(
    TabulaSapiensFilesNodeFactory,
    TabulaSapiensFilesNode,
    TabulaSapiensFilesSpec,
    "source_tabula_sapiens_files",
    "Lists Tabula Sapiens release files without downloading them.",
    "Emits one row per official figshare file. Filenames are classified into \
     organ H5AD, compartment H5AD, full-atlas H5AD, and cell metadata. Rows \
     include size, download URL, release version, and derived organ/category."
);

#[async_trait]
impl DagNode for TabulaSapiensFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_tabula_sapiens_files"
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
        let (article, files) = TabulaSapiensClient::new()
            .files(self.spec.version, self.spec.asset_type.as_deref())
            .await
            .map_err(|error| request_error(error, "source_tabula_sapiens_files"))?;
        let batch = build_file_batch(&files, article.version)?;
        dataframe_output(ctx, batch, "source_tabula_sapiens_files")
    }
}

fn build_version_batch(rows: &[crate::types::Version]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![u32("version"), utf8("api_url")]));
    RecordBatch::try_new(
        schema,
        vec![
            u32_array(rows.iter().map(|row| Some(row.version)).collect()),
            str_array(rows.iter().map(|row| Some(row.url.clone())).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_tabula_sapiens_versions", error))
}

fn build_article_batch(article: &Article) -> Result<RecordBatch, DagError> {
    let total_size: u64 = article.files.iter().filter_map(|file| file.size).sum();
    let schema = Arc::new(Schema::new(vec![
        u64("article_id"),
        u32("version"),
        utf8("title"),
        utf8("doi"),
        utf8("published_date"),
        utf8("modified_date"),
        utf8("license_name"),
        utf8("license_url"),
        utf8("portal_url"),
        u64("file_count"),
        u64("total_size"),
        utf8("description"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            u64_array(vec![Some(article.id)]),
            u32_array(vec![Some(article.version)]),
            str_array(vec![Some(article.title.clone())]),
            str_array(vec![Some(article.doi.clone())]),
            str_array(vec![article.published_date.clone()]),
            str_array(vec![article.modified_date.clone()]),
            str_array(vec![
                article
                    .license
                    .as_ref()
                    .and_then(|license| license.name.clone()),
            ]),
            str_array(vec![
                article
                    .license
                    .as_ref()
                    .and_then(|license| license.url.clone()),
            ]),
            str_array(vec![article.url_public_html.clone()]),
            u64_array(vec![Some(article.files.len() as u64)]),
            u64_array(vec![Some(total_size)]),
            str_array(vec![article.description.clone()]),
        ],
    )
    .map_err(|error| schedule_error("source_tabula_sapiens_release", error))
}

fn build_file_batch(rows: &[ArticleFile], version: u32) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        u32("version"),
        utf8("filename"),
        utf8("asset_type"),
        utf8("organ_or_compartment"),
        u64("size"),
        utf8("download_url"),
    ]));
    let mut types = Vec::new();
    let mut labels = Vec::new();
    for file in rows {
        let (asset_type, label) = crate::types::classify_file(&file.name);
        types.push(Some(asset_type.to_owned()));
        labels.push(label);
    }
    RecordBatch::try_new(
        schema,
        vec![
            u32_array(vec![Some(version); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(types),
            str_array(labels),
            u64_array(rows.iter().map(|row| row.size).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.download_url.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_tabula_sapiens_files", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_batches() {
        let version = crate::types::Version {
            version: 5,
            url: "https://api.figshare.com/v2/articles/14267219/versions/5".into(),
        };
        assert_eq!(build_version_batch(&[version]).unwrap().num_columns(), 2);
        let article = Article::default();
        assert_eq!(build_article_batch(&article).unwrap().num_columns(), 12);
        let file = ArticleFile {
            name: "TS_Lung.h5ad.zip".into(),
            size: Some(1),
            download_url: "https://example.test/lung".into(),
        };
        assert_eq!(build_file_batch(&[file], 5).unwrap().num_columns(), 6);
    }
}
