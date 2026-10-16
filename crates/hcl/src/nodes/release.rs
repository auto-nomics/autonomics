//! Nodes over the official Human Cell Landscape figshare release.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::HclClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, request_error, schedule_error, str_array, u32, u32_array,
    u64, u64_array, utf8,
};
use crate::types::{Article, ArticleFile};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_hcl_factory {
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
pub struct HclVersionsSpec {}

#[derive(Clone)]
pub struct HclVersionsNode {
    meta: NodePorts,
    #[allow(dead_code)]
    spec: HclVersionsSpec,
}

pub struct HclVersionsNodeFactory;

impl_hcl_factory!(
    HclVersionsNodeFactory,
    HclVersionsNode,
    HclVersionsSpec,
    "source_hcl_versions",
    "Lists Human Cell Landscape release versions.",
    "Queries the figshare versions endpoint for the official HCL DGE Data \
     article. Each row contains a version number and the version-specific \
     API URL."
);

#[async_trait]
impl DagNode for HclVersionsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hcl_versions"
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
        let versions = HclClient::new()
            .versions()
            .await
            .map_err(|error| request_error(error, "source_hcl_versions"))?;
        let batch = build_version_batch(&versions)?;
        dataframe_output(ctx, batch, "source_hcl_versions")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HclReleaseSpec {
    #[serde(default)]
    pub version: Option<u32>,
}

#[derive(Clone)]
pub struct HclReleaseNode {
    meta: NodePorts,
    spec: HclReleaseSpec,
}

pub struct HclReleaseNodeFactory;

impl_hcl_factory!(
    HclReleaseNodeFactory,
    HclReleaseNode,
    HclReleaseSpec,
    "source_hcl_release",
    "Fetches metadata for one Human Cell Landscape release.",
    "Resolves the official HCL DGE Data figshare article, or a specified \
     version. Output includes paper, GEO, CNGB, portal, and code accessions \
     alongside DOI, version, dates, license, file count, and total size. No \
     matrices or archives are downloaded."
);

#[async_trait]
impl DagNode for HclReleaseNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hcl_release"
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
        let article = HclClient::new()
            .article(self.spec.version)
            .await
            .map_err(|error| request_error(error, "source_hcl_release"))?;
        let batch = build_article_batch(&article)?;
        dataframe_output(ctx, batch, "source_hcl_release")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HclFilesSpec {
    #[serde(default)]
    pub version: Option<u32>,
    /// H5AD, DGE archive, metadata, marker, or annotation asset type.
    #[serde(default)]
    pub asset_type: Option<String>,
}

#[derive(Clone)]
pub struct HclFilesNode {
    meta: NodePorts,
    spec: HclFilesSpec,
}

pub struct HclFilesNodeFactory;

impl_hcl_factory!(
    HclFilesNodeFactory,
    HclFilesNode,
    HclFilesSpec,
    "source_hcl_files",
    "Lists Human Cell Landscape release files without downloading them.",
    "Emits one row per official figshare file. Rows include filename, asset \
     classification, size, MD5 checksum, download URL, and release version. \
     Asset filtering is available without fetching file content."
);

#[async_trait]
impl DagNode for HclFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hcl_files"
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
        let (article, files) = HclClient::new()
            .files(self.spec.version, self.spec.asset_type.as_deref())
            .await
            .map_err(|error| request_error(error, "source_hcl_files"))?;
        let batch = build_file_batch(&files, article.version)?;
        dataframe_output(ctx, batch, "source_hcl_files")
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
    .map_err(|error| schedule_error("source_hcl_versions", error))
}

fn build_article_batch(article: &Article) -> Result<RecordBatch, DagError> {
    let total_size: u64 = article.files.iter().filter_map(|file| file.size).sum();
    let schema = Arc::new(Schema::new(vec![
        u64("article_id"),
        u32("version"),
        utf8("title"),
        utf8("doi"),
        utf8("paper_doi"),
        utf8("geo_accession"),
        utf8("cngb_accession"),
        utf8("portal_url"),
        utf8("code_url"),
        utf8("published_date"),
        utf8("modified_date"),
        utf8("license_name"),
        utf8("license_url"),
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
            str_array(vec![Some("10.1038/s41586-020-2157-4".into())]),
            str_array(vec![Some("GSE134355".into())]),
            str_array(vec![Some("CNP0000325".into())]),
            str_array(vec![Some("https://bis.zju.edu.cn/HCL/".into())]),
            str_array(vec![Some("https://github.com/ggjlab/HCL/".into())]),
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
            u64_array(vec![Some(article.files.len() as u64)]),
            u64_array(vec![Some(total_size)]),
            str_array(vec![article.description.clone()]),
        ],
    )
    .map_err(|error| schedule_error("source_hcl_release", error))
}

fn build_file_batch(rows: &[ArticleFile], version: u32) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        u32("version"),
        utf8("filename"),
        utf8("asset_type"),
        u64("size"),
        utf8("supplied_md5"),
        utf8("download_url"),
    ]));
    let mut types = Vec::new();
    for file in rows {
        types.push(Some(crate::types::classify_file(&file.name).to_owned()));
    }
    RecordBatch::try_new(
        schema,
        vec![
            u32_array(vec![Some(version); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(types),
            u64_array(rows.iter().map(|row| row.size).collect()),
            str_array(rows.iter().map(|row| row.supplied_md5.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.download_url.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_hcl_files", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_batches() {
        let version = crate::types::Version {
            version: 4,
            url: "https://api.figshare.com/v2/articles/7235471/versions/4".into(),
        };
        assert_eq!(build_version_batch(&[version]).unwrap().num_columns(), 2);
        assert_eq!(
            build_article_batch(&Article::default())
                .unwrap()
                .num_columns(),
            16
        );
        let file = ArticleFile {
            name: "HCL_Fig1_adata.h5ad".into(),
            size: Some(830_846_460),
            download_url: "https://ndownloader.figshare.com/files/17727365".into(),
            supplied_md5: Some("6c3dfe431bb5d402b86c55f72ae729e6".into()),
        };
        assert_eq!(build_file_batch(&[file], 4).unwrap().num_columns(), 6);
    }
}
