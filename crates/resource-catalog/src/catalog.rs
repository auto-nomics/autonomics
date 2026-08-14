//! The [`ResourceCatalog`] facade: a singleton, in-process, single source of
//! truth for all resources.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::entry::ResourceEntry;
use crate::error::{ResourceError, Result};
use crate::patch::ResourcePatch;
use crate::persist::{ManifestStore, TursoManifestStore};
use crate::registry::ResourceRegistry;
use crate::storage::{SharedBackendRegistry, StorageBackend, StorageConfig, StorageRef};
use crate::validate::validate;

/// The process-wide singleton. Set once during bootstrap (see
/// [`ResourceCatalog::set_global`]); non-node SDK crates resolve through it via
/// [`ResourceCatalog::global`].
static GLOBAL: OnceLock<Arc<ResourceCatalog>> = OnceLock::new();

/// The in-process resource catalog.
///
/// Wraps a [`ResourceRegistry`] behind an `RwLock`, an optional persistence
/// backend, a named storage backend registry, and a base directory used to
/// absolutize relative database paths.
#[derive(Clone)]
pub struct ResourceCatalog {
    pub(crate) inner: Arc<RwLock<ResourceRegistry>>,
    pub(crate) backends: SharedBackendRegistry,
    persist: Option<Arc<dyn ManifestStore>>,
    base_dir: PathBuf,
}

impl ResourceCatalog {
    /// A catalog with no persistence backend and no storage backends.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceRegistry::new())),
            backends: Arc::new(RwLock::new(crate::storage::BackendRegistry::new())),
            persist: None,
            base_dir: base_dir.into(),
        }
    }

    /// A catalog with a persistence backend.
    pub fn with_persist(base_dir: impl Into<PathBuf>, persist: Arc<dyn ManifestStore>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceRegistry::new())),
            backends: Arc::new(RwLock::new(crate::storage::BackendRegistry::new())),
            persist: Some(persist),
            base_dir: base_dir.into(),
        }
    }

    /// Open (or create) the catalog, loading the persisted manifest if present.
    ///
    /// Persistence failures are non-fatal: the catalog continues in-memory and
    /// the error is logged.
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
            backends: Arc::new(RwLock::new(crate::storage::BackendRegistry::new())),
            persist: persist.clone(),
            base_dir,
        };

        if let Some(store) = &persist {
            match store.load().await {
                Ok(entries) => {
                    let mut reg = catalog.inner.write().expect("catalog lock");
                    for entry in entries {
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

    /// Like [`load_or_new`](Self::load_or_new) but **returns errors instead
    /// of silently degrading**.
    pub async fn load_or_error(
        base_dir: impl Into<PathBuf>,
        db_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let base_dir = base_dir.into();
        let store = TursoManifestStore::open(&db_path).await?;
        let persist: Arc<dyn ManifestStore> = Arc::new(store);

        let catalog = ResourceCatalog {
            inner: Arc::new(RwLock::new(ResourceRegistry::new())),
            backends: Arc::new(RwLock::new(crate::storage::BackendRegistry::new())),
            persist: Some(persist.clone()),
            base_dir,
        };

        let entries = persist.load().await?;
        {
            let mut reg = catalog.inner.write().expect("catalog lock");
            for entry in entries {
                if let Err(e) = reg.register(entry) {
                    tracing::warn!("resource catalog: loading persisted entry: {e}");
                }
            }
        }

        Ok(catalog)
    }

    /// Set the process-wide global catalog. Only the first call succeeds.
    pub fn set_global(arc: Arc<Self>) -> Result<()> {
        GLOBAL.set(arc).map_err(|_| ResourceError::GlobalAlreadySet)
    }

    /// The process-wide global catalog, if set.
    pub fn global() -> Option<Arc<Self>> {
        GLOBAL.get().cloned()
    }

    // ── Storage backend management ───────────────────────────────────

    /// Register a named storage backend. Builds the operator eagerly so the
    /// first `resolve_storage` call is fast. Re-registering the same name
    /// replaces the backend.
    ///
    /// **Not persisted**: backends are runtime-only (they carry credentials).
    /// The persisted manifest stores only the backend *name* on each entry.
    pub fn register_backend(
        &self,
        name: &str,
        config: StorageConfig,
    ) -> Result<()> {
        let mut backends = self.backends.write().expect("backends lock");
        backends.register(name, config)
    }

    /// Look up a registered backend by name.
    pub fn backend(&self, name: &str) -> Option<StorageBackend> {
        let backends = self.backends.read().expect("backends lock");
        backends.get(name).cloned()
    }

    /// All registered backend names.
    pub fn backend_names(&self) -> Vec<String> {
        let backends = self.backends.read().expect("backends lock");
        backends.names().iter().map(|s| s.to_string()).collect()
    }

    // ── Resource registration ────────────────────────────────────────

    /// Register a single resource.
    pub fn register(&self, entry: ResourceEntry) -> Result<()> {
        self.inner.write().expect("catalog lock").register(entry)
    }

    /// Remove a resource by logical name from the in-memory registry.
    /// Call [`persist`](Self::persist) afterwards to update the manifest.
    pub fn deregister(&self, name: &str) -> Option<ResourceEntry> {
        self.inner.write().expect("catalog lock").remove(name)
    }

    /// Apply a partial patch to an existing resource, in memory only.
    ///
    /// Returns the post-patch entry.
    pub fn patch(&self, name: &str, patch: ResourcePatch) -> Result<ResourceEntry> {
        if patch.is_empty() {
            return Err(ResourceError::Validation(
                "patch is empty: nothing to update (set at least one field, e.g. --description)".into(),
            ));
        }
        let mut reg = self.inner.write().expect("catalog lock");
        let entry = reg
            .get_mut(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        if let Some(d) = patch.description {
            entry.description = d;
        }
        validate(entry)?;
        Ok(entry.clone())
    }

    /// Look up a resource by logical name.
    pub fn get(&self, name: &str) -> Option<ResourceEntry> {
        self.inner.read().expect("catalog lock").get(name).cloned()
    }

    /// All registered resources.
    pub fn list(&self) -> Vec<ResourceEntry> {
        self.inner.read().expect("catalog lock").into_entries()
    }

    /// Persist the registered manifest. Non-fatal on error.
    pub async fn persist(&self) {
        if let Some(store) = &self.persist {
            let entries = self.list();
            if let Err(e) = store.save(&entries).await {
                tracing::warn!("resource catalog: failed to persist manifest: {e}");
            }
        }
    }

    /// Absolutize a path against the catalog's base directory (for Database
    /// paths only — Storage paths are operator-relative).
    pub(crate) fn absolutize(&self, p: &str) -> PathBuf {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            path
        } else {
            self.base_dir.join(path)
        }
    }
}

// ── Storage resolution ─────────────────────────────────────────────────

impl ResourceCatalog {
    /// Resolve a logical name to a [`StorageRef`] — the opendal operator and
    /// path within it.
    ///
    /// The backend named on the resource entry must have been registered via
    /// [`register_backend`](Self::register_backend) beforehand. If it has
    /// not, an [`UnknownBackend`](ResourceError::UnknownBackend) error is
    /// returned.
    pub fn resolve_storage(&self, name: &str) -> Result<StorageRef> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            crate::kind::ResourceAddress::Storage { backend, path, .. } => {
                let op = self.backend_operator(backend)?;
                Ok(StorageRef {
                    operator: op,
                    path: path.clone(),
                })
            }
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "storage",
                found: crate::resolve::kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to a DataFusion listing URL for its storage
    /// resource.
    ///
    /// This is the lightweight, consumer-facing form: callers get a string
    /// (e.g. `s3://bucket/ld_score/1000g_eur/`) and can register a
    /// `ListingTable` without constructing any intermediate ref type.
    pub fn resolve_storage_url(&self, name: &str) -> Result<String> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            crate::kind::ResourceAddress::Storage { backend, path, .. } => {
                let config = self
                    .backend_config(backend)
                    .ok_or_else(|| ResourceError::UnknownBackend(backend.clone()))?;
                Ok(config.listing_url(path))
            }
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "storage",
                found: crate::resolve::kind_str(other),
            }),
        }
    }

    /// Like [`resolve_storage`](Self::resolve_storage) but substitutes
    /// `{key}` placeholders in the stored path with values from `subs`.
    /// Used for per-chromosome path templates (e.g. `{N}` → `1`).
    pub fn resolve_storage_template(
        &self,
        name: &str,
        subs: &std::collections::BTreeMap<String, String>,
    ) -> Result<StorageRef> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            crate::kind::ResourceAddress::Storage { backend, path, .. } => {
                let mut resolved = path.clone();
                for (k, v) in subs {
                    resolved = resolved.replace(&format!("{{{k}}}"), v);
                }
                if resolved.contains('{') {
                    return Err(ResourceError::Validation(format!(
                        "unresolved placeholder in path for resource '{name}': {path}"
                    )));
                }
                let op = self.backend_operator(backend)?;
                Ok(StorageRef {
                    operator: op,
                    path: resolved,
                })
            }
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "storage",
                found: crate::resolve::kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to an [`opendal::Operator`] for its backend.
    /// Returns the same operator every time (cached at registration).
    pub fn backend_operator(&self, backend: &str) -> Result<opendal::Operator> {
        self.backend(backend)
            .map(|b| b.operator)
            .ok_or_else(|| ResourceError::UnknownBackend(backend.to_string()))
    }

    /// Resolve the [`StorageConfig`] for a backend, if registered.
    /// Used by archive logic to derive local filesystem paths.
    pub fn backend_config(&self, backend: &str) -> Option<StorageConfig> {
        self.backend(backend).map(|b| b.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::{DataFormat, ResourceAddress, ResourceKind};

    fn sample() -> ResourceEntry {
        ResourceEntry::new(
            "ldscore.1000g_eur",
            ResourceKind::Storage,
            "1000G EUR LD scores",
            ResourceAddress::storage("default", "ld_score/1000g_eur/"),
        )
    }

    #[test]
    fn global_set_once_and_get() {
        let cat = Arc::new(ResourceCatalog::new("/tmp"));
        ResourceCatalog::set_global(cat.clone()).unwrap();
        assert!(matches!(
            ResourceCatalog::set_global(cat.clone()),
            Err(ResourceError::GlobalAlreadySet)
        ));
        assert!(ResourceCatalog::global().is_some());
    }

    #[tokio::test]
    async fn persist_and_reload_survives_round_trip() {
        let store: Arc<dyn ManifestStore> =
            Arc::new(TursoManifestStore::open_in_memory().await.unwrap());
        let cat = ResourceCatalog::with_persist("/tmp", store.clone());
        cat.register(sample()).unwrap();
        cat.persist().await;

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
        use crate::archive::ArchiveSpec;

        let store: Arc<dyn ManifestStore> =
            Arc::new(TursoManifestStore::open_in_memory().await.unwrap());
        let cat = ResourceCatalog::with_persist("/tmp", store.clone());

        cat.register(
            ResourceEntry::new(
                "test-data.mixer",
                ResourceKind::Storage,
                "MiXiR test fixtures",
                ResourceAddress::storage_with(
                    "default",
                    "reference/mixer_data/",
                    DataFormat::Raw,
                    vec![],
                ),
            )
            .with_archive(ArchiveSpec {
                remote: "aliyun".into(),
                remote_path: "autonomics-data/mixer/test-data/".into(),
                checksum: true,
            }),
        )
        .unwrap();

        cat.persist().await;

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        let entry = &loaded[0];
        assert!(entry.archive_spec.is_some());
    }

    #[test]
    fn patch_updates_description_in_memory() {
        let cat = ResourceCatalog::new("/tmp");
        cat.register(sample()).unwrap();

        let updated = cat
            .patch(
                "ldscore.1000g_eur",
                crate::patch::ResourcePatch::new().description("EUR-only LD scores, v2"),
            )
            .unwrap();
        assert_eq!(updated.description, "EUR-only LD scores, v2");
    }

    #[test]
    fn patch_unknown_resource_returns_error() {
        let cat = ResourceCatalog::new("/tmp");
        assert!(matches!(
            cat.patch(
                "does-not-exist",
                crate::patch::ResourcePatch::new().description("x"),
            ),
            Err(ResourceError::UnknownResource(_))
        ));
    }

    #[test]
    fn patch_empty_patch_returns_error() {
        let cat = ResourceCatalog::new("/tmp");
        cat.register(sample()).unwrap();
        assert!(matches!(
            cat.patch("ldscore.1000g_eur", crate::patch::ResourcePatch::new()),
            Err(ResourceError::Validation(_))
        ));
    }

    #[test]
    fn resolve_storage_requires_registered_backend() {
        let cat = ResourceCatalog::new("/tmp");
        cat.register(sample()).unwrap();
        // No backend registered yet → UnknownBackend
        assert!(matches!(
            cat.resolve_storage("ldscore.1000g_eur"),
            Err(ResourceError::UnknownBackend(_))
        ));

        // Register the backend → resolves successfully
        cat.register_backend("default", StorageConfig::local("/tmp")).unwrap();
        let sref = cat.resolve_storage("ldscore.1000g_eur").unwrap();
        assert_eq!(sref.path, "ld_score/1000g_eur/");
    }
}
