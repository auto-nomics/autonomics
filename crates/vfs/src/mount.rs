//! Unix-style virtual filesystem mounts.
//!
//! A [`MountedObjectStore`] exposes several storage backends as one
//! namespace. Callers use ordinary paths such as `/data/ldscore/1000g_eur`;
//! the mount table rewrites them onto backend-specific keys before forwarding
//! the operation.
//!
//! Backends are declarative and can be loaded from TOML:
//!
//! ```toml
//! [[backend]]
//! id = "default"
//! type = "local"
//! root = "/data/autonomics"
//!
//! [[backend]]
//! id = "oss-prod"
//! type = "oss"
//! bucket = "autonomics-data"
//! endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
//!
//! [[mount]]
//! path = "/"
//! backend = "default"
//!
//! [[mount]]
//! path = "/data/ldscore"
//! backend = "oss-prod"
//! source = "/ld_score"
//! read_only = true
//! ```

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use datafusion::object_store::path::Path;
use datafusion::object_store::{
    Error as ObjectStoreError, GetOptions, GetResult, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, ObjectStoreExt, PutMultipartOptions, Result, PutOptions, PutPayload, PutResult,
};
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::OpendalFileStorage;

#[derive(Debug, Error)]
pub enum MountError {
    #[error("backend '{0}' is defined more than once")]
    DuplicateBackend(String),
    #[error("mount '{0}' is defined more than once")]
    DuplicateMount(String),
    #[error("mount '{path}' refers to unknown backend '{backend}'")]
    UnknownBackend { path: String, backend: String },
    #[error("invalid mount path '{0}': expected an absolute path")]
    InvalidMountPath(String),
    #[error("failed to build backend '{id}': {source}")]
    BuildBackend {
        id: String,
        source: opendal::Error,
    },
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
}

/// Declarative connection information for one storage backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BackendConfig {
    Local { root: String },
    S3 {
        bucket: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_access_key: Option<String>,
    },
    Oss {
        bucket: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_access_key: Option<String>,
    },
}

impl BackendConfig {
    pub fn local(root: impl Into<String>) -> Self {
        Self::Local { root: root.into() }
    }

    pub fn s3(
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        Self::S3 {
            bucket: bucket.into(),
            endpoint: None,
            region: Some(region.into()),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
        }
    }

    pub fn s3_compatible(
        bucket: impl Into<String>,
        endpoint: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        Self::S3 {
            bucket: bucket.into(),
            endpoint: Some(endpoint.into()),
            region: Some(region.into()),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
        }
    }

    pub fn oss(
        bucket: impl Into<String>,
        endpoint: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        Self::Oss {
            bucket: bucket.into(),
            endpoint: Some(endpoint.into()),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
        }
    }

    fn build(&self) -> Result<opendal::Operator, opendal::Error> {
        match self {
            Self::Local { root } => opendal::Operator::new(
                opendal::services::Fs::default().root(root),
            )
            .map(|op| op.finish()),
            Self::S3 {
                bucket,
                endpoint,
                region,
                access_key_id,
                secret_access_key,
            } => {
                let mut builder = opendal::services::S3::default().bucket(bucket);
                if let Some(endpoint) = endpoint {
                    builder = builder.endpoint(endpoint);
                }
                if let Some(region) = region {
                    builder = builder.region(region);
                }
                if let (Some(ak), Some(sk)) = (access_key_id, secret_access_key) {
                    builder = builder.access_key_id(ak).secret_access_key(sk);
                }
                opendal::Operator::new(builder).map(|op| op.finish())
            }
            Self::Oss {
                bucket,
                endpoint,
                access_key_id,
                secret_access_key,
            } => {
                let mut builder = opendal::services::Oss::default().bucket(bucket);
                if let Some(endpoint) = endpoint {
                    builder = builder.endpoint(endpoint);
                }
                if let (Some(ak), Some(sk)) = (access_key_id, secret_access_key) {
                    builder = builder.access_key_id(ak).access_key_secret(sk);
                }
                opendal::Operator::new(builder).map(|op| op.finish())
            }
        }
    }
}

/// A backend entry in `vfs.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDefinition {
    pub id: String,
    #[serde(flatten)]
    pub config: BackendConfig,
}

/// A mount entry in `vfs.toml`, analogous to a Linux bind mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountDefinition {
    /// Absolute virtual mount point, for example `/data/ldscore`.
    pub path: String,
    /// Backend id from `[backend]`.
    pub backend: String,
    /// Source path being mounted.
    ///
    /// For a local backend this can be an absolute host path or a path
    /// relative to the backend root. For object stores it is the prefix inside
    /// the configured bucket. `/` (or the legacy empty value) mounts the
    /// backend root.
    #[serde(alias = "remote")]
    pub source: String,
    /// Reject mutations through this mount.
    #[serde(default)]
    pub read_only: bool,
}

/// Complete declarative mount manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VfsManifest {
    #[serde(default)]
    pub backend: Vec<BackendDefinition>,
    #[serde(default)]
    pub mount: Vec<MountDefinition>,
}

impl VfsManifest {
    pub fn from_toml(source: &str) -> Result<Self, MountError> {
        Ok(toml::from_str(source)?)
    }

    pub fn local_root(root: impl Into<String>) -> Self {
        Self {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: BackendConfig::local("/"),
            }],
            mount: vec![MountDefinition {
                path: "/".into(),
                backend: "default".into(),
                source: root.into(),
                read_only: false,
            }],
        }
    }
}

#[derive(Clone)]
struct Mount {
    virtual_prefix: Path,
    source_prefix: Path,
    store: Arc<dyn ObjectStore>,
    read_only: bool,
    definition: MountDefinition,
}

/// Resolve a mount's declarative source path to a backend key.
fn backend_source_key(config: &BackendConfig, source: &str) -> Result<String, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Ok(String::new());
    }

    match config {
        BackendConfig::Local { root } => {
            let root = lexical_absolute(PathBuf::from(root));
            let source_path = PathBuf::from(trimmed);
            let source_abs = if source_path.is_absolute() {
                lexical_absolute(source_path)
            } else {
                lexical_absolute(root.join(source_path))
            };
            source_abs
                .strip_prefix(&root)
                .map(|relative| relative.to_string_lossy().to_string())
                .map_err(|_| {
                    format!(
                        "local source '{}' is outside backend root '{}'",
                        trimmed,
                        root.display()
                    )
                })
        }
        BackendConfig::S3 { .. } | BackendConfig::Oss { .. } => {
            Ok(trimmed.trim_start_matches('/').to_string())
        }
    }
}

/// Lexically normalize a path without resolving symlinks or requiring it to
/// exist. Mount validation must remain usable for paths created later.
fn lexical_absolute(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// A prefix-routed DataFusion `ObjectStore`.
///
/// Paths are matched against the longest virtual prefix, rewritten to the
/// backend key, and forwarded to that backend's opendal-backed store.
#[derive(Clone, Default)]
pub struct MountedObjectStore {
    mounts: Vec<Mount>,
}

impl MountedObjectStore {
    pub fn from_manifest(manifest: &VfsManifest) -> Result<Self, MountError> {
        let mut backends = HashMap::new();
        for definition in &manifest.backend {
            if backends.insert(definition.id.clone(), definition).is_some() {
                return Err(MountError::DuplicateBackend(definition.id.clone()));
            }
        }

        let mut mounts = Vec::new();
        let mut seen_mount_paths = HashMap::new();
        for definition in &manifest.mount {
            if !definition.path.starts_with('/') {
                return Err(MountError::InvalidMountPath(definition.path.clone()));
            }
            let backend = backends
                .get(&definition.backend)
                .ok_or_else(|| MountError::UnknownBackend {
                    path: definition.path.clone(),
                    backend: definition.backend.clone(),
                })?;
            let operator = backend.config.build().map_err(|source| MountError::BuildBackend {
                id: backend.id.clone(),
                source,
            })?;
            let store: Arc<dyn ObjectStore> =
                Arc::new(OpendalFileStorage::from_operator(operator));
            if seen_mount_paths.insert(definition.path.clone(), mounts.len()).is_some() {
                return Err(MountError::DuplicateMount(definition.path.clone()));
            }
            let virtual_prefix = Path::parse(&definition.path)
                .map_err(|e| MountError::Config(format!("mount path '{}': {e}", definition.path)))?;
            let source_key = backend_source_key(&backend.config, &definition.source)
                .map_err(|e| MountError::Config(format!("mount '{}': {e}", definition.path)))?;
            let source_prefix = Path::parse(&source_key).map_err(|e| {
                MountError::Config(format!("mount source '{}': {e}", definition.source))
            })?;

            mounts.push(Mount {
                virtual_prefix,
                source_prefix,
                store,
                read_only: definition.read_only,
                definition: definition.clone(),
            });
        }

        // Longest prefix first. This makes nested mounts override a root mount.
        mounts.sort_by(|a, b| {
            b.virtual_prefix
                .as_ref()
                .len()
                .cmp(&a.virtual_prefix.as_ref().len())
                .then_with(|| a.virtual_prefix.as_ref().cmp(b.virtual_prefix.as_ref()))
        });
        Ok(Self { mounts })
    }

    pub fn mount_paths(&self) -> Vec<String> {
        self.mounts
            .iter()
            .map(|m| m.definition.path.clone())
            .collect()
    }

    fn find(&self, path: &Path) -> Option<&Mount> {
        self.mounts
            .iter()
            .find(|mount| path.prefix_matches(&mount.virtual_prefix))
    }

    fn resolve<'a>(&'a self, path: &'a Path) -> Result<(&'a Mount, Path), ObjectStoreError> {
        let mount = self.find(path).ok_or_else(|| not_mounted(path))?;
        let suffix = path
            .prefix_match(&mount.virtual_prefix)
            .map(|parts| parts.map(|part| part.as_ref().to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut remote = mount.source_prefix.clone();
        for part in suffix {
            remote = remote.join(part);
        }
        Ok((mount, remote))
    }


    fn reject_write(&self, path: &Path) -> Result<(), ObjectStoreError> {
        if self
            .find(path)
            .map(|mount| mount.read_only)
            .unwrap_or(false)
        {
            return Err(ObjectStoreError::NotSupported {
                source: "read-only VFS mount".into(),
            });
        }
        Ok(())
    }
}

impl fmt::Display for MountedObjectStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MountedObjectStore({} mounts)", self.mounts.len())
    }
}

impl fmt::Debug for MountedObjectStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountedObjectStore")
            .field("mounts", &self.mount_paths())
            .finish()
    }
}

fn not_mounted(path: &Path) -> ObjectStoreError {
    ObjectStoreError::Generic {
        store: "vfs",
        source: format!("path '{}' is not covered by a VFS mount", path).into(),
    }
}

#[async_trait::async_trait]
impl ObjectStore for MountedObjectStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        self.reject_write(location)?;
        let (mount, remote) = self.resolve(location).map_err(|_| not_mounted(location))?;
        mount.store.put_opts(&remote, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> Result<Box<dyn MultipartUpload>> {
        self.reject_write(location)?;
        let (mount, remote) = self.resolve(location).map_err(|_| not_mounted(location))?;
        mount.store.put_multipart_opts(&remote, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let (mount, remote) = self.resolve(location).map_err(|_| not_mounted(location))?;
        let result = mount.store.get_opts(&remote, options).await?;
        let GetResult {
            payload,
            mut meta,
            range,
            attributes,
        } = result;
        meta.location = location.clone();
        Ok(GetResult {
            payload,
            meta,
            range,
            attributes,
        })
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta, ObjectStoreError>> {
        let Some(prefix) = prefix else {
            return futures::stream::iter(Err(not_mounted(&Path::ROOT))).boxed();
        };
        let Ok((mount, remote)) = self.resolve(prefix) else {
            return futures::stream::iter(Err(not_mounted(prefix))).boxed();
        };
        let mount_prefix = mount.virtual_prefix.clone();
        let source_prefix = mount.source_prefix.clone();

        mount
            .store
            .list(Some(&remote))
            .map(move |meta| {
                let mut meta = meta?;
                meta.location = remap_location(&meta.location, &source_prefix, &mount_prefix)?;
                Ok(meta)
            })
            .boxed()
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        let Some(prefix) = prefix else {
            return Err(not_mounted(&Path::ROOT));
        };
        let (mount, remote) = self.resolve(prefix).map_err(|_| not_mounted(prefix))?;
        let result = mount.store.list_with_delimiter(Some(&remote)).await?;
        let mount_prefix = mount.virtual_prefix.clone();
        let source_prefix = mount.source_prefix.clone();

        let objects = result
            .objects
            .into_iter()
            .map(|mut meta| {
                meta.location =
                    remap_location(&meta.location, &source_prefix, &mount_prefix)?;
                Ok(meta)
            })
            .collect::<Result<Vec<_>, ObjectStoreError>>()?;
        let common_prefixes = result
            .common_prefixes
            .into_iter()
            .map(|location| {
                remap_location(&location, &source_prefix, &mount_prefix)
            })
            .collect::<Result<Vec<_>, ObjectStoreError>>()?;

        Ok(ListResult {
            objects,
            common_prefixes,
        })
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path, ObjectStoreError>>,
    ) -> BoxStream<'static, Result<Path, ObjectStoreError>> {
        let this = self.clone();
        locations
            .and_then(move |location| {
                let this = this.clone();
                async move {
                    this.reject_write(&location)?;
                    let (mount, remote) =
                        this.resolve(&location).map_err(|_| not_mounted(&location))?;
                    mount.store.delete(&remote).await?;
                    Ok(location)
                }
            })
            .boxed()
    }

    async fn copy_opts(&self, from: &Path, to: &Path, options: datafusion::object_store::CopyOptions) -> Result<()> {
        self.reject_write(to)?;
        let (from_mount, from_remote) = self.resolve(from).map_err(|_| not_mounted(from))?;
        let (to_mount, to_remote) = self.resolve(to).map_err(|_| not_mounted(to))?;
        if !Arc::ptr_eq(
            &from_mount.store,
            &to_mount.store,
        ) {
            return Err(ObjectStoreError::NotSupported {
                source: "cross-mount copy is not implemented".into(),
            });
        }
        from_mount.store.copy_opts(&from_remote, &to_remote, options).await
    }
}

fn remap_location(
    source: &Path,
    source_prefix: &Path,
    virtual_prefix: &Path,
) -> Result<Path, ObjectStoreError> {
    let suffix = source
        .prefix_match(source_prefix)
        .map(|parts| parts.map(|part| part.as_ref().to_string()).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut path = virtual_prefix.clone();
    for part in suffix {
        path = path.join(part);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::object_store::ObjectStoreExt;

    fn nested_manifest(root: &std::path::Path, nested: &std::path::Path) -> VfsManifest {
        VfsManifest {
            backend: vec![
                BackendDefinition {
                    id: "default".into(),
                    config: BackendConfig::local("/"),
                },
                BackendDefinition {
                    id: "nested".into(),
                    config: BackendConfig::local("/"),
                },
            ],
            mount: vec![
                MountDefinition {
                    path: "/".into(),
                    backend: "default".into(),
                    source: root.to_string_lossy().to_string(),
                    read_only: false,
                },
                MountDefinition {
                    path: "/data/panels".into(),
                    backend: "nested".into(),
                    source: nested.join("remote-prefix").to_string_lossy().to_string(),
                    read_only: true,
                },
            ],
        }
    }

    #[tokio::test]
    async fn longest_mount_rewrites_paths_and_lists_virtual_locations() {
        let root = tempfile::tempdir().unwrap();
        let nested = tempfile::tempdir().unwrap();
        let manifest = nested_manifest(root.path(), nested.path());
        let vfs = MountedObjectStore::from_manifest(&manifest).unwrap();

        // Physical backend key includes the mounted source; VFS callers do not.
         OpendalFileStorage::new(nested.path())
            .op
            .write("remote-prefix/panel.parquet", b"panel".to_vec())
            .await
            .unwrap();

        let location = Path::parse("/data/panels/panel.parquet").unwrap();
        let bytes = vfs.get(&location).await.unwrap().bytes().await.unwrap();
        assert_eq!(bytes.as_ref(), b"panel");

        let prefix = Path::parse("/data/panels/").unwrap();
        let metas: Vec<_> = vfs.list(Some(&prefix)).collect().await;
        assert_eq!(metas.len(), 1);
        assert_eq!(
            metas[0].as_ref().unwrap().location.as_ref(),
            "data/panels/panel.parquet"
        );

        let listed = vfs.list_with_delimiter(Some(&prefix)).await.unwrap();
        assert_eq!(listed.objects.len(), 1);
        assert_eq!(
            listed.objects[0].location.as_ref(),
            "data/panels/panel.parquet"
        );
    }

    #[tokio::test]
    async fn root_mount_reads_and_nested_mount_rejects_writes() {
        let root = tempfile::tempdir().unwrap();
        let nested = tempfile::tempdir().unwrap();
        let manifest = nested_manifest(root.path(), nested.path());
        let vfs = MountedObjectStore::from_manifest(&manifest).unwrap();

        let root_file = Path::parse("/scratch.txt").unwrap();
        vfs.put(&root_file, b"root".to_vec().into()).await.unwrap();
        assert_eq!(
            vfs.get(&root_file).await.unwrap().bytes().await.unwrap().as_ref(),
            b"root"
        );

        let nested_file = Path::parse("/data/panels/new.parquet").unwrap();
        assert!(vfs.put(&nested_file, b"forbidden".to_vec().into()).await.is_err());
    }

    #[test]
    fn local_source_outside_backend_root_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "local".into(),
                config: BackendConfig::local(root.path().to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/outside".into(),
                backend: "local".into(),
                source: outside.path().to_string_lossy().to_string(),
                read_only: true,
            }],
        };
        assert!(matches!(
            MountedObjectStore::from_manifest(&manifest),
            Err(MountError::Config(_))
        ));
    }

    #[test]
    fn duplicate_mounts_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let backend = BackendDefinition {
            id: "default".into(),
            config: BackendConfig::local(root.path().to_string_lossy().to_string()),
        };
        let mount = MountDefinition {
            path: "/data".into(),
            backend: "default".into(),
            source: "/".into(),
            read_only: false,
        };
        let manifest = VfsManifest {
            backend: vec![backend],
            mount: vec![mount.clone(), mount],
        };
        assert!(matches!(
            MountedObjectStore::from_manifest(&manifest),
            Err(MountError::DuplicateMount(_))
        ));
    }
}

#[cfg(test)]
mod single_file_tests {
    use super::*;
    use datafusion::object_store::ObjectStoreExt;

    #[tokio::test]
    async fn single_local_file_can_be_mounted_at_virtual_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("panel.parquet");
        std::fs::write(&source, b"single").unwrap();

        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "local".into(),
                config: BackendConfig::local("/"),
            }],
            mount: vec![MountDefinition {
                path: "/data/panel.parquet".into(),
                backend: "local".into(),
                source: source.to_string_lossy().to_string(),
                read_only: true,
            }],
        };
        let vfs = MountedObjectStore::from_manifest(&manifest).unwrap();
        let path = Path::parse("/data/panel.parquet").unwrap();

        assert_eq!(vfs.get(&path).await.unwrap().bytes().await.unwrap().as_ref(), b"single");
        assert!(vfs.put(&path, b"new".to_vec().into()).await.is_err());
    }
}

#[cfg(test)]
mod datafusion_tests {
    use super::*;
    use datafusion::execution::object_store::ObjectStoreUrl;
    use datafusion::prelude::{CsvReadOptions, SessionContext};

    #[tokio::test]
    async fn datafusion_reads_files_through_vfs_urls() {
        let root = tempfile::tempdir().unwrap();
        let manifest = VfsManifest::local_root(root.path().to_string_lossy().to_string());
        let vfs = MountedObjectStore::from_manifest(&manifest).unwrap();

        let path = Path::parse("/table.csv").unwrap();
        vfs.put(&path, b"id,value\n1,2\n".to_vec().into())
            .await
            .unwrap();

        let ctx = SessionContext::new();
        ctx.runtime_env().register_object_store(
            ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
            Arc::new(vfs),
        );

        let df = ctx
            .read_csv("vfs:///table.csv", CsvReadOptions::default())
            .await
            .unwrap();
        let batches = df.collect().await.unwrap();
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
    }
}
