use async_trait::async_trait;
use opendal::Operator;
use vfs::VfsManifest;

use crate::config::CatalogConfig;
use crate::error::Result;
use crate::hf::MultiRepoHfSource;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest};
use crate::storage::operator_for_backend;

/// Minimal byte-source seam used by the remote catalog.
///
/// Today the only implementation wraps an opendal [`Operator`] (S3, OSS, or a
/// local test backend). When packages move to Hugging Face, an implementation
/// over the HF Hub repository API can slot in here without changing the index
/// layout, checksum verification, local cache, or runtime integration.
#[async_trait]
pub trait ObjectSource: Send + Sync {
    /// Read the complete object stored at `key`.
    async fn read(&self, key: &str) -> Result<Vec<u8>>;

    /// Read `len` bytes starting at `offset`, enabling bounded-memory
    /// downloads of large payload files.
    async fn read_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>>;
}

/// Object-storage source backed directly by an opendal operator.
pub struct OperatorSource(Operator);

#[async_trait]
impl ObjectSource for OperatorSource {
    async fn read(&self, key: &str) -> Result<Vec<u8>> {
        let buffer = self
            .0
            .read(key)
            .await
            .map_err(|error| format!("read object `{key}`: {error}"))?;
        Ok(buffer.to_vec())
    }

    async fn read_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let reader = self
            .0
            .reader(key)
            .await
            .map_err(|error| format!("open object `{key}`: {error}"))?;
        let buffer = reader
            .read(offset..offset + len)
            .await
            .map_err(|error| format!("read object `{key}` at {offset}: {error}"))?;
        Ok(buffer.to_vec())
    }
}

/// Read-only accessor for a catalog published in object storage.
///
/// The layout is one root `index.json` plus a content-addressed directory per
/// entry. Data is never served to the VFS directly; use
/// [`crate::LocalCatalog`] to install selected packages first.
pub struct RemoteCatalog {
    config: CatalogConfig,
    source: Box<dyn ObjectSource>,
}

impl RemoteCatalog {
    /// Build from a backend definition in the `vfs.toml` backend table.
    ///
    /// The manifest is only consulted to find backend credentials and
    /// endpoints; no VFS mounts are constructed.
    pub fn new(manifest: &VfsManifest, config: &CatalogConfig) -> Result<Self> {
        config.validate()?;
        let backend = config
            .backend
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "catalog backend is required in object-storage mode".to_string())?;
        let source = Box::new(OperatorSource(operator_for_backend(manifest, backend)?));
        Ok(Self {
            config: config.clone(),
            source,
        })
    }

    /// Build over a custom source, for remotes that are not object stores
    /// (for example a Hugging Face Hub repository client).
    pub fn from_source(config: &CatalogConfig, source: Box<dyn ObjectSource>) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config: config.clone(),
            source,
        })
    }

    /// Read a catalog hosted in a Hugging Face dataset repository.
    pub fn hf(repo_id: &str, revision: Option<String>, token: Option<String>) -> Result<Self> {
        let config = CatalogConfig {
            backend: Some("huggingface".into()),
            repository: Some(repo_id.to_string()),
            repository_prefix: None,
            revision: None,
            source: "/".into(),
            index: "index.json".into(),
            enabled: true,
            agent_visible: false,
        };
        Self::from_source(
            &config,
            Box::new(MultiRepoHfSource::new(repo_id, revision, token)?),
        )
    }

    pub fn config(&self) -> &CatalogConfig {
        &self.config
    }

    pub fn source(&self) -> &dyn ObjectSource {
        self.source.as_ref()
    }

    pub async fn index(&self) -> Result<CatalogIndex> {
        let key = if let Some(repository) = &self.config.repository {
            format!("{}/{}", repository, self.config.index)
        } else {
            self.config.object_key(&self.config.index)
        };
        let mut index = self.read_index(&key).await?;

        // A registry contains only repository references. Resolve each package
        // repository's local index and merge its entries for search/select.
        for repository in index.repositories.clone() {
            let package_key = format!("{repository}/{}", self.config.index);
            let package_index = self.read_index(&package_key).await?;
            if !package_index.repositories.is_empty() {
                return Err(
                    format!("package index `{package_key}` must not itself be a registry").into(),
                );
            }
            if let Some(entry) = package_index
                .entries
                .iter()
                .find(|entry| entry.repo != repository)
            {
                return Err(format!(
                    "package index `{package_key}` routes `{}` to `{}`",
                    entry.id, entry.repo
                )
                .into());
            }
            index.entries.extend(package_index.entries);
        }

        index
            .validate()
            .map_err(|error| format!("invalid resolved catalog index `{key}`: {error}"))?;
        Ok(index)
    }

    /// Read and validate the package-local index at `owner/name/index.json`.
    pub async fn package_index(&self, repository: &str) -> Result<CatalogIndex> {
        let key = format!("{repository}/{}", self.config.index);
        let index = self.read_index(&key).await?;
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
        let key = if entry.repo.is_empty() {
            self.config.object_key(&entry.manifest_key())
        } else {
            entry.source_manifest_key()
        };
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
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::BTreeMap;

    struct MapSource(BTreeMap<String, Vec<u8>>);

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
        let mut objects = BTreeMap::new();
        objects.insert("index.json".into(), serde_json::to_vec(&registry).unwrap());
        objects.insert(
            "owner/cache-panel/index.json".into(),
            serde_json::to_vec(&package_index).unwrap(),
        );
        let mut config = CatalogConfig::default();
        config.backend = Some("memory".into());
        let remote = RemoteCatalog::from_source(&config, Box::new(MapSource(objects))).unwrap();

        let resolved = remote.index().await.unwrap();
        assert_eq!(resolved.current_entries().next(), Some(&entry));
    }
}
