use async_trait::async_trait;
use tokio::sync::RwLock;
use vfs::VfsManifest;

use crate::config::CatalogConfig;
use crate::model::{CatalogEntry, CatalogIndex, DatasetFile};
use crate::storage::{operator_for_backend, read_json_object};

use super::common::{read_manifest, search_haystack, validate_entry_manifest};
use super::model::{CatalogDataset, CatalogRecord, CatalogSearchQuery, CatalogSnapshot};
use super::provider::CatalogServiceTrait;

/// An object-storage catalog view used by the process runtime.
///
/// The service validates the root index and current manifests when loading a
/// snapshot, then keeps that immutable view in memory until it is refreshed.
#[derive(Debug)]
pub struct S3CatalogService {
    config: CatalogConfig,
    operator: opendal::Operator,
    snapshot: RwLock<CatalogSnapshot>,
}

impl S3CatalogService {
    /// Open the configured catalog backend and load the initial snapshot.
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

#[async_trait]
impl CatalogServiceTrait for S3CatalogService {
    async fn refresh(&self) -> Result<CatalogSnapshot, String> {
        let snapshot = Self::load_snapshot(&self.operator, &self.config).await?;
        *self.snapshot.write().await = snapshot.clone();
        Ok(snapshot)
    }

    async fn snapshot(&self) -> CatalogSnapshot {
        self.snapshot.read().await.clone()
    }

    async fn search(&self, query: CatalogSearchQuery) -> Result<Vec<CatalogRecord>, String> {
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

    async fn describe(
        &self,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<CatalogDataset, String> {
        let snapshot = self.snapshot().await;
        let entry = select_entry(&snapshot.index, id, version, digest)?;
        self.load_dataset(&entry).await
    }

    async fn list_files(
        &self,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<Vec<DatasetFile>, String> {
        Ok(self.describe(id, version, digest).await?.files)
    }

    async fn list_versions(&self, id: &str) -> Result<Vec<CatalogEntry>, String> {
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

        let service = S3CatalogService::new(&manifest, &config).await.unwrap();
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
