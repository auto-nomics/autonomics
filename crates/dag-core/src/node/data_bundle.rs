use opendal::Operator;
use serde::{Deserialize, Serialize};
use std::collections::HashMap as StdHashMap;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

/// A logical reference to data required by a DAG node.
///
/// `DataBundle` intentionally does not know whether its path is backed by a
/// local filesystem, S3, OSS, or another OpenDAL service. The runtime VFS is
/// responsible for resolving the virtual path to a backend operator and key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataBundle {
    pub ident: String,
    pub desc: String,
    pub vpath: String,
}

/// Runtime mapping from stable bundle identifiers to VFS virtual paths.
///
/// Factories and node specs refer only to `DataBundle::ident`. The physical
/// backend and path are resolved by this catalog plus the mounted VFS.
#[derive(Debug, Default, Clone)]
pub struct DataBundleCatalog {
    bundles: StdHashMap<String, DataBundle>,
}

impl DataBundleCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_bundles(
        bundles: impl IntoIterator<Item = DataBundle>,
    ) -> Result<Self, DataBundleCatalogError> {
        let mut catalog = Self::new();
        for bundle in bundles {
            catalog.register(bundle)?;
        }
        Ok(catalog)
    }

    pub fn register(&mut self, bundle: DataBundle) -> Result<(), DataBundleCatalogError> {
        if bundle.ident.trim().is_empty() {
            return Err(DataBundleCatalogError::InvalidIdentifier(
                bundle.ident.clone(),
            ));
        }
        if !bundle.vpath.starts_with('/') {
            return Err(DataBundleCatalogError::InvalidPath(bundle.vpath));
        }
        if self.bundles.contains_key(&bundle.ident) {
            return Err(DataBundleCatalogError::Duplicate);
        }
        self.bundles.insert(bundle.ident.clone(), bundle);
        Ok(())
    }

    pub fn get(&self, ident: &str) -> Option<&DataBundle> {
        self.bundles.get(ident)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &DataBundle)> {
        self.bundles
            .iter()
            .map(|(ident, bundle)| (ident.as_str(), bundle))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DataBundleCatalogError {
    #[error("data bundle identifier cannot be empty")]
    InvalidIdentifier(String),
    #[error("data bundle identifier is registered more than once")]
    Duplicate,

    #[error("data bundle virtual path must be absolute: {0}")]
    InvalidPath(String),
}

/// A factory-declared dependency on a named runtime bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataBundleBinding {
    /// Local slot used by the node implementation, such as `reference_panel`.
    pub binding: String,
    /// Stable identifier in the runtime [`DataBundleCatalog`].
    pub bundle_id: String,
}

impl DataBundleBinding {
    pub fn new(binding: impl Into<String>, bundle_id: impl Into<String>) -> Self {
        Self {
            binding: binding.into(),
            bundle_id: bundle_id.into(),
        }
    }
}

/// The backend-specific form of a [`DataBundle`] after VFS resolution.
#[derive(Debug, Clone)]
pub struct ResolvedDataBundle {
    pub ident: String,
    pub operator: Operator,
    pub key: String,
}

impl DataBundle {
    pub fn new(
        ident: impl Into<String>,
        desc: impl Into<String>,
        vpath: impl Into<String>,
    ) -> Self {
        Self {
            ident: ident.into(),
            desc: desc.into(),
            vpath: vpath.into(),
        }
    }

    /// Resolve the bundle through the runtime VFS.
    ///
    /// The returned operator is cheap to clone and can be used independently
    /// of `OpendalFileStorage`.
    pub fn resolve(&self, vfs: &OpendalFileStorage) -> ResolvedDataBundle {
        ResolvedDataBundle {
            ident: self.ident.clone(),
            operator: vfs.resolve(&self.vpath),
            key: vfs.resolve_path(&self.vpath),
        }
    }

    pub async fn read_file(&self, vfs: &OpendalFileStorage) -> Result<Vec<u8>, opendal::Error> {
        let resolved = self.resolve(vfs);
        let bytes = resolved.operator.read(&resolved.key).await?;

        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::test_support::mounted_vfs;

    #[test]
    fn catalog_registers_and_reports_duplicate_bundles() {
        let panel = DataBundle::new("panel", "Reference panel", "/bundles/panel.txt");
        let mut catalog = DataBundleCatalog::new();

        catalog.register(panel.clone()).unwrap();

        assert_eq!(catalog.get("panel"), Some(&panel));
        assert!(matches!(
            catalog.register(panel),
            Err(DataBundleCatalogError::Duplicate)
        ));
    }

    #[test]
    fn catalog_rejects_relative_virtual_paths() {
        let bundle = DataBundle::new("panel", "Reference panel", "relative/panel.txt");

        assert!(matches!(
            DataBundleCatalog::from_bundles([bundle]),
            Err(DataBundleCatalogError::InvalidPath(_))
        ));
    }

    #[tokio::test]
    async fn read_file_resolves_through_a_vfs_mount() {
        let harness = mounted_vfs(&[("panel.txt", b"panel-data")]);
        let vfs = harness.storage.as_ref();
        let bundle = DataBundle::new(
            "panels",
            "Reference panel bundle",
            "/bundles/panels/panel.txt",
        );

        let bytes = bundle.read_file(vfs).await.unwrap();

        assert_eq!(bytes, b"panel-data");
        assert!(!harness.data_dir.path().join("panel.txt").exists());
    }
}
