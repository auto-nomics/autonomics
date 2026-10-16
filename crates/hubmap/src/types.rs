use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DatasetFile {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub edam_term: Option<String>,
    #[serde(default)]
    pub is_data_product: Option<bool>,
    #[serde(default)]
    pub is_qa_qc: Option<bool>,
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default, rename = "type")]
    pub file_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OriginSample {
    #[serde(default)]
    pub organ: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Entity {
    #[serde(default)]
    pub uuid: String,
    #[serde(default, rename = "hubmap_id")]
    pub hubmap_id: String,
    #[serde(default, rename = "entity_type")]
    pub entity_type: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "data_access_level")]
    pub data_access_level: Option<String>,
    #[serde(default, rename = "dataset_type")]
    pub dataset_type: Option<String>,
    #[serde(default, rename = "data_types")]
    pub data_types: Vec<String>,
    #[serde(default)]
    pub origin_samples: Vec<OriginSample>,
    #[serde(default)]
    pub donor: Option<Value>,
    #[serde(default)]
    pub donors: Option<Value>,
    #[serde(default)]
    pub contributors: Option<Value>,
    #[serde(default)]
    pub contacts: Option<Value>,
    #[serde(default)]
    pub source_samples: Option<Value>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default, rename = "ingest_metadata")]
    pub ingest_metadata: Option<Value>,
    #[serde(default)]
    pub group_name: Option<String>,
    #[serde(default, rename = "group_uuid")]
    pub group_uuid: Option<String>,
    #[serde(default, rename = "doi_url")]
    pub doi_url: Option<String>,
    #[serde(default, rename = "registered_doi")]
    pub registered_doi: Option<String>,
    #[serde(default)]
    pub files: Vec<DatasetFile>,
    #[serde(default, rename = "created_timestamp")]
    pub created_timestamp: Option<u64>,
    #[serde(default, rename = "last_modified_timestamp")]
    pub last_modified_timestamp: Option<u64>,
    #[serde(default, rename = "published_timestamp")]
    pub published_timestamp: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchHit {
    #[serde(default, rename = "_id")]
    pub id: String,
    #[serde(default, rename = "_score")]
    pub score: Option<f64>,
    #[serde(default, rename = "_source")]
    pub source: Entity,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchTotal {
    #[serde(default)]
    pub value: Option<u64>,
    #[serde(default)]
    pub relation: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchHits {
    #[serde(default)]
    pub total: Option<SearchTotal>,
    #[serde(default)]
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub took: Option<u64>,
    #[serde(default)]
    pub timed_out: Option<bool>,
    #[serde(default)]
    pub hits: SearchHits,
}

#[derive(Debug, Clone, Default)]
pub struct HubmapSearchFilter {
    pub query: Option<String>,
    pub entity_type: Option<String>,
    pub organ: Option<String>,
    pub assay: Option<String>,
    pub status: Option<String>,
    pub from: Option<u32>,
    pub size: u32,
}
