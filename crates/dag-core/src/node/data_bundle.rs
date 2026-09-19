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
    /// Catalog object prefix backing this bundle, when known.
    ///
    /// Legacy and built-in bundles leave this unset. Catalog-backed bundles can
    /// use it to resolve immutable panel manifests without changing legacy node APIs.
    #[serde(default)]
    pub source: Option<String>,
    /// Immutable catalog content digest, when known.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Runtime registry mapping stable bundle identifiers to [`DataBundle`] entries.
///
/// This registry is deliberately independent of the object-storage data
/// catalog. It is a runtime projection that can contain catalog-backed
/// datasets, built-in mappings, local overrides, and test fixtures. Factories
/// and node specs refer only to `DataBundle::ident`; the mounted VFS resolves
/// that logical bundle to a physical backend and object key.
#[derive(Debug, Default, Clone)]
pub struct BundleRegistry {
    bundles: StdHashMap<String, DataBundle>,
}

impl BundleRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from bundles, rejecting duplicate identifiers.
    pub fn from_bundles(
        bundles: impl IntoIterator<Item = DataBundle>,
    ) -> Result<Self, BundleRegistryError> {
        let mut catalog = Self::new();
        for bundle in bundles {
            catalog.register(bundle)?;
        }
        Ok(catalog)
    }

    /// Overlay entries on top of this registry.
    ///
    /// Later entries override earlier ones. Runtime-provided bundles use this
    /// path to replace engine defaults without editing factory declarations.
    pub fn with_overriding_bundles(
        mut self,
        bundles: impl IntoIterator<Item = DataBundle>,
    ) -> Result<Self, BundleRegistryError> {
        for bundle in bundles {
            if bundle.ident.trim().is_empty() {
                return Err(BundleRegistryError::InvalidIdentifier(bundle.ident.clone()));
            }
            if !bundle.vpath.starts_with('/') {
                return Err(BundleRegistryError::InvalidPath(bundle.vpath));
            }
            self.bundles.insert(bundle.ident.clone(), bundle);
        }
        Ok(self)
    }

    /// Register one bundle, rejecting an empty identifier, a relative virtual
    /// path, or a duplicate identifier.
    pub fn register(&mut self, bundle: DataBundle) -> Result<(), BundleRegistryError> {
        if bundle.ident.trim().is_empty() {
            return Err(BundleRegistryError::InvalidIdentifier(bundle.ident.clone()));
        }
        if !bundle.vpath.starts_with('/') {
            return Err(BundleRegistryError::InvalidPath(bundle.vpath));
        }
        if self.bundles.contains_key(&bundle.ident) {
            return Err(BundleRegistryError::Duplicate);
        }
        self.bundles.insert(bundle.ident.clone(), bundle);
        Ok(())
    }

    /// Look up a bundle by its stable identifier.
    pub fn get(&self, ident: &str) -> Option<&DataBundle> {
        self.bundles.get(ident)
    }

    /// Iterate all registered bundles in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &DataBundle)> {
        self.bundles
            .iter()
            .map(|(ident, bundle)| (ident.as_str(), bundle))
    }
}

/// Errors produced while constructing or modifying a [`BundleRegistry`].
#[derive(Debug, thiserror::Error)]
pub enum BundleRegistryError {
    #[error("data bundle identifier cannot be empty")]
    InvalidIdentifier(String),
    #[error("data bundle identifier is registered more than once")]
    Duplicate,

    #[error("data bundle virtual path must be absolute: {0}")]
    InvalidPath(String),
}

/// A dependency declaration attached by a node factory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataBundleBinding {
    /// Local slot used by the node implementation, such as `reference_panel`.
    pub binding: String,
    /// Stable identifier in the runtime [`BundleRegistry`].
    pub bundle_id: String,
}

impl DataBundleBinding {
    /// Declare that `binding` should receive the bundle registered as `bundle_id`.
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
            source: None,
            digest: None,
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
    fn registry_registers_and_reports_duplicate_bundles() {
        let panel = DataBundle::new("panel", "Reference panel", "/bundles/panel.txt");
        let mut registry = BundleRegistry::new();

        registry.register(panel.clone()).unwrap();

        assert_eq!(registry.get("panel"), Some(&panel));
        assert!(matches!(
            registry.register(panel),
            Err(BundleRegistryError::Duplicate)
        ));
    }

    #[test]
    fn registry_rejects_relative_virtual_paths() {
        let bundle = DataBundle::new("panel", "Reference panel", "relative/panel.txt");

        assert!(matches!(
            BundleRegistry::from_bundles([bundle]),
            Err(BundleRegistryError::InvalidPath(_))
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
