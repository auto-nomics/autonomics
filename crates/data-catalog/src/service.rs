//! Searchable runtime view over the immutable object-storage catalog.
//!
//! [`CatalogRuntime`](crate::runtime::CatalogRuntime) intentionally remains a
//! lightweight loader used during
//! process startup. [`CatalogService`] adds the process-level behavior needed
//! by agents: a refreshable current-entry snapshot, manifest-backed summaries,
//! text/tag search, version inspection, and file listings.

use std::collections::BTreeMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use vfs::VfsManifest;

use crate::config::CatalogConfig;
use crate::model::{CatalogEntry, CatalogIndex, DatasetFile, DatasetManifest};
use crate::storage::{operator_for_backend, read_json_object};

#[derive(Debug)]
pub struct CatalogService {
    config: CatalogConfig,
    operator: opendal::Operator,
    snapshot: RwLock<CatalogSnapshot>,
}

#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub index: CatalogIndex,
    pub records: Vec<CatalogRecord>,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogDataset {
    #[serde(flatten)]
    pub record: CatalogRecord,
    pub files: Vec<DatasetFile>,
    pub manifest_path: String,
    pub payload_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogSearchQuery {
    /// Free-text terms matched against id, version, kind, metadata, payload,
    /// description, tags, and digest. All whitespace-separated terms must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Exact kind filter, for example `ldsc_ref_ld_chr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Required tags. A record matches when it contains every requested tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Maximum number of records to return. Defaults to 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Behavior for refreshing a process-level catalog snapshot.
#[async_trait]
pub trait CatalogServiceTrait {
    /// The immutable snapshot produced by a successful refresh.
    type Snapshot;

    /// Reload catalog state from the configured backend.
    async fn refresh(&self) -> Result<Self::Snapshot, String>;
}

#[async_trait]
impl CatalogServiceTrait for CatalogService {
    type Snapshot = CatalogSnapshot;

    async fn refresh(&self) -> Result<CatalogSnapshot, String> {
        CatalogService::refresh(self).await
    }
}

impl CatalogService {
    pub async fn new(manifest: &VfsManifest, config: &CatalogConfig) -> Result<Self, String> {
        config.validate()?;
        let operator = operator_for_backend(manifest, &config.backend)?;
        let snapshot = Self::load_snapshot(&operator, config).await?;
        Ok(Self {
            config: config.clone(),
            operator,
            snapshot: RwLock::new(snapshot),
        })
    }

    /// Reload `index.json` and all current manifests, replacing the cached view.
    pub async fn refresh(&self) -> Result<CatalogSnapshot, String> {
        let snapshot = Self::load_snapshot(&self.operator, &self.config).await?;
        *self.snapshot.write().await = snapshot.clone();
        Ok(snapshot)
    }

    pub async fn snapshot(&self) -> CatalogSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn search(&self, query: CatalogSearchQuery) -> Result<Vec<CatalogRecord>, String> {
        let records = self.snapshot().await.records;
        let limit = query.limit.unwrap_or(50).min(500);
        let terms = query
            .query
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let required_tags = query
            .tags
            .iter()
            .map(|tag| tag.to_ascii_lowercase())
            .collect::<Vec<_>>();

        let mut matched = records
            .into_iter()
            .filter(|record| {
                query.kind.as_deref().is_none_or(|kind| kind == record.kind)
                    && required_tags.iter().all(|required| {
                        record
                            .tags
                            .iter()
                            .any(|tag| tag.eq_ignore_ascii_case(required))
                    })
                    && terms
                        .iter()
                        .all(|term| search_haystack(record).contains(term))
            })
            .take(limit)
            .collect::<Vec<_>>();
        matched.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| right.created_unix_seconds.cmp(&left.created_unix_seconds))
        });
        Ok(matched)
    }

    /// Describe a current or historical entry. Version and digest are both
    /// optional; when both are omitted, the current entry is returned.
    pub async fn describe(
        &self,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<CatalogDataset, String> {
        let snapshot = self.snapshot().await;
        let entry = select_entry(&snapshot.index, id, version, digest)?;
        self.load_dataset(&entry).await
    }

    pub async fn list_files(
        &self,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<Vec<DatasetFile>, String> {
        Ok(self.describe(id, version, digest).await?.files)
    }

    pub async fn list_versions(&self, id: &str) -> Result<Vec<CatalogEntry>, String> {
        let snapshot = self.snapshot().await;
        let mut entries = snapshot
            .index
            .entries
            .iter()
            .filter(|entry| entry.id == id)
            .cloned()
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Err(format!("catalog dataset `{id}` was not found"));
        }
        entries.sort_by(|left, right| {
            right
                .created_unix_seconds
                .cmp(&left.created_unix_seconds)
                .then_with(|| left.version.cmp(&right.version))
        });
        Ok(entries)
    }

    async fn load_snapshot(
        operator: &opendal::Operator,
        config: &CatalogConfig,
    ) -> Result<CatalogSnapshot, String> {
        config.validate()?;
        let index_key = config.object_key(&config.index);
        let index: CatalogIndex = read_json_object(operator, &index_key).await?;
        index
            .validate()
            .map_err(|error| format!("invalid catalog index `{index_key}`: {error}"))?;

        let mut records = Vec::new();
        for entry in index.current_entries() {
            let manifest = read_manifest(operator, config, &entry.manifest).await?;
            validate_entry_manifest(entry, &manifest)?;
            records.push(CatalogRecord::from_parts(entry.clone(), manifest));
        }
        Ok(CatalogSnapshot { index, records })
    }

    async fn load_dataset(&self, entry: &CatalogEntry) -> Result<CatalogDataset, String> {
        let manifest = read_manifest(&self.operator, &self.config, &entry.manifest).await?;
        validate_entry_manifest(entry, &manifest)?;
        let record = CatalogRecord::from_parts(entry.clone(), manifest.clone());
        Ok(CatalogDataset {
            record,
            files: manifest.files,
            manifest_path: entry.manifest.clone(),
            payload_path: entry.files.clone(),
        })
    }
}

impl CatalogRecord {
    fn from_parts(entry: CatalogEntry, manifest: DatasetManifest) -> Self {
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

async fn read_manifest(
    operator: &opendal::Operator,
    config: &CatalogConfig,
    relative_key: &str,
) -> Result<DatasetManifest, String> {
    let key = config.object_key(relative_key);
    let manifest: DatasetManifest = read_json_object(operator, &key).await?;
    manifest
        .validate()
        .map_err(|error| format!("invalid manifest `{key}`: {error}"))?;
    Ok(manifest)
}

fn validate_entry_manifest(entry: &CatalogEntry, manifest: &DatasetManifest) -> Result<(), String> {
    if manifest.id != entry.id
        || manifest.version != entry.version
        || manifest.kind != entry.kind
        || manifest.digest.as_deref() != Some(entry.digest.as_str())
    {
        return Err(format!(
            "catalog entry `{}` does not match its manifest",
            entry.id
        ));
    }
    Ok(())
}

fn select_entry(
    index: &CatalogIndex,
    id: &str,
    version: Option<&str>,
    digest: Option<&str>,
) -> Result<CatalogEntry, String> {
    let mut matched = index
        .entries
        .iter()
        .filter(|entry| entry.id == id)
        .filter(|entry| version.is_none_or(|value| entry.version == value))
        .filter(|entry| digest.is_none_or(|value| entry.digest == value))
        .cloned()
        .collect::<Vec<_>>();
    match matched.len() {
        0 => Err(format!("catalog dataset `{id}` was not found")),
        1 => Ok(matched.remove(0)),
        _ => {
            matched.retain(|entry| entry.current);
            match matched.len() {
                1 => Ok(matched.remove(0)),
                0 => Err(format!(
                    "catalog dataset `{id}` has multiple versions; specify version or digest"
                )),
                _ => Err(format!(
                    "catalog dataset `{id}` has multiple current entries"
                )),
            }
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

fn search_haystack(record: &CatalogRecord) -> String {
    let mut text = format!(
        "{} {} {} {} {} {} ",
        record.id,
        record.version,
        record.kind,
        record.digest,
        record.vfs_alias,
        record.vfs_immutable
    );
    if let Some(description) = &record.description {
        text.push_str(description);
        text.push(' ');
    }
    text.push_str(&record.tags.join(" "));
    text.push(' ');
    for (key, value) in &record.metadata {
        text.push_str(key);
        text.push(' ');
        text.push_str(value);
        text.push(' ');
    }
    if let Ok(payload) = serde_json::to_string(&record.payload) {
        text.push_str(&payload);
    }
    text.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{BuildOptions, build_package};
    use crate::publish::publish_package;
    use crate::storage::operator_for_backend;
    use vfs::{BackendConfig, BackendDefinition, MountDefinition};

    #[tokio::test]
    async fn searches_current_manifest_metadata_and_lists_versions() {
        let workspace = tempfile::tempdir().unwrap();
        let warehouse = tempfile::tempdir().unwrap();
        let input = workspace.path().join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("panel.txt"), b"panel-data").unwrap();
        let package = build_package(
            &input,
            workspace.path().join("package"),
            BuildOptions {
                id: Some("search.panel".into()),
                version: Some("v1".into()),
                kind: Some("panel".into()),
                metadata: [
                    (
                        "description".to_string(),
                        "European reference panel".to_string(),
                    ),
                    ("tags".to_string(), "genomics, eur".to_string()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        )
        .unwrap();
        let manifest = vfs::VfsManifest {
            backend: vec![BackendDefinition {
                id: "warehouse".into(),
                config: BackendConfig::local(warehouse.path().to_string_lossy().into_owned()),
            }],
            mount: vec![MountDefinition {
                path: "/".into(),
                backend: "warehouse".into(),
                source: "/".into(),
                read_only: true,
            }],
        };
        let config = CatalogConfig {
            backend: "warehouse".into(),
            source: "/catalog".into(),
            ..Default::default()
        };
        let operator = operator_for_backend(&manifest, &config.backend).unwrap();
        publish_package(package.path, &config, &operator)
            .await
            .unwrap();

        let service = CatalogService::new(&manifest, &config).await.unwrap();
        let records = service
            .search(CatalogSearchQuery {
                query: Some("european genomics".into()),
                kind: Some("panel".into()),
                tags: vec!["eur".into()],
                limit: Some(10),
            })
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "search.panel");
        assert_eq!(
            records[0].description.as_deref(),
            Some("European reference panel")
        );
        assert_eq!(records[0].file_count, 1);
        assert_eq!(records[0].total_size_bytes, "panel-data".len() as u64);

        let detail = service.describe("search.panel", None, None).await.unwrap();
        assert_eq!(detail.files[0].path, "panel.txt");
        let versions = service.list_versions("search.panel").await.unwrap();
        assert_eq!(versions.len(), 1);
        assert!(versions[0].current);

        let package_v2 = build_package(
            &input,
            workspace.path().join("package-v2"),
            BuildOptions {
                id: Some("search.panel".into()),
                version: Some("v2".into()),
                kind: Some("panel".into()),
                metadata: [("description".to_string(), "Updated panel".to_string())]
                    .into_iter()
                    .collect(),
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
        publish_package(package_v2.path, &config, &operator)
            .await
            .unwrap();
        let refreshed = service.refresh().await.unwrap();
        assert_eq!(refreshed.records.len(), 1);
        assert_eq!(refreshed.records[0].version, "v2");
        let versions = service.list_versions("search.panel").await.unwrap();
        assert_eq!(versions.len(), 2);
        let current = versions
            .iter()
            .find(|entry| entry.current)
            .expect("one current version");
        assert_eq!(current.version, "v2");
    }
}
