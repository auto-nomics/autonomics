use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OntologyTerm {
    pub label: String,
    pub ontology_term_id: String,
    #[serde(default)]
    pub tissue_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PublisherAuthor {
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub given: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PublisherMetadata {
    #[serde(default)]
    pub authors: Vec<PublisherAuthor>,
    #[serde(default)]
    pub is_preprint: Option<bool>,
    #[serde(default)]
    pub journal: Option<String>,
    #[serde(default)]
    pub published_at: Option<f64>,
    #[serde(default)]
    pub published_day: Option<u32>,
    #[serde(default)]
    pub published_month: Option<u32>,
    #[serde(default)]
    pub published_year: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CollectionLink {
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub link_type: String,
    #[serde(default)]
    pub link_url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DatasetAsset {
    #[serde(default)]
    pub created_at: Option<f64>,
    #[serde(default)]
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub filetype: Option<String>,
    pub id: String,
    #[serde(default)]
    pub s3_uri: String,
    #[serde(default)]
    pub updated_at: Option<f64>,
    #[serde(default)]
    pub user_submitted: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DatasetDeployment {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Dataset {
    #[serde(default)]
    pub assay: Vec<OntologyTerm>,
    #[serde(default)]
    pub cell_count: Option<u64>,
    #[serde(default)]
    pub cell_type: Vec<OntologyTerm>,
    #[serde(default)]
    pub cell_type_ancestors: Vec<String>,
    pub collection_id: String,
    #[serde(default)]
    pub created_at: Option<f64>,
    #[serde(default)]
    pub dataset_assets: Vec<DatasetAsset>,
    #[serde(default)]
    pub dataset_deployments: Vec<DatasetDeployment>,
    #[serde(default)]
    pub development_stage: Vec<OntologyTerm>,
    #[serde(default)]
    pub development_stage_ancestors: Vec<String>,
    #[serde(default)]
    pub disease: Vec<OntologyTerm>,
    #[serde(default)]
    pub donor_id: Vec<String>,
    #[serde(default)]
    pub explorer_url: Option<String>,
    pub id: String,
    #[serde(default)]
    pub is_primary_data: Option<String>,
    #[serde(default)]
    pub is_valid: Option<bool>,
    #[serde(default)]
    pub mean_genes_per_cell: Option<f64>,
    pub name: String,
    #[serde(default)]
    pub organism: Vec<OntologyTerm>,
    #[serde(default)]
    pub primary_cell_count: Option<u64>,
    #[serde(default)]
    pub processing_status: Option<Value>,
    #[serde(default)]
    pub published: Option<bool>,
    #[serde(default)]
    pub published_at: Option<f64>,
    #[serde(default)]
    pub revision: Option<u32>,
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub self_reported_ethnicity: Vec<OntologyTerm>,
    #[serde(default)]
    pub sex: Vec<OntologyTerm>,
    #[serde(default)]
    pub suspension_type: Vec<String>,
    #[serde(default)]
    pub tissue: Vec<OntologyTerm>,
    #[serde(default)]
    pub tissue_ancestors: Vec<String>,
    #[serde(default)]
    pub tombstone: Option<bool>,
    #[serde(default)]
    pub updated_at: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Collection {
    #[serde(default)]
    pub consortia: Vec<String>,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub published_at: Option<f64>,
    #[serde(default)]
    pub publisher_metadata: Option<PublisherMetadata>,
    #[serde(default)]
    pub revised_at: Option<f64>,
    #[serde(default)]
    pub summary_citation: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CollectionDetail {
    #[serde(default)]
    pub access_type: Option<String>,
    #[serde(default)]
    pub consortia: Vec<String>,
    #[serde(default)]
    pub contact_email: Option<String>,
    #[serde(default)]
    pub contact_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<f64>,
    #[serde(default)]
    pub curator_name: Option<String>,
    #[serde(default)]
    pub data_submission_policy_version: Option<String>,
    #[serde(default)]
    pub datasets: Vec<Dataset>,
    #[serde(default)]
    pub description: Option<String>,
    pub id: String,
    #[serde(default)]
    pub is_pre_analysis: Option<bool>,
    #[serde(default)]
    pub links: Vec<CollectionLink>,
    pub name: String,
    #[serde(default)]
    pub published_at: Option<f64>,
    #[serde(default)]
    pub publisher_metadata: Option<PublisherMetadata>,
    #[serde(default)]
    pub summary_citation: Option<String>,
    #[serde(default)]
    pub updated_at: Option<f64>,
    #[serde(default)]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DatasetFilter {
    pub collection_id: Option<String>,
    pub query: Option<String>,
    pub organism: Option<String>,
    pub tissue: Option<String>,
    pub disease: Option<String>,
    pub cell_type: Option<String>,
    pub assay: Option<String>,
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AssetResponse {
    #[serde(default)]
    pub assets: Vec<DatasetAsset>,
}

pub fn filter_datasets(datasets: &[Dataset], filter: &DatasetFilter) -> Vec<Dataset> {
    let query = normalized(&filter.query);
    let collection_id = normalized(&filter.collection_id);
    let organism = normalized(&filter.organism);
    let tissue = normalized(&filter.tissue);
    let disease = normalized(&filter.disease);
    let cell_type = normalized(&filter.cell_type);
    let assay = normalized(&filter.assay);

    let mut result: Vec<Dataset> = datasets
        .iter()
        .filter(|dataset| {
            collection_id
                .as_ref()
                .is_none_or(|value| value.eq_ignore_ascii_case(dataset.collection_id.trim()))
                && query
                    .as_ref()
                    .is_none_or(|value| matches_query(dataset, value))
                && ontology_matches(&dataset.organism, organism.as_deref())
                && ontology_matches(&dataset.tissue, tissue.as_deref())
                && ontology_matches(&dataset.disease, disease.as_deref())
                && ontology_matches(&dataset.cell_type, cell_type.as_deref())
                && ontology_matches(&dataset.assay, assay.as_deref())
        })
        .cloned()
        .collect();

    if let Some(max_results) = filter.max_results {
        result.truncate(max_results.max(1));
    }
    result
}

fn normalized(value: &Option<String>) -> Option<String> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

fn matches_query(dataset: &Dataset, query: &str) -> bool {
    let mut haystack = format!("{} {}", dataset.id, dataset.name);
    for term in dataset
        .organism
        .iter()
        .chain(dataset.tissue.iter())
        .chain(dataset.disease.iter())
        .chain(dataset.cell_type.iter())
        .chain(dataset.assay.iter())
    {
        haystack.push_str(&term.label);
        haystack.push_str(&term.ontology_term_id);
    }
    haystack.to_ascii_lowercase().contains(query)
}

fn ontology_matches(terms: &[OntologyTerm], needle: Option<&str>) -> bool {
    let Some(needle) = needle else {
        return true;
    };
    terms.iter().any(|term| {
        term.label.to_ascii_lowercase().contains(needle)
            || term.ontology_term_id.to_ascii_lowercase().contains(needle)
    })
}
