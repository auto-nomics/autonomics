//! SCP portal source nodes.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::ScpClient;
use crate::nodes::util::{
    bool_array, dataframe_output, dataframe_port, request_error, schedule_error, str_array, u64,
    u64_array, utf8,
};
use crate::types::{SearchResponse, SiteStudy, StudyDetail, StudyFile};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_scp_factory {
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
pub struct ScpStudiesSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Clone)]
pub struct ScpStudiesNode {
    meta: NodePorts,
    spec: ScpStudiesSpec,
}

pub struct ScpStudiesNodeFactory;

impl_scp_factory!(
    ScpStudiesNodeFactory,
    ScpStudiesNode,
    ScpStudiesSpec,
    "source_scp_studies",
    "Lists public Broad Single Cell Portal studies.",
    "A source node over the public site/studies endpoint. It supports local \
     query filtering and row limits, and reports accession, name, description, \
     public/detached flags, cell count, and gene count."
);

#[async_trait]
impl DagNode for ScpStudiesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scp_studies"
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
        let filter = crate::types::SiteStudyFilter {
            query: self.spec.query.clone(),
            max_results: self.spec.max_results.map(|value| value as usize),
        };
        let studies = ScpClient::new()
            .site_studies(&filter)
            .await
            .map_err(|error| request_error(error, "source_scp_studies"))?;
        let batch = build_study_summary_batch(&studies)?;
        dataframe_output(ctx, batch, "source_scp_studies")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct ScpSearchSpec {
    pub terms: String,
    /// `study` or `gene`; defaults to `study`.
    #[serde(default)]
    pub search_type: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
}

#[derive(Clone)]
pub struct ScpSearchNode {
    meta: NodePorts,
    spec: ScpSearchSpec,
}

pub struct ScpSearchNodeFactory;

impl_scp_factory!(
    ScpSearchNodeFactory,
    ScpSearchNode,
    ScpSearchSpec,
    "source_scp_search",
    "Searches Broad Single Cell Portal studies or genes.",
    "A source node over the public search endpoint. Output includes pagination \
     totals, matching accessions, relevance metadata, and one row per returned \
     study. Gene searches still emit the study hits that contain those genes."
);

#[async_trait]
impl DagNode for ScpSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scp_search"
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
        let response = ScpClient::new()
            .search(
                self.spec.search_type.as_deref().unwrap_or("study"),
                &self.spec.terms,
                self.spec.page,
            )
            .await
            .map_err(|error| request_error(error, "source_scp_search"))?;
        let batch = build_search_batch(&response)?;
        dataframe_output(ctx, batch, "source_scp_search")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct ScpStudySpec {
    pub accession: String,
}

#[derive(Clone)]
pub struct ScpStudyNode {
    meta: NodePorts,
    spec: ScpStudySpec,
}

pub struct ScpStudyNodeFactory;

impl_scp_factory!(
    ScpStudyNodeFactory,
    ScpStudyNode,
    ScpStudySpec,
    "source_scp_study",
    "Fetches one Broad Single Cell Portal study.",
    "Resolves a public SCP accession through site/studies/{accession}. The \
     row includes study metadata, publication/external-resource JSON, file \
     counts, and directory-listing JSON without downloading any files."
);

#[async_trait]
impl DagNode for ScpStudyNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scp_study"
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
        let study = ScpClient::new()
            .study(&self.spec.accession)
            .await
            .map_err(|error| request_error(error, "source_scp_study"))?;
        let batch = build_study_detail_batch(&study)?;
        dataframe_output(ctx, batch, "source_scp_study")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct ScpFilesSpec {
    pub accession: String,
    /// Optional file type filter, such as `Expression Matrix`, `Metadata`, or `Cluster`.
    #[serde(default)]
    pub file_type: Option<String>,
}

#[derive(Clone)]
pub struct ScpFilesNode {
    meta: NodePorts,
    spec: ScpFilesSpec,
}

pub struct ScpFilesNodeFactory;

impl_scp_factory!(
    ScpFilesNodeFactory,
    ScpFilesNode,
    ScpFilesSpec,
    "source_scp_files",
    "Lists Broad Single Cell Portal study files without downloading them.",
    "Fetches study metadata and emits one row per StudyFile. Columns include \
     file type, description, size, bucket location, signed download URL, and \
     streaming URL. File transfer remains a separate download workflow."
);

#[async_trait]
impl DagNode for ScpFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scp_files"
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
        let study = ScpClient::new()
            .study(&self.spec.accession)
            .await
            .map_err(|error| request_error(error, "source_scp_files"))?;
        let mut files = study.study_files.clone();
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
        let batch = build_file_batch(&files, &study.accession)?;
        dataframe_output(ctx, batch, "source_scp_files")
    }
}

fn build_study_summary_batch(rows: &[SiteStudy]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("accession"),
        utf8("name"),
        utf8("description"),
        arrow_schema::Field::new("public", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("detached", arrow_schema::DataType::Boolean, true),
        u64("cell_count"),
        u64("gene_count"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.accession.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.description.clone()))
                    .collect(),
            ),
            bool_array(rows.iter().map(|row| row.public).collect()),
            bool_array(rows.iter().map(|row| row.detached).collect()),
            u64_array(rows.iter().map(|row| row.cell_count).collect()),
            u64_array(rows.iter().map(|row| row.gene_count).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_scp_studies", error))
}

fn build_search_batch(response: &SearchResponse) -> Result<RecordBatch, DagError> {
    let rows = &response.studies;
    let count = rows.len();
    let schema = Arc::new(Schema::new(vec![
        utf8("search_type"),
        utf8("terms"),
        utf8("current_page"),
        utf8("total_pages"),
        utf8("total_studies"),
        utf8("source"),
        utf8("accession"),
        utf8("name"),
        utf8("description"),
        utf8("study_url"),
        utf8("term_matches_json"),
        utf8("facet_matches_json"),
        u64("cell_count"),
        u64("gene_count"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(response.search_type.clone()); count]),
            str_array(vec![Some(response.terms.clone()); count]),
            str_array(vec![
                response.current_page.map(|value| value.to_string());
                count
            ]),
            str_array(vec![
                response.total_pages.map(|value| value.to_string());
                count
            ]),
            str_array(vec![
                response.total_studies.map(|value| value.to_string());
                count
            ]),
            str_array(rows.iter().map(|row| row.source.clone()).collect()),
            str_array(rows.iter().map(|row| Some(row.accession.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.description.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.study_url.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| serde_json::to_string(&row.term_matches).ok())
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| serde_json::to_string(&row.facet_matches).ok())
                    .collect(),
            ),
            u64_array(rows.iter().map(|row| row.cell_count).collect()),
            u64_array(rows.iter().map(|row| row.gene_count).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_scp_search", error))
}

fn build_study_detail_batch(study: &StudyDetail) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("accession"),
        utf8("name"),
        utf8("description"),
        utf8("full_description"),
        arrow_schema::Field::new("public", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("detached", arrow_schema::DataType::Boolean, true),
        u64("cell_count"),
        u64("gene_count"),
        u64("file_count"),
        u64("directory_count"),
        u64("publication_count"),
        u64("external_resource_count"),
        utf8("files_json"),
        utf8("directories_json"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(study.accession.clone())]),
            str_array(vec![Some(study.name.clone())]),
            str_array(vec![study.description.clone()]),
            str_array(vec![study.full_description.clone()]),
            bool_array(vec![study.public]),
            bool_array(vec![study.detached]),
            u64_array(vec![study.cell_count]),
            u64_array(vec![study.gene_count]),
            u64_array(vec![Some(study.study_files.len() as u64)]),
            u64_array(vec![Some(study.directory_listings.len() as u64)]),
            u64_array(vec![Some(study.publications.len() as u64)]),
            u64_array(vec![Some(study.external_resources.len() as u64)]),
            str_array(vec![serde_json::to_string(&study.study_files).ok()]),
            str_array(vec![serde_json::to_string(&study.directory_listings).ok()]),
        ],
    )
    .map_err(|error| schedule_error("source_scp_study", error))
}

fn build_file_batch(rows: &[StudyFile], accession: &str) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("accession"),
        utf8("filename"),
        utf8("file_type"),
        utf8("description"),
        utf8("size_bytes"),
        utf8("bucket_location"),
        utf8("download_url"),
        utf8("media_url"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(accession.to_owned()); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(rows.iter().map(|row| row.file_type.clone()).collect()),
            str_array(rows.iter().map(|row| row.description.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.size.map(|value| value.to_string()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| row.bucket_location.clone()).collect()),
            str_array(rows.iter().map(|row| row.download_url.clone()).collect()),
            str_array(rows.iter().map(|row| row.media_url.clone()).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_scp_files", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_site_studies() {
        let rows = vec![SiteStudy {
            accession: "SCP1".into(),
            name: "Hippocampus".into(),
            description: "mouse brain".into(),
            ..Default::default()
        }];
        let filter = crate::types::SiteStudyFilter {
            query: Some("hippocampus".into()),
            ..Default::default()
        };
        assert_eq!(crate::types::filter_site_studies(&rows, &filter).len(), 1);
    }

    #[test]
    fn builds_all_batches() {
        let study = SiteStudy::default();
        assert_eq!(
            build_study_summary_batch(&[study]).unwrap().num_columns(),
            7
        );
        let response = SearchResponse::default();
        assert_eq!(build_search_batch(&response).unwrap().num_columns(), 14);
        let detail = StudyDetail::default();
        assert_eq!(build_study_detail_batch(&detail).unwrap().num_columns(), 14);
        let file = StudyFile::default();
        assert_eq!(build_file_batch(&[file], "SCP1").unwrap().num_columns(), 8);
    }
}
