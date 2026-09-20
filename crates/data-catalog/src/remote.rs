use opendal::Operator;
use vfs::VfsManifest;

use crate::config::CatalogConfig;
use crate::error::Result;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest};
use crate::storage::{operator_for_backend, read_json_object};

/// Read-only accessor for a catalog published in object storage.
///
/// The layout is one root `index.json` plus a content-addressed directory per
/// entry. Data is never served to the VFS directly; use
/// [`crate::LocalCatalog`] to install selected packages first.
pub struct RemoteCatalog {
    config: CatalogConfig,
    operator: Operator,
}

impl RemoteCatalog {
    pub fn new(manifest: &VfsManifest, config: &CatalogConfig) -> Result<Self> {
        config.validate()?;
        let operator = operator_for_backend(manifest, &config.backend)?;
        Ok(Self {
            config: config.clone(),
            operator,
        })
    }

    pub fn config(&self) -> &CatalogConfig {
        &self.config
    }

    pub fn operator(&self) -> &Operator {
        &self.operator
    }

    pub async fn index(&self) -> Result<CatalogIndex> {
        let key = self.config.object_key(&self.config.index);
        let index: CatalogIndex = read_json_object(&self.operator, &key).await?;
        index
            .validate()
            .map_err(|error| format!("invalid catalog index `{key}`: {error}"))?;
        Ok(index)
    }

    pub async fn manifest(&self, entry: &CatalogEntry) -> Result<DatasetManifest> {
        let key = self.config.object_key(&entry.manifest_key());
        let manifest: DatasetManifest = read_json_object(&self.operator, &key).await?;
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
