use async_trait::async_trait;
use opendal::Operator;
use vfs::VfsManifest;

use crate::config::CatalogConfig;
use crate::error::Result;
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
        let source = Box::new(OperatorSource(operator_for_backend(
            manifest,
            &config.backend,
        )?));
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

    pub fn config(&self) -> &CatalogConfig {
        &self.config
    }

    pub fn source(&self) -> &dyn ObjectSource {
        self.source.as_ref()
    }

    pub async fn index(&self) -> Result<CatalogIndex> {
        let key = self.config.object_key(&self.config.index);
        let bytes = self.source.read(&key).await?;
        let index: CatalogIndex = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse object `{key}`: {error}"))?;
        index
            .validate()
            .map_err(|error| format!("invalid catalog index `{key}`: {error}"))?;
        Ok(index)
    }

    pub async fn manifest(&self, entry: &CatalogEntry) -> Result<DatasetManifest> {
        let key = self.config.object_key(&entry.manifest_key());
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
