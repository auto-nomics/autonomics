use async_trait::async_trait;

use crate::config::CatalogConfig;
use crate::error::Result;
use crate::hf::MultiRepoHfSource;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest};

const INDEX_NAME: &str = "index.json";

/// Minimal byte-source seam used by the Hugging Face catalog client.
#[async_trait]
pub trait ObjectSource: Send + Sync {
    /// Read the complete object stored at `owner/repo/path`.
    async fn read(&self, key: &str) -> Result<Vec<u8>>;

    /// Read `len` bytes starting at `offset`, enabling bounded-memory
    /// downloads of large payload files.
    async fn read_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>>;
}

/// Read-only accessor for a Hugging Face catalog registry.
///
/// The registry contains repository references. Each referenced package
/// repository owns its local index and immutable payload versions.
pub struct RemoteCatalog {
    registry_repo: String,
    source: Box<dyn ObjectSource>,
}

impl RemoteCatalog {
    /// Read a catalog hosted in a Hugging Face dataset repository.
    pub fn hf(repo_id: &str, revision: Option<String>, token: Option<String>) -> Result<Self> {
        let config = CatalogConfig {
            repository: Some(repo_id.to_string()),
            ..CatalogConfig::default()
        };
        config.validate()?;
        Ok(Self {
            registry_repo: repo_id.to_string(),
            source: Box::new(MultiRepoHfSource::new(repo_id, revision, token)?),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_source(config: CatalogConfig, source: Box<dyn ObjectSource>) -> Self {
        Self {
            registry_repo: config.repository.expect("test catalog has repository"),
            source,
        }
    }

    pub(crate) fn source(&self) -> &dyn ObjectSource {
        self.source.as_ref()
    }

    pub async fn index(&self) -> Result<CatalogIndex> {
        let registry_key = format!("{}/{INDEX_NAME}", self.registry_repo);
        let mut index = self.read_index(&registry_key).await?;

        // A registry contains only repository references. Resolve each package
        // repository's local index and merge its entries for search/select.
        for repository in index.repositories.clone() {
            let package_key = format!("{repository}/{INDEX_NAME}");
            let package_index = self.read_index(&package_key).await?;
            validate_package_index(&package_index, &repository, &package_key)?;
            index.entries.extend(package_index.entries);
        }

        index
            .validate()
            .map_err(|error| format!("invalid resolved catalog registry: {error}"))?;
        Ok(index)
    }

    /// Read and validate the package-local index at `owner/name/index.json`.
    pub async fn package_index(&self, repository: &str) -> Result<CatalogIndex> {
        let key = format!("{repository}/{INDEX_NAME}");
        let index = self.read_index(&key).await?;
        validate_package_index(&index, repository, &key)?;
        Ok(index)
    }

    async fn read_index(&self, key: &str) -> Result<CatalogIndex> {
        let bytes = self.source.read(key).await?;
        let index: CatalogIndex = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse object `{key}`: {error}"))?;
        index
            .validate()
            .map_err(|error| format!("invalid catalog index `{key}`: {error}"))?;
        Ok(index)
    }

    pub async fn manifest(&self, entry: &CatalogEntry) -> Result<DatasetManifest> {
        let key = entry.source_manifest_key();
        let bytes = self.source.read(&key).await?;
        let manifest: DatasetManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse object `{key}`: {error}"))?;
        manifest
            .validate()
            .map_err(|error| format!("invalid manifest `{key}`: {error}"))?;
        validate_entry_manifest(entry, &manifest)?;
        Ok(manifest)
    }
}

fn validate_package_index(index: &CatalogIndex, repository: &str, key: &str) -> Result<()> {
    if !index.repositories.is_empty() {
        return Err(format!("package index `{key}` must not be a registry").into());
    }
    if let Some(entry) = index.entries.iter().find(|entry| entry.repo != repository) {
        return Err(format!(
            "package index `{key}` routes `{}` to `{}`",
            entry.id, entry.repo
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_entry_manifest(
    entry: &CatalogEntry,
    manifest: &DatasetManifest,
) -> Result<()> {
    if manifest.id != entry.id
        || manifest.version != entry.version
        || manifest.kind != entry.kind
        || manifest.digest.as_deref() != Some(entry.digest.as_str())
    {
        return Err(format!("catalog entry `{}` does not match its manifest", entry.id).into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    pub(crate) struct MapSource(pub(crate) BTreeMap<String, Vec<u8>>);

    #[async_trait]
    impl ObjectSource for MapSource {
        async fn read(&self, key: &str) -> Result<Vec<u8>> {
            self.0
                .get(key)
                .cloned()
                .ok_or_else(|| format!("missing object `{key}`").into())
        }

        async fn read_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
            let bytes = self.read(key).await?;
            let start = offset as usize;
            let end = (offset + len) as usize;
            bytes
                .get(start..end)
                .map(|value| value.to_vec())
                .ok_or_else(|| format!("invalid range for object `{key}`").into())
        }
    }

    #[tokio::test]
    async fn registry_resolves_package_local_indexes() {
        let entry = CatalogEntry {
            id: "cache.panel".into(),
            repo: "owner/cache-panel".into(),
            version: "v1".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            current: true,
            created_unix_seconds: 1,
        };
        let registry = CatalogIndex {
            repositories: vec!["owner/cache-panel".into()],
            ..CatalogIndex::default()
        };
        let package_index = CatalogIndex {
            entries: vec![entry.clone()],
            ..CatalogIndex::default()
        };
        let mut objects = MapSource::default();
        objects.0.insert(
            "owner/catalog-index/index.json".into(),
            serde_json::to_vec(&registry).unwrap(),
        );
        objects.0.insert(
            "owner/cache-panel/index.json".into(),
            serde_json::to_vec(&package_index).unwrap(),
        );
        let config = CatalogConfig {
            repository: Some("owner/catalog-index".into()),
            ..CatalogConfig::default()
        };
        let remote = RemoteCatalog::from_source(config, Box::new(objects));

        let resolved = remote.index().await.unwrap();
        assert_eq!(resolved.current_entries().next(), Some(&entry));
    }
}
