use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Pagination {
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub size: Option<u32>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub pages: Option<u64>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub order: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Page<T> {
    #[serde(rename = "hits")]
    pub hits: Vec<T>,
    #[serde(default)]
    pub pagination: Pagination,
    #[serde(default, rename = "termFacets")]
    pub term_facets: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Project {
    #[serde(default, rename = "projectId")]
    pub project_id: String,
    #[serde(default, rename = "projectTitle")]
    pub title: String,
    #[serde(default, rename = "projectShortname")]
    pub shortname: String,
    #[serde(default)]
    pub laboratory: Vec<Option<String>>,
    #[serde(default, rename = "estimatedCellCount")]
    pub estimated_cell_count: Option<u64>,
    #[serde(default, rename = "projectDescription")]
    pub project_description: Option<String>,
    #[serde(default, rename = "dataUseRestriction")]
    pub data_use_restriction: Option<String>,
    #[serde(default)]
    pub contributors: Option<Value>,
    #[serde(default)]
    pub publications: Option<Value>,
    #[serde(default)]
    pub matrices: Option<Value>,
    #[serde(default, rename = "contributedAnalyses")]
    pub contributed_analyses: Option<Value>,
    #[serde(default)]
    pub accessions: Option<Value>,
    #[serde(default, rename = "supplementaryLinks")]
    pub supplementary_links: Option<Value>,
}

/// Project summary used inside non-project index hits. Azul represents
/// identifiers and titles as arrays in these endpoints.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProjectSummary {
    #[serde(default, rename = "projectId")]
    pub project_id: Vec<Option<String>>,
    #[serde(default, rename = "projectTitle")]
    pub title: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DonorOrganism {
    #[serde(default, rename = "donorCount")]
    pub donor_count: Option<u64>,
    #[serde(default, rename = "genusSpecies")]
    pub genus_species: Vec<Option<String>>,
    #[serde(default, rename = "developmentStage")]
    pub development_stage: Vec<Option<String>>,
    #[serde(default, rename = "biologicalSex")]
    pub biological_sex: Vec<Option<String>>,
    #[serde(default)]
    pub disease: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Specimen {
    #[serde(default)]
    pub organ: Vec<Option<String>>,
    #[serde(default, rename = "organPart")]
    pub organ_part: Vec<Option<String>>,
    #[serde(default)]
    pub disease: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Protocol {
    #[serde(default, rename = "libraryConstructionApproach")]
    pub library_construction_approach: Vec<Option<String>>,
    #[serde(default, rename = "nucleicAcidSource")]
    pub nucleic_acid_source: Vec<Option<String>>,
    #[serde(default, rename = "instrumentManufacturerModel")]
    pub instrument_model: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProjectDate {
    #[serde(default, rename = "submissionDate")]
    pub submission_date: Option<String>,
    #[serde(default, rename = "updateDate")]
    pub update_date: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FileTypeSummary {
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default, rename = "totalSize")]
    pub total_size: Option<u64>,
    #[serde(default, rename = "contentDescription")]
    pub content_description: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProjectHit {
    #[serde(default, rename = "entryId")]
    pub entry_id: String,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default, rename = "donorOrganisms")]
    pub donor_organisms: Vec<DonorOrganism>,
    #[serde(default)]
    pub specimens: Vec<Specimen>,
    #[serde(default)]
    pub protocols: Vec<Protocol>,
    #[serde(default)]
    pub dates: Vec<ProjectDate>,
    #[serde(default, rename = "fileTypeSummaries")]
    pub file_type_summaries: Vec<FileTypeSummary>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RepositoryFile {
    #[serde(default, rename = "contentDescription")]
    pub content_description: Vec<Option<String>>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default, rename = "fileSource")]
    pub file_source: Option<String>,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, rename = "matrixCellCount")]
    pub matrix_cell_count: Option<u64>,
    #[serde(default)]
    pub drs_uri: Option<String>,
    #[serde(default)]
    pub azul_url: Option<String>,
    #[serde(default)]
    pub azul_mirror_uri: Option<String>,
    #[serde(default)]
    pub accessible: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FileHit {
    #[serde(default, rename = "entryId")]
    pub entry_id: String,
    #[serde(default)]
    pub projects: Vec<ProjectSummary>,
    #[serde(default)]
    pub files: Vec<RepositoryFile>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RepositorySample {
    #[serde(default, rename = "sampleEntityType")]
    pub sample_entity_type: Option<String>,
    #[serde(default, rename = "sampleId")]
    pub sample_id: String,
    #[serde(default, rename = "sampleName")]
    pub sample_name: Option<String>,
    #[serde(default, rename = "effectiveOrgan")]
    pub effective_organ: Option<String>,
    #[serde(default, rename = "modelOrgan")]
    pub model_organ: Option<String>,
    #[serde(default)]
    pub accessible: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SampleHit {
    #[serde(default, rename = "entryId")]
    pub entry_id: String,
    #[serde(default)]
    pub projects: Vec<ProjectSummary>,
    #[serde(default)]
    pub samples: Vec<RepositorySample>,
}
