use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Pagination {
    #[serde(default)]
    pub total_results: Option<u64>,
    #[serde(default)]
    pub current_page: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub total_pages: Option<u32>,
    #[serde(default)]
    pub next_page: Option<u32>,
    #[serde(default)]
    pub prev_page: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Dataset {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub share_id: Option<String>,
    #[serde(default)]
    pub organism_id: Option<u64>,
    #[serde(default)]
    pub organism: Option<String>,
    #[serde(default)]
    pub pubmed_id: Option<String>,
    #[serde(default)]
    pub geo_id: Option<String>,
    #[serde(default)]
    pub is_public: Option<u8>,
    #[serde(default)]
    pub is_downloadable: Option<u8>,
    #[serde(default)]
    pub date_added: Option<String>,
    #[serde(default)]
    pub dtype: Option<String>,
    #[serde(default)]
    pub load_status: Option<String>,
    #[serde(default)]
    pub has_h5ad: Option<u8>,
    #[serde(default)]
    pub has_tarball: Option<u8>,
    #[serde(default)]
    pub annotation_release: Option<String>,
    #[serde(default)]
    pub gene_count: Option<u64>,
    #[serde(default)]
    pub obs_count: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Collection {
    pub label: String,
    pub share_id: String,
    #[serde(default)]
    pub dataset_count: u64,
    #[serde(default)]
    pub is_current: u8,
    #[serde(default)]
    pub is_domain: u8,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub success: u8,
    #[serde(default)]
    pub problem: String,
    #[serde(default)]
    pub datasets: Vec<Dataset>,
    #[serde(default)]
    pub pagination: Pagination,
}

#[derive(Debug, Default, Deserialize)]
pub struct CollectionsResponse {
    #[serde(default)]
    pub user_layouts: Vec<Collection>,
    #[serde(default)]
    pub domain_layouts: Vec<Collection>,
    #[serde(default)]
    pub shared_layouts: Vec<Collection>,
}
