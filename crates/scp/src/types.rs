use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SiteStudy {
    pub accession: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub public: Option<bool>,
    #[serde(default)]
    pub detached: Option<bool>,
    #[serde(default)]
    pub cell_count: Option<u64>,
    #[serde(default)]
    pub gene_count: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StudyFile {
    pub name: String,
    #[serde(default, rename = "file_type")]
    pub file_type: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "bucket_location")]
    pub bucket_location: Option<String>,
    #[serde(default, rename = "upload_file_size")]
    pub size: Option<u64>,
    #[serde(default, rename = "download_url")]
    pub download_url: Option<String>,
    #[serde(default, rename = "media_url")]
    pub media_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StudyDetail {
    pub accession: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "full_description")]
    pub full_description: Option<String>,
    #[serde(default)]
    pub public: Option<bool>,
    #[serde(default)]
    pub detached: Option<bool>,
    #[serde(default)]
    pub cell_count: Option<u64>,
    #[serde(default)]
    pub gene_count: Option<u64>,
    #[serde(default)]
    pub study_files: Vec<StudyFile>,
    #[serde(default)]
    pub directory_listings: Vec<Value>,
    #[serde(default)]
    pub publications: Vec<Value>,
    #[serde(default)]
    pub external_resources: Vec<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchStudy {
    #[serde(default, rename = "study_source")]
    pub source: Option<String>,
    pub accession: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub public: Option<bool>,
    #[serde(default)]
    pub detached: Option<bool>,
    #[serde(default)]
    pub cell_count: Option<u64>,
    #[serde(default)]
    pub gene_count: Option<u64>,
    #[serde(default, rename = "study_url")]
    pub study_url: Option<String>,
    #[serde(default, rename = "term_matches")]
    pub term_matches: Value,
    #[serde(default, rename = "facet_matches")]
    pub facet_matches: Value,
    #[serde(default, rename = "term_search_weight")]
    pub term_search_weight: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchResponse {
    #[serde(default, rename = "type")]
    pub search_type: String,
    #[serde(default)]
    pub terms: String,
    #[serde(default)]
    pub current_page: Option<u32>,
    #[serde(default)]
    pub total_studies: Option<u64>,
    #[serde(default)]
    pub total_pages: Option<u32>,
    #[serde(default)]
    pub matching_accessions: Vec<String>,
    #[serde(default)]
    pub studies: Vec<SearchStudy>,
}

#[derive(Debug, Clone, Default)]
pub struct SiteStudyFilter {
    pub query: Option<String>,
    pub max_results: Option<usize>,
}

pub fn filter_site_studies(studies: &[SiteStudy], filter: &SiteStudyFilter) -> Vec<SiteStudy> {
    let query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let mut result: Vec<SiteStudy> = studies
        .iter()
        .filter(|study| {
            query.as_ref().is_none_or(|query| {
                format!("{} {} {}", study.accession, study.name, study.description)
                    .to_ascii_lowercase()
                    .contains(query)
            })
        })
        .cloned()
        .collect();
    if let Some(max_results) = filter.max_results {
        result.truncate(max_results.max(1));
    }
    result
}
