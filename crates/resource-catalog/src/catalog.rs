//! The [`ResourceCatalog`] facade: a singleton, in-process, single source of
//! truth for all resources.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::entry::ResourceEntry;
use crate::error::{ResourceError, Result};
use crate::persist::{ManifestStore, TursoManifestStore};
use crate::registry::ResourceRegistry;

/// The process-wide singleton. Set once during bootstrap (see
/// [`ResourceCatalog::set_global`]); non-node SDK crates resolve through it via
/// [`ResourceCatalog::global`].
static GLOBAL: OnceLock<Arc<ResourceCatalog>> = OnceLock::new();

/// The in-process resource catalog.
///
/// Wraps a [`ResourceRegistry`] behind an `RwLock`, an optional persistence
/// backend, and a base directory used to absolutize relative paths.
#[derive(Clone)]
pub struct ResourceCatalog {
    pub(crate) inner: Arc<RwLock<ResourceRegistry>>,
    persist: Option<Arc<dyn ManifestStore>>,
    base_dir: PathBuf,
}

impl ResourceCatalog {
    /// A catalog with no persistence backend.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceRegistry::new())),
            persist: None,
            base_dir: base_dir.into(),
        }
    }

    /// A catalog with a persistence backend.
    pub fn with_persist(base_dir: impl Into<PathBuf>, persist: Arc<dyn ManifestStore>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceRegistry::new())),
            persist: Some(persist),
            base_dir: base_dir.into(),
        }
    }

    /// Open (or create) the catalog, loading the persisted manifest if present.
    ///
    /// Persistence failures are non-fatal: the catalog continues in-memory and
    /// the error is logged, mirroring the degraded `register_iceberg` path.
    pub async fn load_or_new(base_dir: impl Into<PathBuf>, db_path: impl AsRef<Path>) -> Self {
        let base_dir = base_dir.into();
        let persist: Option<Arc<dyn ManifestStore>> = match TursoManifestStore::open(&db_path).await
        {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                tracing::warn!(
                    "resource catalog: failed to open persistence at {}: {e}; continuing in-memory",
                    db_path.as_ref().display()
                );
                None
            }
        };

        let catalog = ResourceCatalog {
            inner: Arc::new(RwLock::new(ResourceRegistry::new())),
            persist: persist.clone(),
            base_dir,
        };

        if let Some(store) = &persist {
            match store.load().await {
                Ok(entries) => {
                    let mut reg = catalog.inner.write().expect("catalog lock");
                    for entry in entries {
                        // Idempotent load: same-name/same-address re-registration
                        // is a no-op; a conflicting address is skipped with a log.
                        if let Err(e) = reg.register(entry) {
                            tracing::warn!("resource catalog: loading persisted entry: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("resource catalog: failed to load persisted manifest: {e}");
                }
            }
        }

        catalog
    }

    /// Set the process-wide global catalog. Only the first call succeeds.
    pub fn set_global(arc: Arc<Self>) -> Result<()> {
        GLOBAL.set(arc).map_err(|_| ResourceError::GlobalAlreadySet)
    }

    /// The process-wide global catalog, if set.
    pub fn global() -> Option<Arc<Self>> {
        GLOBAL.get().cloned()
    }

    /// Register a single resource.
    pub fn register(&self, entry: ResourceEntry) -> Result<()> {
        self.inner.write().expect("catalog lock").register(entry)
    }

    /// Register a provider's resources, surfacing validation/duplicate errors.
    pub fn register_provider(&self, provider: &dyn crate::provider::ResourceProvider) -> Result<()> {
        for entry in provider.resources() {
            self.register(entry)?;
        }
        Ok(())
    }

    /// Look up a resource by logical name.
    pub fn get(&self, name: &str) -> Option<ResourceEntry> {
        self.inner.read().expect("catalog lock").get(name).cloned()
    }

    /// All registered resources.
    pub fn list(&self) -> Vec<ResourceEntry> {
        self.inner.read().expect("catalog lock").into_entries()
    }

    /// Persist the registered manifest (not a live scan). Non-fatal on error.
    pub async fn persist(&self) {
        if let Some(store) = &self.persist {
            let entries = self.list();
            if let Err(e) = store.save(&entries).await {
                tracing::warn!("resource catalog: failed to persist manifest: {e}");
            }
        }
    }

    /// Absolutize a path against the catalog's base directory.
    pub(crate) fn absolutize(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base_dir.join(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::ResourceAddress;

    fn sample() -> ResourceEntry {
        ResourceEntry::new(
            "ldscore.1000g_eur",
            crate::kind::ResourceKind::IcebergTable,
            "1000G EUR LD scores",
            ResourceAddress::iceberg("ld_score", "1000g_eur"),
        )
    }

    #[test]
    fn global_set_once_and_get() {
        let cat = Arc::new(ResourceCatalog::new("/tmp"));
        ResourceCatalog::set_global(cat.clone()).unwrap();
        // second set must fail
        assert!(matches!(
            ResourceCatalog::set_global(cat.clone()),
            Err(ResourceError::GlobalAlreadySet)
        ));
        assert!(ResourceCatalog::global().is_some());
    }

    #[tokio::test]
    async fn persist_and_reload_survives_round_trip() {
        let store: Arc<dyn ManifestStore> = Arc::new(TursoManifestStore::open_in_memory().await.unwrap());
        let cat = ResourceCatalog::with_persist("/tmp", store.clone());
        cat.register(sample()).unwrap();
        cat.persist().await;

        // A second catalog wrapping the SAME store loads the persisted entry.
        let reloaded = ResourceCatalog::with_persist("/tmp", store.clone());
        for entry in store.load().await.unwrap() {
            reloaded.register(entry).unwrap();
        }
        let entries = reloaded.list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "ldscore.1000g_eur");
    }

    #[tokio::test]
    async fn archive_fields_survive_persist_round_trip() {
        use crate::archive::{ArchiveSpec, ArchiveStatus};

        let store: Arc<dyn ManifestStore> = Arc::new(TursoManifestStore::open_in_memory().await.unwrap());
        let cat = ResourceCatalog::with_persist("/tmp", store.clone());

        // Register a resource WITH archive spec + status.
        cat.register(
            ResourceEntry::new(
                "test-data.mixer",
                crate::kind::ResourceKind::FilePath,
                "MiXiR test fixtures",
                ResourceAddress::path("reference/mixer_data/"),
            )
            .with_archive(ArchiveSpec {
                remote: "aliyun".into(),
                remote_path: "autonomics-data/mixer/test-data/".into(),
                checksum: true,
            }),
        )
        .unwrap();

        // Simulate an archive_status update (as if archive() was called).
        cat.update_archive_status_for_test(
            "test-data.mixer",
            ArchiveStatus {
                archived_at: Some("2026-08-10T12:00:00Z".into()),
                restored_at: None,
                file_count: Some(42),
                size_bytes: Some(1073741824),
                verified: Some(true),
            },
        )
        .unwrap();

        cat.persist().await;

        // Reload from the same store.
        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        let entry = &loaded[0];
        assert_eq!(entry.name, "test-data.mixer");

        // Archive spec survived.
        let spec = entry.archive_spec.as_ref().expect("archive_spec should persist");
        assert_eq!(spec.remote, "aliyun");
        assert_eq!(spec.remote_path, "autonomics-data/mixer/test-data/");
        assert!(spec.checksum);

        // Archive status survived.
        let status = entry.archive_status.as_ref().expect("archive_status should persist");
        assert_eq!(status.archived_at.as_deref(), Some("2026-08-10T12:00:00Z"));
        assert_eq!(status.file_count, Some(42));
        assert_eq!(status.size_bytes, Some(1073741824));
        assert_eq!(status.verified, Some(true));
    }
}

impl ResourceCatalog {
    /// Test-only helper to set archive_status directly.
    #[cfg(test)]
    fn update_archive_status_for_test(&self, name: &str, status: crate::archive::ArchiveStatus) -> crate::error::Result<()> {
        let mut reg = self.inner.write().expect("catalog lock");
        let entry = reg
            .get_mut(name)
            .ok_or_else(|| crate::error::ResourceError::UnknownResource(name.to_string()))?;
        entry.archive_status = Some(status);
        Ok(())
    }
}
