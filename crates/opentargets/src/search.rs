//! Search response types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One search hit. `object` holds the full entity record (shape varies by
/// `entity`) and is left as [`serde_json::Value`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `target`, `disease`, `drug`, `study`, `variant` …
    #[serde(default)]
    pub entity: String,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub highlights: Vec<String>,
    /// Full entity object; schema depends on `entity`.
    #[serde(default)]
    pub object: Option<Value>,
}

/// The `{ total, hits, aggregations }` payload returned by `search`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResults {
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub hits: Vec<SearchResult>,
    #[serde(default)]
    pub aggregations: Option<Value>,
}
