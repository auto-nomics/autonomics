use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{CatalogEntry, CatalogIndex, DatasetFile, DatasetManifest};

/// A cached view of the current catalog index and its materialized records.
#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub index: CatalogIndex,
    pub records: Vec<CatalogRecord>,
}

/// A searchable summary of one current catalog dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogRecord {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub digest: String,
    pub current: bool,
    pub created_unix_seconds: i64,
    pub vfs_alias: String,
    pub vfs_immutable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub payload: serde_json::Map<String, serde_json::Value>,
    pub file_count: usize,
    pub total_size_bytes: u64,
}

/// A dataset summary with its validated manifest details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogDataset {
    #[serde(flatten)]
    pub record: CatalogRecord,
    pub files: Vec<DatasetFile>,
    pub manifest_path: String,
    pub payload_path: String,
}

/// Filters accepted by catalog search implementations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogSearchQuery {
    /// Free-text terms matched against id, version, kind, metadata, payload,
    /// tags, description, and digest. All whitespace-separated terms must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Exact dataset kind, for example ldsc_ref_ld_chr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Required tags. A result must contain every requested tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Maximum number of records to return. Defaults to 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl CatalogRecord {
    pub(crate) fn from_parts(entry: CatalogEntry, manifest: DatasetManifest) -> Self {
        let description = manifest.metadata.get("description").cloned().or_else(|| {
            manifest
                .payload
                .get("description")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
        let tags = merge_tags(
            manifest.metadata.get("tags").map(String::as_str),
            manifest.payload.get("tags"),
        );
        let file_count = manifest.files.len();
        let total_size_bytes = manifest.files.iter().map(|file| file.size).sum();
        Self {
            id: entry.id,
            version: entry.version,
            kind: entry.kind,
            digest: entry.digest,
            current: entry.current,
            created_unix_seconds: entry.created_unix_seconds,
            vfs_alias: entry.vfs_alias,
            vfs_immutable: entry.vfs_immutable,
            description,
            tags,
            metadata: manifest.metadata,
            payload: manifest.payload,
            file_count,
            total_size_bytes,
        }
    }
}

fn merge_tags(metadata: Option<&str>, payload: Option<&serde_json::Value>) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(value) = metadata {
        tags.extend(
            value
                .split([',', ';', ' '])
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string),
        );
    }
    match payload {
        Some(serde_json::Value::Array(values)) => tags.extend(
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::to_string),
        ),
        Some(serde_json::Value::String(value)) => tags.extend(
            value
                .split([',', ';', ' '])
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string),
        ),
        _ => {}
    }
    tags.sort();
    tags.dedup();
    tags
}
