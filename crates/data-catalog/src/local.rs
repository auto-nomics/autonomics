use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use vfs::MountDefinition;

use crate::error::Result;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest, hex};
use crate::remote::{RemoteCatalog, validate_entry_manifest};

const DOWNLOAD_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// Local cache of selected catalog packages.
///
/// Layout:
/// - `<root>/index.json`: [`CatalogIndex`] of installed versions
/// - `<root>/<entry root>/manifest.json` plus payload files
///
/// Installing a package verifies every file checksum before the entry becomes
/// visible in the local index. Runtime VFS mounts are generated from this
/// cache; the remote catalog is never mounted directly.
pub struct LocalCatalog {
    root: PathBuf,
}

impl LocalCatalog {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("entries"))
            .map_err(|error| format!("create catalog cache {}: {error}", root.display()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn index(&self) -> Result<CatalogIndex> {
        let path = self.root.join("index.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let index: CatalogIndex = serde_json::from_slice(&bytes).map_err(|error| {
                    format!("parse catalog cache index `{}`: {error}", path.display())
                })?;
                index.validate().map_err(|error| {
                    format!("invalid catalog cache index `{}`: {error}", path.display())
                })?;
                Ok(index)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(CatalogIndex::default())
            }
            Err(error) => {
                Err(format!("read catalog cache index `{}`: {error}", path.display()).into())
            }
        }
    }

    fn write_index(&self, index: &CatalogIndex) -> Result<()> {
        let path = self.root.join("index.json");
        let pending = self
            .root
            .join(format!("index.json.pending-{}", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(index).map_err(|error| error.to_string())?;
        std::fs::write(&pending, bytes).map_err(|error| {
            format!("write catalog cache index `{}`: {error}", pending.display())
        })?;
        std::fs::rename(&pending, &path).map_err(|error| {
            format!("publish catalog cache index `{}`: {error}", path.display())
        })?;
        Ok(())
    }

    fn entry_dir(&self, entry: &CatalogEntry) -> PathBuf {
        self.root.join(entry.entry_root())
    }

    /// Read and validate the cached manifest for an installed entry.
    pub fn manifest(&self, entry: &CatalogEntry) -> Result<DatasetManifest> {
        let path = self.entry_dir(entry).join("manifest.json");
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("read local manifest `{}`: {error}", path.display()))?;
        let manifest: DatasetManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse local manifest `{}`: {error}", path.display()))?;
        manifest
            .validate()
            .map_err(|error| format!("invalid local manifest `{}`: {error}", path.display()))?;
        validate_entry_manifest(entry, &manifest)?;
        Ok(manifest)
    }

    /// Resolve and install one package from the remote catalog.
    pub async fn install(
        &self,
        remote: &RemoteCatalog,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<CatalogEntry> {
        let index = remote.index().await?;
        let entry = index.select(id, version, digest)?;
        self.install_entry(remote, &entry).await
    }

    /// Download, verify, and install one already-resolved remote entry.
    ///
    /// Installing an entry whose id and digest are already indexed is
    /// idempotent: the existing payload directory is kept and only the
    /// current-version pointer may be refreshed.
    pub async fn install_entry(
        &self,
        remote: &RemoteCatalog,
        entry: &CatalogEntry,
    ) -> Result<CatalogEntry> {
        let mut index = self.index()?;
        let known = index
            .entries
            .iter()
            .any(|existing| existing.id == entry.id && existing.digest == entry.digest);
        if known && self.entry_dir(entry).join("manifest.json").is_file() {
            let current_is_entry = index.entries.iter().any(|existing| {
                existing.id == entry.id && entry.current && existing.digest == entry.digest
            });
            if !current_is_entry {
                index.upsert_current(entry.clone());
                self.write_index(&index)?;
            }
            return Ok(entry.clone());
        }

        let manifest = remote.manifest(entry).await?;
        let staged = self.stage_entry(remote, entry, &manifest).await?;
        let final_dir = self.entry_dir(entry);
        if let Some(parent) = final_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("create entry directory `{}`: {error}", parent.display())
            })?;
        }
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir).map_err(|error| {
                format!("replace cached entry `{}`: {error}", final_dir.display())
            })?;
        }
        std::fs::rename(&staged, &final_dir)
            .map_err(|error| format!("publish cached entry `{}`: {error}", final_dir.display()))?;

        index.upsert_current(entry.clone());
        self.write_index(&index)?;
        Ok(entry.clone())
    }

    /// Install remote current versions that are missing from this cache.
    ///
    /// When `id` is `None`, every current remote entry is considered.
    pub async fn update(
        &self,
        remote: &RemoteCatalog,
        id: Option<&str>,
    ) -> Result<Vec<CatalogEntry>> {
        let remote_index = remote.index().await?;
        let local_index = self.index()?;
        let mut updated = Vec::new();
        for entry in remote_index.current_entries() {
            if let Some(id) = id {
                if entry.id != id {
                    continue;
                }
            }
            let installed = local_index
                .entries
                .iter()
                .any(|existing| existing.id == entry.id && existing.digest == entry.digest);
            if !installed {
                updated.push(self.install_entry(remote, entry).await?);
            }
        }
        Ok(updated)
    }

    async fn stage_entry(
        &self,
        remote: &RemoteCatalog,
        entry: &CatalogEntry,
        manifest: &DatasetManifest,
    ) -> Result<PathBuf> {
        let staged = self.root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        for file in &manifest.files {
            let key =
                remote
                    .config()
                    .object_key(&format!("{}/{}", entry.payload_prefix(), file.path));
            let target = staged.join(&file.path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    format!("create staging directory `{}`: {error}", parent.display())
                })?;
            }
            download_and_verify(remote.operator(), &key, &target, file.size, &file.sha256).await?;
        }
        let manifest_bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| format!("encode local manifest: {error}"))?;
        tokio::fs::write(staged.join("manifest.json"), manifest_bytes)
            .await
            .map_err(|error| format!("write staged manifest `{}`: {error}", staged.display()))?;
        Ok(staged)
    }

    /// VFS mounts backed by this cache. `backend_id` must reference a local
    /// backend rooted at [`LocalCatalog::root`].
    pub fn mount_definitions(
        &self,
        backend_id: &str,
        agent_visible: bool,
    ) -> Result<Vec<MountDefinition>> {
        let index = self.index()?;
        let mut mounts = Vec::new();
        if agent_visible {
            mounts.push(MountDefinition {
                path: "/catalog".into(),
                backend: backend_id.into(),
                source: "/".into(),
                read_only: true,
            });
        }
        for entry in index.current_entries() {
            let source = entry.entry_root();
            mounts.push(MountDefinition {
                path: entry.vfs_immutable(),
                backend: backend_id.into(),
                source: source.clone(),
                read_only: true,
            });
            mounts.push(MountDefinition {
                path: entry.vfs_alias(),
                backend: backend_id.into(),
                source,
                read_only: true,
            });
        }
        Ok(mounts)
    }

    /// Build the DAG bundle registry from installed current entries.
    pub fn bundle_registry(&self) -> Result<dag_core::BundleRegistry> {
        let index = self.index()?;
        let mut bundles = Vec::new();
        for entry in index.current_entries() {
            let mut bundle = dag_core::DataBundle::new(
                entry.id.clone(),
                format!("{} {} catalog dataset", entry.id, entry.version),
                entry.vfs_alias(),
            );
            bundle.source = Some(format!("/catalog/{}", entry.entry_root()));
            bundle.digest = Some(entry.digest.clone());
            bundles.push(bundle);
        }
        Ok(dag_core::BundleRegistry::from_bundles(bundles)
            .expect("validated catalog entries have unique current ids"))
    }
}

async fn download_and_verify(
    operator: &opendal::Operator,
    key: &str,
    target: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(target)
        .await
        .map_err(|error| format!("create cached file `{}`: {error}", target.display()))?;
    let reader = operator
        .reader(key)
        .await
        .map_err(|error| format!("open object `{key}`: {error}"))?;
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    while offset < expected_size {
        let end = (offset + DOWNLOAD_CHUNK_BYTES).min(expected_size);
        let chunk = reader
            .read(offset..end)
            .await
            .map_err(|error| format!("read object `{key}` at {offset}: {error}"))?;
        let chunk = chunk.to_vec();
        let chunk_len = chunk.len() as u64;
        if chunk_len == 0 {
            return Err(format!(
                "object `{key}` ended at {offset} bytes, expected {expected_size}"
            )
            .into());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("write cached file `{}`: {error}", target.display()))?;
        offset += chunk_len;
    }
    file.flush()
        .await
        .map_err(|error| format!("flush cached file `{}`: {error}", target.display()))?;
    let actual = format!("sha256:{}", hex(&hasher.finalize()));
    if actual != expected_sha256 {
        return Err(format!(
            "object `{key}` checksum mismatch: expected {expected_sha256}, got {actual}"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CatalogConfig;
    use crate::package::{BuildOptions, build_package};
    use crate::publish::publish_package;
    use crate::storage::operator_for_backend;
    use vfs::{
        BackendConfig, BackendDefinition, MountedObjectStore, OpendalFileStorage, VfsManifest,
    };

    async fn published_fixture(id: &str) -> (RemoteCatalog, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let warehouse = workspace.path().join("warehouse");
        std::fs::create_dir_all(&warehouse).unwrap();
        let input = workspace.path().join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("data.txt"), b"local-cache-data").unwrap();
        let package = build_package(
            &input,
            workspace.path().join("package"),
            BuildOptions {
                id: Some(id.into()),
                version: Some("v1".into()),
                kind: Some("table".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "warehouse".into(),
                config: BackendConfig::local(warehouse.to_string_lossy().into_owned()),
            }],
            mount: Vec::new(),
        };
        let config = CatalogConfig {
            backend: "warehouse".into(),
            ..Default::default()
        };
        let operator = operator_for_backend(&manifest, &config.backend).unwrap();
        publish_package(package.path, &config, &operator)
            .await
            .unwrap();
        let remote = RemoteCatalog::new(&manifest, &config).unwrap();
        (remote, workspace)
    }

    #[tokio::test]
    async fn installs_verifies_and_mounts_a_package() {
        let (remote, _warehouse) = published_fixture("cache.panel").await;
        let cache_root = tempfile::tempdir().unwrap();
        let catalog = LocalCatalog::open(cache_root.path()).unwrap();

        let entry = catalog
            .install(&remote, "cache.panel", None, None)
            .await
            .unwrap();
        assert_eq!(entry.version, "v1");
        assert!(entry.current);

        let index = catalog.index().unwrap();
        assert_eq!(index.current_entries().count(), 1);
        let manifest = catalog.manifest(&entry).unwrap();
        assert_eq!(manifest.files[0].path, "data.txt");

        let payload = catalog.root().join(entry.entry_root()).join("data.txt");
        assert_eq!(std::fs::read(payload).unwrap(), b"local-cache-data");

        // Idempotent reinstall keeps the payload and current pointer stable.
        catalog
            .install(&remote, "cache.panel", None, None)
            .await
            .unwrap();
        assert_eq!(catalog.index().unwrap().entries.len(), 1);

        let mounts = catalog.mount_definitions("catalog-cache", true).unwrap();
        let mount_manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "catalog-cache".into(),
                config: BackendConfig::local(cache_root.path().to_string_lossy().into_owned()),
            }],
            mount: mounts,
        };
        let store = MountedObjectStore::from_manifest(&mount_manifest).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::with_mounts(scratch.path(), std::sync::Arc::new(store));
        let path = storage.resolve_path("/bundles/cache.panel/data.txt");
        let bytes = storage
            .resolve("/bundles/cache.panel/data.txt")
            .read(&path)
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), b"local-cache-data");

        let registry = catalog.bundle_registry().unwrap();
        let bundle = registry.get("cache.panel").unwrap();
        assert_eq!(bundle.vpath.as_str(), "/bundles/cache.panel");
        assert_eq!(bundle.digest.as_deref(), Some(entry.digest.as_str()));
    }
}
