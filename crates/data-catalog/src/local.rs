use std::path::{Path, PathBuf};

use futures::StreamExt as _;
use futures::TryStreamExt as _;
use sha2::{Digest, Sha256};
use vfs::MountDefinition;

use crate::error::Result;
use crate::migrate::RawCatalogIndex;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest, hex};
use crate::remote::{ObjectSource, RemoteCatalog, validate_entry_manifest};

const DOWNLOAD_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
/// Matches container-runtime's panel cache entry convention.
pub const PANEL_CACHE_COMPLETE_MARKER: &str = ".autonomics-panel-complete";
/// Environment override for the shared panel/catalog cache root.
pub const PANEL_CACHE_ROOT_ENV: &str = "AUTONOMICS_PANEL_CACHE_ROOT";

/// Resolve the shared panel/catalog cache root.
///
/// `AUTONOMICS_PANEL_CACHE_ROOT` takes precedence; otherwise the root is
/// `$HOME/.autonomics/panels` (or a temp fallback when HOME is unset). The
/// CLI installer, the runtime host, and the container panel cache all
/// resolve to the same location, so installed packages are mounted by the
/// runtime without copying.
pub fn default_panel_cache_root() -> PathBuf {
    if let Some(root) = std::env::var_os(PANEL_CACHE_ROOT_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(root);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Path::new(&home).join(".autonomics").join("panels");
    }
    std::env::temp_dir().join("autonomics").join("panels")
}

/// One live byte-accounting tick for an in-flight bundle download.
///
/// `downloaded_bytes` is cumulative across the bundle's concurrently
/// downloading files; `total_bytes` is the sum of its manifest file sizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    /// Bundle identity (`owner/name`).
    pub repo: String,
    /// Bytes staged so far for the bundle.
    pub downloaded_bytes: u64,
    /// Total expected bytes for the bundle.
    pub total_bytes: u64,
}

/// Callback receiving [`TransferProgress`] ticks while
/// [`LocalCatalog::install_repository_with_progress`] streams a bundle.
pub type ProgressSink<'a> = std::sync::Arc<dyn Fn(TransferProgress) + Send + Sync + 'a>;

/// Local cache of selected catalog packages.
///
/// Layout (schema v3+):
/// - `<root>/index.json`: [`CatalogIndex`] of installed versions
/// - `<root>/<owner>/<name>@<digest>/manifest.json` plus payload files
///
/// Installing a package verifies every file checksum before the entry becomes
/// visible in the local index. Runtime VFS mounts are generated from this
/// cache; the remote catalog is never mounted directly. Entry directories use
/// the panel cache convention, including completion markers and in-use locks.
///
/// `repository_prefix`, when set, lets the runtime derive a short-name alias
/// for each entry by stripping `{prefix}-` from the start of the repo. This
/// is the seam that keeps node constants owner-agnostic: the Rust code only
/// spells the canonical short name (e.g. `catalog-plink-ref-1000g-eur-binary`),
/// while the configured prefix picks which `owner/` namespace provides it.
pub struct LocalCatalog {
    root: PathBuf,
    repository_prefix: Option<String>,
    /// Serializes index read-modify-write cycles during concurrent installs.
    /// The network phase of [`Self::install_entry`] runs outside the lock;
    /// only the publish phase (directory rename + index rewrite) holds it.
    index_lock: tokio::sync::Mutex<()>,
}

impl LocalCatalog {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with_prefix(root, None)
    }

    /// Open a local catalog cache with an explicit package prefix.
    ///
    /// The prefix enables short-name alias keys in the DAG bundle registry,
    /// so node constants referencing the short name resolve to whichever
    /// owner the deployment has configured.
    pub fn open_with_prefix(
        root: impl Into<PathBuf>,
        repository_prefix: Option<String>,
    ) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("create catalog cache {}: {error}", root.display()))?;
        Ok(Self {
            root,
            repository_prefix,
            index_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Update the repository prefix used to derive short-name aliases.
    pub fn set_repository_prefix(&mut self, repository_prefix: Option<String>) {
        self.repository_prefix = repository_prefix;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn repository_prefix(&self) -> Option<&str> {
        self.repository_prefix.as_deref()
    }

    pub fn index(&self) -> Result<CatalogIndex> {
        let path = self.root.join("index.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let raw: RawCatalogIndex = serde_json::from_slice(&bytes).map_err(|error| {
                    format!("parse catalog cache index `{}`: {error}", path.display())
                })?;
                let index = raw
                    .into_v3(self.repository_prefix.as_deref())
                    .map_err(|error| {
                        format!("migrate catalog cache index `{}`: {error}", path.display())
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

    /// Panel-cache-compatible directory for one installed package version.
    pub fn entry_path(&self, entry: &CatalogEntry) -> PathBuf {
        // `<owner>/<name>@sha256:<digest>` keeps each package under a
        // dedicated owner subdirectory; two packages from the same owner
        // never share a parent directory.
        self.root.join(entry.cache_dir_name())
    }

    fn is_installed(&self, entry: &CatalogEntry) -> bool {
        let dir = self.entry_path(entry);
        dir.join("manifest.json").is_file() && dir.join(PANEL_CACHE_COMPLETE_MARKER).is_file()
    }

    /// Whether the current entry for `repo` is fully installed in this cache
    /// (manifest plus completion marker), matching what [`Self::bundle_registry`]
    /// exposes to the DAG runtime. Purely local — never touches the network,
    /// so a provisioned deployment can be checked offline.
    ///
    /// An unreadable index reports `false`; the subsequent install attempt
    /// re-reads the index and surfaces the real error.
    pub fn is_repository_installed(&self, repo: &str) -> bool {
        let Ok(index) = self.index() else {
            return false;
        };
        index
            .current_entries()
            .any(|entry| entry.repo.as_str() == repo && self.is_installed(entry))
    }

    /// Read and validate the cached manifest for an installed entry.
    pub fn manifest(&self, entry: &CatalogEntry) -> Result<DatasetManifest> {
        let path = self.entry_path(entry).join("manifest.json");
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
    ///
    /// `repo` must be the canonical `owner/name` HF identifier of the
    /// package repository. Version and digest pin the exact entry to
    /// install; either may be omitted to take the registry current version.
    pub async fn install(
        &self,
        remote: &RemoteCatalog,
        repo: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<CatalogEntry> {
        let index = remote.index().await?;
        let entry = index.select(repo, version, digest)?;
        self.install_entry(remote, &entry).await
    }

    /// Resolve and install the current entry from one package repository.
    pub async fn install_repository(
        &self,
        remote: &RemoteCatalog,
        repository: &str,
    ) -> Result<CatalogEntry> {
        self.install_repository_with_progress(remote, repository, None)
            .await
    }

    /// [`Self::install_repository`] with live byte-level progress: the
    /// sink receives cumulative `downloaded_bytes` for the bundle as its
    /// files stream in (throttling is the caller's concern).
    pub async fn install_repository_with_progress(
        &self,
        remote: &RemoteCatalog,
        repository: &str,
        progress: Option<ProgressSink<'_>>,
    ) -> Result<CatalogEntry> {
        let entry = remote
            .package_index(repository)
            .await?
            .select_current()
            .map_err(|error| {
                format!("package repository `{repository}` has no current entry: {error}")
            })?;
        self.install_entry_with_progress(remote, &entry, progress)
            .await
    }

    /// Download, verify, and install one already-resolved remote entry.
    ///
    /// Installing an entry whose repo and digest are already indexed is
    /// idempotent: the existing payload directory is kept and only the
    /// current-version pointer may be refreshed.
    ///
    /// Safe under concurrent installs: the network phase (manifest fetch +
    /// staging download into a private UUID directory) runs without the
    /// index lock; the publish phase (atomic rename + index rewrite) is
    /// serialized so concurrent read-modify-write cycles cannot lose
    /// entries.
    pub async fn install_entry(
        &self,
        remote: &RemoteCatalog,
        entry: &CatalogEntry,
    ) -> Result<CatalogEntry> {
        self.install_entry_with_progress(remote, entry, None).await
    }

    /// [`Self::install_entry`] with live byte-level progress.
    pub async fn install_entry_with_progress(
        &self,
        remote: &RemoteCatalog,
        entry: &CatalogEntry,
        progress: Option<ProgressSink<'_>>,
    ) -> Result<CatalogEntry> {
        {
            let _guard = self.index_lock.lock().await;
            let mut index = self.index()?;
            let known = index
                .entries
                .iter()
                .any(|existing| existing.repo == entry.repo && existing.digest == entry.digest);
            if known && self.is_installed(entry) {
                let current_is_entry = index.entries.iter().any(|existing| {
                    existing.repo == entry.repo && entry.current && existing.digest == entry.digest
                });
                if !current_is_entry {
                    index.upsert_current(entry.clone());
                    self.write_index(&index)?;
                }
                return Ok(entry.clone());
            }
        }

        // Network phase — concurrent-safe: staging is a private directory.
        let manifest = remote.manifest(entry).await?;
        let staged = self
            .stage_entry_with_progress(remote, entry, &manifest, progress)
            .await?;

        // Publish phase — serialized: rename plus index read-modify-write.
        let _guard = self.index_lock.lock().await;
        let final_dir = self.entry_path(entry);
        if let Some(parent) = final_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "create owner directory `{}` for cached entry: {error}",
                    parent.display()
                )
            })?;
        }
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir).map_err(|error| {
                format!("replace cached entry `{}`: {error}", final_dir.display())
            })?;
        }
        std::fs::rename(&staged, &final_dir)
            .map_err(|error| format!("publish cached entry `{}`: {error}", final_dir.display()))?;

        let mut index = self.index()?;
        index.upsert_current(entry.clone());
        self.write_index(&index)?;
        Ok(entry.clone())
    }

    /// Install current versions missing from repositories declared locally.
    pub async fn update(
        &self,
        remote: &RemoteCatalog,
        repo: Option<&str>,
    ) -> Result<Vec<CatalogEntry>> {
        let local_index = self.index()?;
        let mut updated = Vec::new();
        for repository in local_index.repositories.clone() {
            let entry = remote
                .package_index(repository.as_str())
                .await?
                .select_current()?;
            if repo.is_some_and(|value| entry.repo.as_str() != value) {
                continue;
            }
            let installed = local_index
                .entries
                .iter()
                .any(|existing| existing.repo == entry.repo && existing.digest == entry.digest);
            if !installed {
                updated.push(self.install_entry(remote, &entry).await?);
            }
        }
        Ok(updated)
    }

    /// Number of payload files downloaded concurrently while staging one
    /// entry. Files are independent objects; parallel fetch multiplies the
    /// effective throughput of large multi-file packages.
    const STAGE_FILE_CONCURRENCY: usize = 4;

    async fn stage_entry_with_progress(
        &self,
        remote: &RemoteCatalog,
        entry: &CatalogEntry,
        manifest: &DatasetManifest,
        progress: Option<ProgressSink<'_>>,
    ) -> Result<PathBuf> {
        let staged = self.root.join(format!(
            ".downloading-{}@{}-{}",
            entry.repo,
            entry.digest,
            uuid::Uuid::new_v4()
        ));
        // Cumulative per-bundle byte accounting across concurrently
        // downloading files, folded into `TransferProgress` ticks.
        let bundle_total: u64 = manifest.files.iter().map(|file| file.size).sum();
        let per_file: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let repo = entry.repo.to_string();
        // Resolve borrowed manifest data into owned per-file jobs before
        // building futures: a closure borrowing `&DatasetFile` that returns
        // an async block trips `FnOnce`-is-not-general-enough once the
        // future crosses an async-trait boundary.
        let jobs: Vec<(String, PathBuf, u64, String)> = manifest
            .files
            .iter()
            .map(|file| {
                (
                    entry.source_payload_path(&file.path),
                    staged.join(&file.path),
                    file.size,
                    file.sha256.clone(),
                )
            })
            .collect();
        let downloads = futures::stream::iter(jobs)
            .map(|(key, target, size, sha256)| {
                let progress = progress.clone();
                let per_file = per_file.clone();
                let repo = repo.clone();
                let file_path = key.rsplit('/').next().unwrap_or(&key).to_string();
                async move {
                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|error| {
                            format!("create staging directory `{}`: {error}", parent.display())
                        })?;
                    }
                    let reporter = progress.map(|sink| {
                        let per_file = per_file.clone();
                        let repo = repo.clone();
                        let report = move |bytes: u64| {
                            let mut files = per_file.lock().unwrap();
                            files.insert(file_path.clone(), bytes);
                            let downloaded = files.values().sum::<u64>();
                            sink(TransferProgress {
                                repo: repo.clone(),
                                downloaded_bytes: downloaded,
                                total_bytes: bundle_total,
                            });
                        };
                        std::sync::Arc::new(report) as std::sync::Arc<dyn Fn(u64) + Send + Sync>
                    });
                    download_and_verify(
                        remote.source(),
                        &key,
                        &target,
                        size,
                        &sha256,
                        reporter.as_ref().map(|report| report.as_ref()),
                    )
                    .await
                }
            })
            .buffer_unordered(Self::STAGE_FILE_CONCURRENCY);
        if let Err(error) = downloads.try_collect::<Vec<()>>().await {
            // A failed or cancelled staging leaves no partial directory
            // behind; the UUID naming only tolerated orphans, it never
            // reused them.
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
        }
        let manifest_bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| format!("encode local manifest: {error}"))?;
        tokio::fs::write(staged.join("manifest.json"), manifest_bytes)
            .await
            .map_err(|error| format!("write staged manifest `{}`: {error}", staged.display()))?;
        tokio::fs::write(
            staged.join(PANEL_CACHE_COMPLETE_MARKER),
            format!("digest={}\n", entry.digest),
        )
        .await
        .map_err(|error| format!("write staged marker `{}`: {error}", staged.display()))?;
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
            // Cache directory layout is `<owner>/<name>@sha256:<digest>`.
            let source = entry.cache_dir_name();
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
    ///
    /// Each entry is keyed primarily by its HF repo (`owner/name`). When the
    /// catalog is opened with a `repository_prefix` (e.g. `wjixiang/catalog`),
    /// a secondary alias key is registered for every entry whose repo starts
    /// with `{prefix}-`, derived by stripping that prefix. This is how node
    /// constants stay owner-agnostic: they spell only the canonical short
    /// name (e.g. `catalog-plink-ref-1000g-eur-binary`), and the configured
    /// prefix decides which `owner/` namespace provides it.
    pub fn bundle_registry(&self) -> Result<dag_core::BundleRegistry> {
        let index = self.index()?;
        let mut registry = dag_core::BundleRegistry::new();
        for entry in index.current_entries() {
            let mut bundle = dag_core::DataBundle::new(
                entry.repo.to_string(),
                format!("{} {} catalog dataset", entry.repo, entry.version),
                entry.vfs_alias(),
            );
            bundle.source = Some(entry.vfs_immutable());
            bundle.digest = Some(entry.digest.clone());
            registry.register(bundle.clone()).map_err(|err| {
                crate::error::Error::from(format!(
                    "register primary catalog bundle for repo `{}`: {err}",
                    entry.repo
                ))
            })?;
            if let Some(alias) =
                short_name_alias(entry.repo.as_str(), self.repository_prefix.as_deref())
            {
                let mut alias_bundle = bundle;
                alias_bundle.ident = alias.to_string();
                // Alias is best-effort: if another owner happens to share the
                // short name we keep the first registration (the primary key
                // remains unique), but we still try to attach the alias for
                // owner-specific configurations.
                let _ = registry.register(alias_bundle);
            }
        }
        Ok(registry)
    }
}

/// Strip `{prefix}-` from the start of `repo` to recover the canonical short
/// name. Returns `None` when no prefix is configured or the repo does not
/// belong to the configured prefix owner.
fn short_name_alias<'a>(repo: &'a str, prefix: Option<&str>) -> Option<&'a str> {
    let prefix = prefix?;
    let prefix_with_dash = format!("{prefix}-");
    repo.strip_prefix(prefix_with_dash.as_str())
}

async fn download_and_verify(
    source: &dyn ObjectSource,
    key: &str,
    target: &Path,
    expected_size: u64,
    expected_sha256: &str,
    report: Option<&(dyn Fn(u64) + Send + Sync)>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(target)
        .await
        .map_err(|error| format!("create cached file `{}`: {error}", target.display()))?;
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    while offset < expected_size {
        let end = (offset + DOWNLOAD_CHUNK_BYTES).min(expected_size);
        let chunk = source.read_range(key, offset, end - offset).await?;
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
        if let Some(report) = report {
            report(offset);
        }
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
    use crate::package::{BuildOptions, build_package};
    use crate::remote::test_utils::MapSource;
    use vfs::{
        BackendConfig, BackendDefinition, MountedObjectStore, OpendalFileStorage, VfsManifest,
    };

    #[test]
    fn panel_cache_root_env_overrides_default() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                // SAFETY: no other data-catalog test reads this variable.
                unsafe { std::env::remove_var(PANEL_CACHE_ROOT_ENV) };
            }
        }
        let _reset = Reset;
        // SAFETY: see Reset above.
        unsafe { std::env::set_var(PANEL_CACHE_ROOT_ENV, "/tmp/panel-cache-override") };
        assert_eq!(
            default_panel_cache_root(),
            PathBuf::from("/tmp/panel-cache-override")
        );
    }

    async fn published_fixture(repo: &str) -> (RemoteCatalog, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let input = workspace.path().join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("data.txt"), b"local-cache-data").unwrap();
        let package = build_package(
            &input,
            workspace.path().join("package"),
            BuildOptions {
                repo: Some(repo.into()),
                version: Some("v1".into()),
                kind: Some("table".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let manifest = package.manifest;
        let entry = CatalogEntry {
            repo: crate::model::HfRepoId::new(repo).unwrap(),
            version: manifest.version.clone(),
            kind: manifest.kind.clone(),
            digest: manifest.digest.clone().expect("fixture has digest"),
            current: true,
            created_unix_seconds: 1,
        };
        let package_index = CatalogIndex {
            entries: vec![entry.clone()],
            ..CatalogIndex::default()
        };
        let registry = CatalogIndex {
            repositories: vec![entry.repo.clone()],
            ..CatalogIndex::default()
        };
        let mut objects = MapSource::default();
        objects.0.insert(
            "owner/catalog-index/index.json".into(),
            serde_json::to_vec(&registry).unwrap(),
        );
        objects.0.insert(
            entry.source_manifest_key(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        objects.0.insert(
            entry.source_payload_path("data.txt"),
            b"local-cache-data".to_vec(),
        );
        objects.0.insert(
            format!("{}/index.json", entry.repo),
            serde_json::to_vec(&package_index).unwrap(),
        );
        let config = crate::CatalogConfig {
            repository: Some("owner/catalog-index".into()),
            ..crate::CatalogConfig::default()
        };
        let remote = RemoteCatalog::from_source(config, Box::new(objects));
        (remote, workspace)
    }

    #[tokio::test]
    async fn installs_verifies_and_mounts_a_package() {
        let (remote, _warehouse) = published_fixture("owner/cache-panel").await;
        let cache_root = tempfile::tempdir().unwrap();
        let catalog = LocalCatalog::open(cache_root.path()).unwrap();

        let entry = catalog
            .install(&remote, "owner/cache-panel", None, None)
            .await
            .unwrap();
        assert_eq!(entry.version, "v1");
        assert!(entry.current);

        let index = catalog.index().unwrap();
        assert_eq!(index.current_entries().count(), 1);
        let manifest = catalog.manifest(&entry).unwrap();
        assert_eq!(manifest.files[0].path, "data.txt");

        let payload = catalog.entry_path(&entry).join("data.txt");
        assert_eq!(std::fs::read(payload).unwrap(), b"local-cache-data");
        // Layout: `<owner>/<name>@<digest>`.
        let owner_dir = catalog
            .entry_path(&entry)
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(owner_dir, "owner");
        assert_eq!(
            catalog
                .entry_path(&entry)
                .file_name()
                .unwrap()
                .to_string_lossy(),
            format!("cache-panel@{}", entry.digest)
        );
        assert_eq!(
            std::fs::read_to_string(catalog.entry_path(&entry).join(PANEL_CACHE_COMPLETE_MARKER))
                .unwrap(),
            format!("digest={}\n", entry.digest)
        );

        // Idempotent reinstall keeps the payload and current pointer stable.
        catalog
            .install(&remote, "owner/cache-panel", None, None)
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
        let path = storage.resolve_path("/bundles/owner/cache-panel/data.txt");
        let bytes = storage
            .resolve("/bundles/owner/cache-panel/data.txt")
            .read(&path)
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), b"local-cache-data");

        let registry = catalog.bundle_registry().unwrap();
        let bundle = registry.get("owner/cache-panel").unwrap();
        assert_eq!(bundle.vpath.as_str(), "/bundles/owner/cache-panel");
        assert_eq!(
            bundle.source.as_deref(),
            Some(entry.vfs_immutable().as_str())
        );
        assert_eq!(bundle.digest.as_deref(), Some(entry.digest.as_str()));
    }

    #[tokio::test]
    async fn is_repository_installed_reflects_cache_state_without_network() {
        let (remote, _warehouse) = published_fixture("owner/probe-panel").await;
        let cache_root = tempfile::tempdir().unwrap();
        let catalog = LocalCatalog::open(cache_root.path()).unwrap();

        assert!(!catalog.is_repository_installed("owner/probe-panel"));
        catalog
            .install_repository(&remote, "owner/probe-panel")
            .await
            .unwrap();
        assert!(catalog.is_repository_installed("owner/probe-panel"));
        assert!(!catalog.is_repository_installed("owner/other-panel"));
    }

    #[tokio::test]
    async fn progress_ticks_report_cumulative_bytes_up_to_total() {
        let (remote, _warehouse) = published_fixture("owner/progress-panel").await;
        let cache_root = tempfile::tempdir().unwrap();
        let catalog = LocalCatalog::open(cache_root.path()).unwrap();

        let ticks: std::sync::Arc<std::sync::Mutex<Vec<TransferProgress>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = ticks.clone();
        let sink: ProgressSink<'_> = std::sync::Arc::new(move |tick| {
            recorder.lock().unwrap().push(tick);
        });
        let entry = catalog
            .install_repository_with_progress(&remote, "owner/progress-panel", Some(sink))
            .await
            .unwrap();

        let ticks = ticks.lock().unwrap();
        assert!(!ticks.is_empty(), "download must produce at least one tick");
        assert!(ticks.iter().all(|tick| tick.repo == entry.repo.as_str()));
        assert!(
            ticks
                .iter()
                .all(|tick| tick.downloaded_bytes <= tick.total_bytes),
            "cumulative bytes never exceed the bundle total"
        );
        let last = ticks.last().unwrap();
        assert_eq!(last.downloaded_bytes, last.total_bytes);
        assert_eq!(
            last.total_bytes,
            catalog
                .manifest(&entry)
                .unwrap()
                .files
                .iter()
                .map(|f| f.size)
                .sum::<u64>()
        );
    }

    #[tokio::test]
    async fn concurrent_installs_keep_all_index_entries() {
        // Two installs racing on one cache: the publish phase is
        // serialized by the index lock, so neither read-modify-write
        // cycle can drop the other's entry.
        let (remote_a, _warehouse_a) = published_fixture("owner/parallel-a").await;
        let (remote_b, _warehouse_b) = published_fixture("owner/parallel-b").await;
        let cache_root = tempfile::tempdir().unwrap();
        let catalog = LocalCatalog::open(cache_root.path()).unwrap();

        let (first, second) = tokio::join!(
            catalog.install_repository(&remote_a, "owner/parallel-a"),
            catalog.install_repository(&remote_b, "owner/parallel-b"),
        );
        first.unwrap();
        second.unwrap();

        let index = catalog.index().unwrap();
        for repo in ["owner/parallel-a", "owner/parallel-b"] {
            assert!(
                index
                    .current_entries()
                    .any(|entry| entry.repo.as_str() == repo),
                "`{repo}` must survive the concurrent install"
            );
        }
    }

    #[tokio::test]
    async fn bundle_registry_resolves_under_both_repo_and_short_name() {
        let (remote, _warehouse) = published_fixture("owner/cache-cache-panel").await;
        let cache_root = tempfile::tempdir().unwrap();
        let catalog =
            LocalCatalog::open_with_prefix(cache_root.path(), Some("owner/cache".to_string()))
                .unwrap();

        let entry = catalog
            .install(&remote, "owner/cache-cache-panel", None, None)
            .await
            .unwrap();

        let registry = catalog.bundle_registry().unwrap();

        // Primary: HF repo (owner/name) is the canonical identity.
        let by_repo = registry
            .get(entry.repo.as_str())
            .expect("repo key resolves");
        assert_eq!(by_repo.ident, entry.repo.as_str());
        assert_eq!(by_repo.vpath.as_str(), "/bundles/owner/cache-cache-panel");
        assert_eq!(by_repo.digest.as_deref(), Some(entry.digest.as_str()));

        // Short-name alias: derived from the configured prefix so node
        // constants that spell only the canonical short name resolve here.
        let by_short = registry.get("cache-panel").expect("short alias resolves");
        assert_eq!(by_short.ident, "cache-panel");
        assert_eq!(by_short.vpath.as_str(), by_repo.vpath.as_str());
        assert_eq!(by_short.digest.as_deref(), Some(entry.digest.as_str()));
    }
}
