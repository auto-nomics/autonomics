//! HCA Azul query source nodes.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::HcaClient;
use crate::nodes::util::{
    bool_array, dataframe_output, dataframe_port, request_error, schedule_error, str_array,
    u64_array, utf8,
};
use crate::types::{
    FileHit, ProjectHit, ProjectSummary, RepositoryFile, RepositorySample, SampleHit,
};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_hca_factory {
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
pub struct HcaProjectsSpec {
    /// Case-insensitive text filter over project ID, title, short name, and description.
    #[serde(default)]
    pub query: Option<String>,
    /// Maximum project rows retained.
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Clone)]
pub struct HcaProjectsNode {
    meta: NodePorts,
    spec: HcaProjectsSpec,
}

pub struct HcaProjectsNodeFactory;

impl_hca_factory!(
    HcaProjectsNodeFactory,
    HcaProjectsNode,
    HcaProjectsSpec,
    "source_hca_projects",
    "Searches Human Cell Atlas projects and emits project metadata.",
    "A source node over the HCA Azul `/index/projects` endpoint. It follows \
     all official pagination pages, applies an optional local text filter, \
     and returns project title, cell counts, species, organ, assay, accessions, \
     publication/matrices JSON, and update dates."
);

#[async_trait]
impl DagNode for HcaProjectsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hca_projects"
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
        let hits = HcaClient::new()
            .projects(
                self.spec.query.as_deref(),
                self.spec.max_results.map(|value| value as usize),
            )
            .await
            .map_err(|error| request_error(error, "source_hca_projects"))?;
        let batch = build_project_batch(&hits)?;
        dataframe_output(ctx, batch, "source_hca_projects")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HcaProjectSpec {
    /// HCA project UUID.
    pub project_id: String,
}

#[derive(Clone)]
pub struct HcaProjectNode {
    meta: NodePorts,
    spec: HcaProjectSpec,
}

pub struct HcaProjectNodeFactory;

impl_hca_factory!(
    HcaProjectNodeFactory,
    HcaProjectNode,
    HcaProjectSpec,
    "source_hca_project",
    "Fetches one Human Cell Atlas project metadata record.",
    "Resolves a project UUID through `/index/projects/{project_id}`. The row \
     schema matches source_hca_projects; no matrices or other files are fetched."
);

#[async_trait]
impl DagNode for HcaProjectNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hca_project"
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
        let hit = HcaClient::new()
            .project(&self.spec.project_id)
            .await
            .map_err(|error| request_error(error, "source_hca_project"))?;
        let batch = build_project_batch(&[hit])?;
        dataframe_output(ctx, batch, "source_hca_project")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HcaFilesSpec {
    /// Restrict files to this HCA project UUID.
    #[serde(default)]
    pub project_id: Option<String>,
    /// File format filter, such as `h5`, `h5ad`, `fastq.gz`, or `bam`.
    #[serde(default)]
    pub file_format: Option<String>,
    /// Maximum index rows requested from Azul (default 50, max 50).
    #[serde(default)]
    pub size: Option<u32>,
}

#[derive(Clone)]
pub struct HcaFilesNode {
    meta: NodePorts,
    spec: HcaFilesSpec,
}

pub struct HcaFilesNodeFactory;

impl_hca_factory!(
    HcaFilesNodeFactory,
    HcaFilesNode,
    HcaFilesSpec,
    "source_hca_files",
    "Indexes Human Cell Atlas repository files without downloading them.",
    "A source node over HCA Azul `/index/files`. Supports official project \
     and file-format filters. Output rows include checksum, size, DRS URI, \
     Azul URL, S3 mirror URI, project linkage, and accessibility."
);

#[async_trait]
impl DagNode for HcaFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hca_files"
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
        let (hits, _) = HcaClient::new()
            .files(
                self.spec.project_id.as_deref(),
                self.spec.file_format.as_deref(),
                self.spec.size.unwrap_or(50),
            )
            .await
            .map_err(|error| request_error(error, "source_hca_files"))?;
        let batch = build_file_batch(&hits)?;
        dataframe_output(ctx, batch, "source_hca_files")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HcaSamplesSpec {
    /// Restrict samples to this HCA project UUID.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Maximum index rows requested from Azul (default 50, max 50).
    #[serde(default)]
    pub size: Option<u32>,
}

#[derive(Clone)]
pub struct HcaSamplesNode {
    meta: NodePorts,
    spec: HcaSamplesSpec,
}

pub struct HcaSamplesNodeFactory;

impl_hca_factory!(
    HcaSamplesNodeFactory,
    HcaSamplesNode,
    HcaSamplesSpec,
    "source_hca_samples",
    "Indexes Human Cell Atlas samples as a table.",
    "A source node over HCA Azul `/index/samples`, with an official project \
     filter. Output includes sample ID/name, entity type, effective and model \
     organ, project linkage, and accessibility."
);

#[async_trait]
impl DagNode for HcaSamplesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_hca_samples"
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
        let (hits, _) = HcaClient::new()
            .samples(
                self.spec.project_id.as_deref(),
                self.spec.size.unwrap_or(50),
            )
            .await
            .map_err(|error| request_error(error, "source_hca_samples"))?;
        let batch = build_sample_batch(&hits)?;
        dataframe_output(ctx, batch, "source_hca_samples")
    }
}

fn joined(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join("; "))
}

fn json<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn present(values: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    values
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn optional_json(value: &Option<serde_json::Value>) -> Option<String> {
    value.as_ref().and_then(json)
}

fn summary_project_id(project: &ProjectSummary) -> Option<String> {
    project.project_id.iter().flatten().next().cloned()
}

fn summary_project_title(project: &ProjectSummary) -> Option<String> {
    project.title.iter().flatten().next().cloned()
}

fn build_project_batch(rows: &[ProjectHit]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("project_id"),
        utf8("title"),
        utf8("shortname"),
        utf8("description"),
        utf8("laboratory"),
        utf8("data_use_restriction"),
        arrow_schema::Field::new("estimated_cell_count", arrow_schema::DataType::UInt64, true),
        arrow_schema::Field::new("donor_count", arrow_schema::DataType::UInt64, true),
        utf8("species"),
        utf8("development_stage"),
        utf8("organ"),
        utf8("disease"),
        utf8("library_method"),
        utf8("instrument_model"),
        utf8("file_types_json"),
        utf8("accessions_json"),
        utf8("publications_json"),
        utf8("matrices_json"),
        utf8("contributed_analyses_json"),
        utf8("supplementary_links_json"),
        utf8("submission_date"),
        utf8("update_date"),
    ]));

    let mut project_ids = Vec::new();
    let mut titles = Vec::new();
    let mut shortnames = Vec::new();
    let mut descriptions = Vec::new();
    let mut laboratories = Vec::new();
    let mut data_use_restrictions = Vec::new();
    let mut estimated_cell_counts = Vec::new();
    let mut donor_counts = Vec::new();
    let mut species = Vec::new();
    let mut development_stages = Vec::new();
    let mut organs = Vec::new();
    let mut diseases = Vec::new();
    let mut library_methods = Vec::new();
    let mut instrument_models = Vec::new();
    let mut file_types = Vec::new();
    let mut accessions = Vec::new();
    let mut publications = Vec::new();
    let mut matrices = Vec::new();
    let mut contributed_analyses = Vec::new();
    let mut supplementary_links = Vec::new();
    let mut submission_dates = Vec::new();
    let mut update_dates = Vec::new();

    for hit in rows {
        let Some(project) = hit.projects.first() else {
            continue;
        };
        project_ids.push(Some(project.project_id.clone()));
        titles.push(Some(project.title.clone()));
        shortnames.push(Some(project.shortname.clone()));
        descriptions.push(project.project_description.clone());
        laboratories.push(joined(
            &project
                .laboratory
                .iter()
                .filter_map(|value| value.clone())
                .collect::<Vec<_>>(),
        ));
        data_use_restrictions.push(project.data_use_restriction.clone());
        estimated_cell_counts.push(project.estimated_cell_count);
        donor_counts.push(Some(
            hit.donor_organisms
                .iter()
                .filter_map(|donor| donor.donor_count)
                .sum(),
        ));
        let species_values = present(
            hit.donor_organisms
                .iter()
                .flat_map(|donor| donor.genus_species.iter().cloned()),
        );
        species.push(unique_joined(species_values.as_slice()));
        let development_values = present(
            hit.donor_organisms
                .iter()
                .flat_map(|donor| donor.development_stage.iter().cloned()),
        );
        development_stages.push(unique_joined(development_values.as_slice()));
        let organ_values = present(
            hit.specimens
                .iter()
                .flat_map(|specimen| specimen.organ.iter().cloned()),
        );
        organs.push(unique_joined(organ_values.as_slice()));
        let disease_values = present(
            hit.specimens
                .iter()
                .flat_map(|specimen| specimen.disease.iter().cloned())
                .chain(
                    hit.donor_organisms
                        .iter()
                        .flat_map(|donor| donor.disease.iter().cloned()),
                ),
        );
        diseases.push(unique_joined(disease_values.as_slice()));
        let library_values = present(
            hit.protocols
                .iter()
                .flat_map(|protocol| protocol.library_construction_approach.iter().cloned()),
        );
        library_methods.push(unique_joined(library_values.as_slice()));
        let instrument_values = present(
            hit.protocols
                .iter()
                .flat_map(|protocol| protocol.instrument_model.iter().cloned()),
        );
        instrument_models.push(unique_joined(instrument_values.as_slice()));
        file_types.push(json(&hit.file_type_summaries));
        accessions.push(optional_json(&project.accessions));
        publications.push(optional_json(&project.publications));
        matrices.push(optional_json(&project.matrices));
        contributed_analyses.push(optional_json(&project.contributed_analyses));
        supplementary_links.push(optional_json(&project.supplementary_links));
        submission_dates.push(
            hit.dates
                .first()
                .and_then(|date| date.submission_date.clone()),
        );
        update_dates.push(hit.dates.first().and_then(|date| date.update_date.clone()));
    }

    RecordBatch::try_new(
        schema,
        vec![
            str_array(project_ids),
            str_array(titles),
            str_array(shortnames),
            str_array(descriptions),
            str_array(laboratories),
            str_array(data_use_restrictions),
            u64_array(estimated_cell_counts),
            u64_array(donor_counts),
            str_array(species),
            str_array(development_stages),
            str_array(organs),
            str_array(diseases),
            str_array(library_methods),
            str_array(instrument_models),
            str_array(file_types),
            str_array(accessions),
            str_array(publications),
            str_array(matrices),
            str_array(contributed_analyses),
            str_array(supplementary_links),
            str_array(submission_dates),
            str_array(update_dates),
        ],
    )
    .map_err(|error| schedule_error("source_hca_projects", error))
}

fn unique_joined(values: &[String]) -> Option<String> {
    let mut cleaned: Vec<String> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    cleaned.sort();
    cleaned.dedup();
    joined(&cleaned)
}

fn build_file_batch(rows: &[FileHit]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("project_id"),
        utf8("project_title"),
        utf8("file_uuid"),
        utf8("name"),
        utf8("format"),
        utf8("version"),
        arrow_schema::Field::new("size", arrow_schema::DataType::UInt64, true),
        utf8("sha256"),
        utf8("content_description"),
        utf8("file_source"),
        utf8("drs_uri"),
        utf8("azul_url"),
        utf8("azul_mirror_uri"),
        arrow_schema::Field::new("matrix_cell_count", arrow_schema::DataType::UInt64, true),
        arrow_schema::Field::new("accessible", arrow_schema::DataType::Boolean, true),
    ]));

    let mut project_ids = Vec::new();
    let mut project_titles = Vec::new();
    let mut file_uuids = Vec::new();
    let mut names = Vec::new();
    let mut formats = Vec::new();
    let mut versions = Vec::new();
    let mut sizes = Vec::new();
    let mut hashes = Vec::new();
    let mut descriptions = Vec::new();
    let mut file_sources = Vec::new();
    let mut drs_uris = Vec::new();
    let mut azul_urls = Vec::new();
    let mut mirror_uris = Vec::new();
    let mut matrix_cell_counts = Vec::new();
    let mut accessible = Vec::new();

    for hit in rows {
        let project_id = hit.projects.first().and_then(summary_project_id);
        let project_title = hit.projects.first().and_then(summary_project_title);
        for file in &hit.files {
            push_file_row(
                &mut project_ids,
                &mut project_titles,
                &mut file_uuids,
                &mut names,
                &mut formats,
                &mut versions,
                &mut sizes,
                &mut hashes,
                &mut descriptions,
                &mut file_sources,
                &mut drs_uris,
                &mut azul_urls,
                &mut mirror_uris,
                &mut matrix_cell_counts,
                &mut accessible,
                project_id.clone(),
                project_title.clone(),
                file,
            );
        }
    }

    RecordBatch::try_new(
        schema,
        vec![
            str_array(project_ids),
            str_array(project_titles),
            str_array(file_uuids),
            str_array(names),
            str_array(formats),
            str_array(versions),
            u64_array(sizes),
            str_array(hashes),
            str_array(descriptions),
            str_array(file_sources),
            str_array(drs_uris),
            str_array(azul_urls),
            str_array(mirror_uris),
            u64_array(matrix_cell_counts),
            bool_array(accessible),
        ],
    )
    .map_err(|error| schedule_error("source_hca_files", error))
}

#[allow(clippy::too_many_arguments)]
fn push_file_row(
    project_ids: &mut Vec<Option<String>>,
    project_titles: &mut Vec<Option<String>>,
    file_uuids: &mut Vec<Option<String>>,
    names: &mut Vec<Option<String>>,
    formats: &mut Vec<Option<String>>,
    versions: &mut Vec<Option<String>>,
    sizes: &mut Vec<Option<u64>>,
    hashes: &mut Vec<Option<String>>,
    descriptions: &mut Vec<Option<String>>,
    file_sources: &mut Vec<Option<String>>,
    drs_uris: &mut Vec<Option<String>>,
    azul_urls: &mut Vec<Option<String>>,
    mirror_uris: &mut Vec<Option<String>>,
    matrix_cell_counts: &mut Vec<Option<u64>>,
    accessible: &mut Vec<Option<bool>>,
    project_id: Option<String>,
    project_title: Option<String>,
    file: &RepositoryFile,
) {
    project_ids.push(project_id);
    project_titles.push(project_title);
    file_uuids.push(Some(file.uuid.clone()));
    names.push(Some(file.name.clone()));
    formats.push(file.format.clone());
    versions.push(Some(file.version.clone()));
    sizes.push(file.size);
    hashes.push((!file.sha256.is_empty()).then(|| file.sha256.clone()));
    descriptions.push((!file.content_description.is_empty()).then(|| {
        file.content_description
            .iter()
            .filter_map(|value| value.clone())
            .collect::<Vec<_>>()
            .join("; ")
    }));
    file_sources.push(file.file_source.clone());
    drs_uris.push(file.drs_uri.clone());
    azul_urls.push(file.azul_url.clone());
    mirror_uris.push(file.azul_mirror_uri.clone());
    matrix_cell_counts.push(file.matrix_cell_count);
    accessible.push(file.accessible);
}

fn build_sample_batch(rows: &[SampleHit]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("project_id"),
        utf8("project_title"),
        utf8("sample_id"),
        utf8("sample_name"),
        utf8("entity_type"),
        utf8("effective_organ"),
        utf8("model_organ"),
        arrow_schema::Field::new("accessible", arrow_schema::DataType::Boolean, true),
    ]));
    let mut project_ids = Vec::new();
    let mut project_titles = Vec::new();
    let mut sample_ids = Vec::new();
    let mut sample_names = Vec::new();
    let mut entity_types = Vec::new();
    let mut effective_organs = Vec::new();
    let mut model_organs = Vec::new();
    let mut accessible = Vec::new();
    for hit in rows {
        let project_id = hit.projects.first().and_then(summary_project_id);
        let title = hit.projects.first().and_then(summary_project_title);
        for sample in &hit.samples {
            project_ids.push(project_id.clone());
            project_titles.push(title.clone());
            sample_ids.push(Some(sample.sample_id.clone()));
            sample_names.push(sample.sample_name.clone());
            entity_types.push(sample.sample_entity_type.clone());
            effective_organs.push(sample.effective_organ.clone());
            model_organs.push(sample.model_organ.clone());
            accessible.push(sample.accessible);
        }
    }
    RecordBatch::try_new(
        schema,
        vec![
            str_array(project_ids),
            str_array(project_titles),
            str_array(sample_ids),
            str_array(sample_names),
            str_array(entity_types),
            str_array(effective_organs),
            str_array(model_organs),
            bool_array(accessible),
        ],
    )
    .map_err(|error| schedule_error("source_hca_samples", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Project;

    #[test]
    fn builds_project_batch_with_nested_metadata() {
        let hit = ProjectHit {
            entry_id: "project-1".into(),
            projects: vec![Project {
                project_id: "project-1".into(),
                title: "Brain Cells".into(),
                shortname: "Brain".into(),
                estimated_cell_count: Some(100),
                ..Default::default()
            }],
            ..Default::default()
        };
        let batch = build_project_batch(&[hit]).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 22);
    }

    #[test]
    fn builds_file_and_sample_batches() {
        let file_hit = FileHit::default();
        assert_eq!(build_file_batch(&[file_hit]).unwrap().num_columns(), 15);
        let sample_hit = SampleHit::default();
        assert_eq!(build_sample_batch(&[sample_hit]).unwrap().num_columns(), 8);
    }
}
