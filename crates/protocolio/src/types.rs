use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Requested rendering of rich-text fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentFormat {
    Json,
    Html,
    #[default]
    Markdown,
}

impl ContentFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Markdown => "markdown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "html" => Some(Self::Html),
            "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Pagination {
    pub current_page: Option<u32>,
    pub total_pages: Option<u32>,
    pub total_results: Option<u64>,
    pub next_page: Option<String>,
    pub prev_page: Option<String>,
    pub page_size: Option<serde_json::Value>,
    pub first: Option<u64>,
    pub last: Option<u64>,
    pub changed_on: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct User {
    pub name: Option<String>,
    pub affiliation: Option<String>,
    pub username: Option<String>,
    pub link: Option<String>,
}

impl User {
    pub fn label(&self) -> Option<&str> {
        self.name.as_deref().or(self.username.as_deref())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProtocolStats {
    pub number_of_views: Option<u64>,
    pub number_of_steps: Option<u64>,
    pub number_of_bookmarks: Option<u64>,
    pub number_of_comments: Option<u64>,
    pub number_of_exports: Option<u64>,
    pub number_of_runs: Option<u64>,
    pub number_of_votes: Option<u64>,
    pub number_of_reagents: Option<u64>,
    pub number_of_equipments: Option<u64>,
    pub number_of_collections: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProtocolSummary {
    pub id: Option<i64>,
    pub guid: Option<String>,
    pub title: Option<String>,
    pub doi: Option<String>,
    pub uri: Option<String>,
    pub url: Option<String>,
    pub version_id: Option<u32>,
    pub version_uri: Option<String>,
    pub published_on: Option<i64>,
    pub created_on: Option<i64>,
    pub creator: Option<User>,
    pub authors: Vec<User>,
    pub public: Option<Value>,
    pub number_of_steps: Option<u64>,
    pub stats: Option<ProtocolStats>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Protocol {
    #[serde(flatten)]
    pub summary: ProtocolSummary,
    pub description: Option<Value>,
    pub before_start: Option<Value>,
    pub guidelines: Option<Value>,
    pub warning: Option<Value>,
    pub materials_text: Option<Value>,
    pub steps: Vec<ProtocolStep>,
    pub materials: Vec<Reagent>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProtocolStep {
    pub id: Option<i64>,
    pub guid: Option<String>,
    pub previous_id: Option<i64>,
    pub previous_guid: Option<String>,
    pub modified_on: Option<i64>,
    pub step: Option<Value>,
    pub components: Option<Value>,
    pub section: Option<Value>,
    pub section_color: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Reagent {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub mol_weight: Option<f64>,
    pub linfor: Option<String>,
    pub url: Option<String>,
    pub sku: Option<String>,
    pub vendor: Option<User>,
    pub is_citeab: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProtocolListResponse {
    pub items: Vec<ProtocolSummary>,
    pub pagination: Pagination,
    pub status_code: Option<u32>,
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolResponse {
    #[serde(default)]
    pub payload: Option<Protocol>,
    #[serde(default)]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub status_code: Option<u32>,
    #[serde(default)]
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StepsResponse {
    pub steps: Vec<ProtocolStep>,
    pub status_code: Option<u32>,
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MaterialsResponse {
    pub materials: Vec<Reagent>,
    pub status_code: Option<u32>,
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReagentListResponse {
    pub items: Vec<Reagent>,
    pub pagination: Pagination,
    pub status_code: Option<u32>,
    pub status_text: Option<String>,
}

impl ProtocolResponse {
    pub fn protocol(&self) -> Option<&Protocol> {
        self.payload.as_ref().or(self.protocol.as_ref())
    }
}
