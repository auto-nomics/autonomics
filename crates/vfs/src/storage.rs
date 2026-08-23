use std::collections::HashMap as StdHashMap;
use std::fmt::{Debug, Display};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use bytes::Bytes;
use chrono::{TimeZone, Utc};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::object_store::Error as ObjectStoreError;
use datafusion::object_store::{
    Attributes, CopyMode, CopyOptions, GetOptions, GetRange, GetResult, GetResultPayload,
    ListResult, MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions,
    PutPayload, PutResult, UploadPart, path::Path,
};
use datafusion::prelude::SessionContext;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt, TryStreamExt};
use opendal::Operator;
use opendal::services::Fs;
use tempfile::TempDir;

use crate::{MountHandle, MountedObjectStore};

pub struct OpendalFileStorage {
    pub op: Operator,
    pub mounts: Option<Arc<MountedObjectStore>>,
    path_locks: Arc<PathLocks>,
    /// Keeps the temp directory alive until this storage is dropped.
    _temp_guard: Option<TempDir>,
}

type PathLocks = std::sync::RwLock<StdHashMap<String, Arc<tokio::sync::RwLock<()>>>>;
static STAGING_OBJECT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl OpendalFileStorage {
    /// Normalize a user-supplied path to always start with `/` so that
    /// OpenDAL resolves it relative to the configured root (not the process cwd).
    ///
    /// Performs lexical `..` resolution with root clamping: `..` at the
    /// virtual root is silently ignored, so no path can escape the sandbox.
    /// - `"/foo"`              → `"/foo"`
    /// - `"foo"`               → `"/foo"`
    /// - `"./foo"`             → `"/foo"`
    /// - `"/foo/../bar"`       → `"/bar"`
    /// - `"/../etc/passwd"`    → `"/etc/passwd"` (clamped — still within virtual root)
    /// - `"a/../../../b"`      → `"/b"` (clamped)
    /// - `""`                  → `"/"`
    pub fn normalize_path(path: &str) -> String {
        let mut stack: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {} // skip empty and current-dir
                ".." => {
                    stack.pop(); // lexical parent; clamps at root
                }
                s => stack.push(s),
            }
        }
        if stack.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", stack.join("/"))
        }
    }

    /// Build a storage whose default backend is the local FS at
    /// `data_dir`, with a mount table layered on top. Paths under a
    /// mount are routed to that mount's backend operator (see
    /// [`Self::resolve`]); everything else falls through to the default
    /// local-FS operator.
    pub fn with_mounts(data_dir: &std::path::Path, mounts: Arc<MountedObjectStore>) -> Self {
        let op = Operator::new(Fs::default().root(data_dir.to_str().unwrap_or("/tmp/opendal")))
            .unwrap()
            .finish();
        Self {
            op,
            mounts: Some(mounts),
            path_locks: Arc::new(PathLocks::default()),
            _temp_guard: None,
        }
    }

    /// Wrap an existing OpenDAL operator (local FS, S3, OSS, ...) so it can
    /// be used as a DataFusion `ObjectStore`. No mount table attached.
    pub fn from_operator(op: Operator) -> Self {
        Self {
            op,
            mounts: None,
            path_locks: Arc::new(PathLocks::default()),
            _temp_guard: None,
        }
    }

    /// Create a new storage backed by the local filesystem at the given root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let op = Operator::new(Fs::default().root(root.to_str().unwrap_or("/tmp/opendal")))
            .unwrap()
            .finish();
        Self {
            op,
            mounts: None,
            path_locks: Arc::new(PathLocks::default()),
            _temp_guard: None,
        }
    }

    /// Create a storage backed by a temporary directory.
    /// The temporary directory is cleaned up when this storage is dropped.
    pub fn new_temp() -> Self {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let root = tmp.path().to_path_buf();
        let op = Operator::new(Fs::default().root(root.to_str().unwrap_or("/tmp/opendal")))
            .unwrap()
            .finish();
        Self {
            op,
            mounts: None,
            path_locks: Arc::new(PathLocks::default()),
            _temp_guard: Some(tmp),
        }
    }

    /// Return the per-object rw lock used to make stat/read and replacement
    /// mutually consistent. Locks are retained for the lifetime of the store;
    /// this is bounded by the number of distinct VFS objects used by it.
    fn object_lock(&self, path: &str) -> Arc<tokio::sync::RwLock<()>> {
        lock_object(&self.path_locks, path)
    }

    /// Resolve a virtual path to the concrete backend operator that
    /// owns it. Internal helper; public callers should use the `&str`
    /// wrapper [`Self::resolve`].
    fn dispatch_op(&self, path: &Path) -> Operator {
        if let Some(mounts) = &self.mounts {
            if let Some(handle) = mounts.handle_for(path) {
                return (*handle.backend_op).clone();
            }
        }
        self.op.clone()
    }

    /// Translate a virtual path to the key used on the resolved backend
    /// operator. Internal helper; public callers should use the `&str`
    /// wrapper [`Self::resolve_path`].
    fn dispatch_path(&self, path: &Path) -> String {
        if let Some(mounts) = &self.mounts {
            if let Some(handle) = mounts.handle_for(path) {
                return mount_key(&handle, path);
            }
        }
        path.to_string()
    }

    /// Construct the virtual-path form of a backend-local entry returned
    /// by a mount's backend operator: strip the mount's backend-relative
    /// source prefix, then re-attach the mount's virtual prefix. Used by
    /// the [`ObjectStore`] trait impls so callers see entries under the
    /// mount's virtual namespace (e.g. `mnt/.../parquet/1000g.parquet`
    /// becomes `/data/ldsc/1000g.parquet`).
    fn remap_to_virtual(remote_location: &Path, handle: &MountHandle) -> Path {
        let source_key = handle.source_prefix.as_ref().trim_matches('/');
        let remote = remote_location.as_ref().trim_matches('/');
        let suffix = if source_key.is_empty() {
            remote.to_string()
        } else {
            remote
                .strip_prefix(source_key)
                .map(|s| s.trim_start_matches('/').to_string())
                .unwrap_or_else(|| remote.to_string())
        };
        let prefix = handle.definition.path.trim_end_matches('/');
        let joined = match (prefix.is_empty(), suffix.is_empty()) {
            (true, true) => "/".to_string(),
            (true, false) => format!("/{suffix}"),
            (false, true) => prefix.to_string(),
            (false, false) => format!("{prefix}/{suffix}"),
        };
        Path::parse(&joined).unwrap_or(Path::ROOT)
    }

    /// Reject writes through read-only mounts. Internal helper; public
    /// callers should use the `&str` wrapper [`Self::check_writable`].
    fn check_writable_path(&self, path: &Path) -> Result<(), ObjectStoreError> {
        if let Some(mounts) = &self.mounts {
            if let Some(handle) = mounts.handle_for(path) {
                if handle.read_only {
                    return Err(ObjectStoreError::NotSupported {
                        source: format!("mount '{}' is read-only", handle.definition.path).into(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Public `&str` wrapper around [`Self::dispatch_op`]. The path is
    /// first normalised via [`Self::normalize_path`] so lexical `..`
    /// segments are clamped to the virtual root before the mount
    /// lookup runs (avoids `/foo/../data/ldsc` escaping a mount that
    /// covers `/data/ldsc`).
    pub fn resolve(&self, path: &str) -> Operator {
        let v = Self::normalize_path(path);
        match Path::parse(&v) {
            Ok(p) => self.dispatch_op(&p),
            Err(_) => self.op.clone(),
        }
    }

    /// Public `&str` wrapper around [`Self::dispatch_path`].
    pub fn resolve_path(&self, path: &str) -> String {
        let v = Self::normalize_path(path);
        match Path::parse(&v) {
            Ok(p) => self.dispatch_path(&p),
            Err(_) => v,
        }
    }

    /// Reject writes through read-only mounts. Callers should run this
    /// BEFORE dispatching a write/delete/rename/copy-target to a
    /// backend operator. Public `&str` wrapper.
    pub fn check_writable(&self, path: &str) -> Result<(), opendal::Error> {
        let v = Self::normalize_path(path);
        if let Ok(p) = Path::parse(&v) {
            if let Err(e) = self.check_writable_path(&p) {
                return Err(opendal::Error::new(
                    opendal::ErrorKind::PermissionDenied,
                    format!("{e}"),
                ));
            }
        }
        Ok(())
    }

    /// Atomically replace a virtual path with the supplied bytes.
    ///
    /// This is the direct-call form of the ObjectStore write path: bytes are
    /// first written to a staging key, then installed while holding the same
    /// per-object lock used by VFS reads and replacements.
    pub async fn write_bytes(
        &self,
        virtual_path: &str,
        content: Vec<u8>,
    ) -> Result<(), opendal::Error> {
        self.check_writable(virtual_path)?;
        let path = self.resolve_path(virtual_path);
        let operator = self.resolve(virtual_path);
        let staging_path = staging_key(&path);
        let buffer = opendal::Buffer::from(content);
        operator.write(&staging_path, buffer).await?;

        let lock = self.object_lock(&path);
        {
            let _guard = lock.write().await;
            if let Err(error) = operator.rename(&staging_path, &path).await {
                let _ = operator.delete(&staging_path).await;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Read a bounded byte range while holding the object's read lock.
    pub async fn read_range(
        &self,
        virtual_path: &str,
        range: std::ops::Range<u64>,
    ) -> Result<opendal::Buffer, opendal::Error> {
        let operator = self.resolve(virtual_path);
        let key = self.resolve_path(virtual_path);
        let lock = self.object_lock(&key);
        let _guard = lock.read().await;
        operator.read_with(&key).range(range).await
    }

    /// Stream a byte range while retaining ownership of the object read lock.
    ///
    /// The returned stream is `'static`, allowing HTTP responses to remain
    /// chunked without losing coordination with replacement writers.
    pub async fn read_stream(
        &self,
        virtual_path: &str,
        range: std::ops::Range<u64>,
    ) -> Result<impl Stream<Item = Result<Bytes, opendal::Error>> + Send + 'static, opendal::Error>
    {
        let operator = self.resolve(virtual_path);
        let key = self.resolve_path(virtual_path);
        let lock = self.object_lock(&key);
        let guard = lock.read_owned().await;
        let reader = operator.reader(&key).await?;
        let inner = reader.into_stream(range).await?;

        Ok(futures::stream::unfold(
            (guard, inner),
            |(guard, mut inner)| async move {
                inner.next().await.map(|item| {
                    let item = item.map(|buffer| buffer.to_bytes());
                    (item, (guard, inner))
                })
            },
        ))
    }

    /// Return an object's byte length.
    pub async fn content_length(&self, virtual_path: &str) -> Result<u64, opendal::Error> {
        let operator = self.resolve(virtual_path);
        let key = self.resolve_path(virtual_path);
        let meta = operator.stat(&key).await?;
        Ok(meta.content_length())
    }

    /// Delete an object while holding its replacement lock.
    pub async fn delete_object(&self, virtual_path: &str) -> Result<(), opendal::Error> {
        self.check_writable(virtual_path)?;
        let operator = self.resolve(virtual_path);
        let key = self.resolve_path(virtual_path);
        let lock = self.object_lock(&key);
        let _guard = lock.write().await;
        match operator.delete(&key).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Snapshot of all mount paths (in longest-prefix-first order).
    /// Empty when no mount table is attached.
    pub fn mount_paths(&self) -> Vec<String> {
        self.mounts
            .as_ref()
            .map(|m| m.mount_paths())
            .unwrap_or_default()
    }

    /// Snapshot of full mount definitions, useful for `mount_list`
    /// introspection (path / backend / source / read_only).
    pub fn mount_definitions(&self) -> Vec<crate::MountDefinition> {
        self.mounts
            .as_ref()
            .map(|m| m.mount_definitions())
            .unwrap_or_default()
    }

    /// Whether a virtual path is covered by an attached mount table.
    pub fn is_mounted(&self, path: &str) -> bool {
        let Some(mounts) = &self.mounts else {
            return false;
        };
        let normalized = Self::normalize_path(path);
        Path::parse(&normalized)
            .map(|path| mounts.handle_for(&path).is_some())
            .unwrap_or(false)
    }

    /// Default local-FS operator. Used by download tools
    /// (OpenGWAS, GWAS Catalog) that MUST land files on a writable
    /// local path even when the user has read-only mounts configured.
    pub fn downloads_op(&self) -> &Operator {
        &self.op
    }

    /// Return true iff `path_a` and `path_b` resolve to the same
    /// backend operator. Used by `cp`/`mv` to decide between
    /// backend-internal rename/copy and an explicit read+write.
    pub fn same_backend(&self, path_a: &str, path_b: &str) -> bool {
        if let Some(mounts) = &self.mounts {
            use datafusion::object_store::path::Path as DsPath;
            let va = OpendalFileStorage::normalize_path(path_a);
            let vb = OpendalFileStorage::normalize_path(path_b);
            let ha = DsPath::parse(&va).ok().and_then(|p| mounts.handle_for(&p));
            let hb = DsPath::parse(&vb).ok().and_then(|p| mounts.handle_for(&p));
            match (ha, hb) {
                (Some(a), Some(b)) => {
                    // Same mount only if both backend_op Arc pointers match.
                    std::sync::Arc::ptr_eq(&a.backend_op, &b.backend_op)
                }
                (Some(_), None) | (None, Some(_)) => false,
                (None, None) => true, // both fall through to default local FS
            }
        } else {
            true
        }
    }

    pub fn register_to_ctx(self) -> (SessionContext, Arc<OpendalFileStorage>) {
        let ctx = SessionContext::new();
        let object_store = Arc::new(self);

        ctx.register_object_store(
            ObjectStoreUrl::parse("file://").unwrap().as_ref(),
            object_store.clone(),
        );
        (ctx, object_store)
    }

    /// Translate a backend-local entry path (as returned by a resolved
    /// operator's lister) back into the virtual namespace: strip the
    /// covering mount's backend-relative source prefix, then re-attach
    /// the mount's virtual prefix. Entries outside any mount pass
    /// through unchanged.
    ///
    /// `vpath` is the virtual path the listing was rooted at; it is
    /// only used to find the covering mount.
    pub fn remap_entry_to_virtual(&self, vpath: &str, entry_path: &str) -> String {
        let v = Self::normalize_path(vpath);
        let Ok(dp) = Path::parse(&v) else {
            return entry_path.to_string();
        };
        let Some(handle) = self.mounts.as_ref().and_then(|m| m.handle_for(&dp)) else {
            return entry_path.to_string();
        };
        let source_key = handle.source_prefix.as_ref().trim_matches('/');
        let remote = entry_path.trim_matches('/');
        let suffix = if source_key.is_empty() {
            remote.to_string()
        } else {
            remote
                .strip_prefix(source_key)
                .map(|s| s.trim_start_matches('/').to_string())
                .unwrap_or_else(|| remote.to_string())
        };
        let prefix = handle.definition.path.trim_end_matches('/');
        match (prefix.is_empty(), suffix.is_empty()) {
            (true, true) => "/".to_string(),
            (true, false) => format!("/{suffix}"),
            (false, true) => prefix.to_string(),
            (false, false) => format!("{prefix}/{suffix}"),
        }
    }
}

fn lock_object(path_locks: &PathLocks, path: &str) -> Arc<tokio::sync::RwLock<()>> {
    if let Some(lock) = path_locks
        .read()
        .ok()
        .and_then(|locks| locks.get(path).cloned())
    {
        return lock;
    }

    let Ok(mut locks) = path_locks.write() else {
        return Arc::new(tokio::sync::RwLock::new(()));
    };
    locks
        .entry(path.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(())))
        .clone()
}

impl Debug for OpendalFileStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpendalFileStorage")
            .field("op", &self.op)
            .finish()
    }
}

impl Display for OpendalFileStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpendalFileStorage({})", self.op.info().name())
    }
}

impl ObjectStore for OpendalFileStorage {
    fn put_opts<'life0, 'life1, 'async_trait>(
        &'life0 self,
        location: &'life1 Path,
        payload: PutPayload,
        _opts: PutOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<PutResult, ObjectStoreError>>
                + std::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        if let Err(e) = self.check_writable_path(location) {
            return Box::pin(async move { Err(e) });
        }
        let op = self.dispatch_op(location);
        let path = self.dispatch_path(location);
        let staging_path = staging_key(&path);
        Box::pin(async move {
            let total_len = payload.content_length();
            let mut buf = Vec::with_capacity(total_len);
            for chunk in payload.iter() {
                buf.extend_from_slice(chunk);
            }
            let buffer = opendal::Buffer::from(buf);
            op.write(&staging_path, buffer)
                .await
                .map_err(opendal_to_object_store_error)?;

            let lock = self.object_lock(&path);
            {
                let _guard = lock.write().await;
                if let Err(err) = op.rename(&staging_path, &path).await {
                    let _ = op.delete(&staging_path).await;
                    return Err(opendal_to_object_store_error(err));
                }
            }

            let e_tag = op
                .stat(&path)
                .await
                .ok()
                .and_then(|m| m.etag().map(String::from));
            Ok(PutResult {
                e_tag,
                version: None,
            })
        })
    }

    fn put_multipart_opts<'life0, 'life1, 'async_trait>(
        &'life0 self,
        location: &'life1 Path,
        _opts: PutMultipartOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Box<dyn MultipartUpload>, ObjectStoreError>>
                + std::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        if let Err(e) = self.check_writable_path(location) {
            return Box::pin(async move { Err(e) });
        }
        let op = self.dispatch_op(location);
        let path = self.dispatch_path(location);
        let staging_path = staging_key(&path);
        let destination_lock = self.object_lock(&path);
        Box::pin(async move {
            let writer = op
                .writer(&staging_path)
                .await
                .map_err(opendal_to_object_store_error)?;
            let (part_tx, mut part_rx) = tokio::sync::mpsc::channel(1);
            let writer_task = tokio::spawn(async move {
                let mut writer = writer;
                let mut next_part = 0u64;
                let mut pending = std::collections::BTreeMap::new();
                while let Some((part_index, part)) = part_rx.recv().await {
                    pending.insert(part_index, part);
                    while let Some(part) = pending.remove(&next_part) {
                        writer.write(part).await?;
                        next_part += 1;
                    }
                }
                writer.close().await?;
                Ok(())
            });
            Ok(Box::new(OpendalMultipartUpload {
                part_tx: Some(part_tx),
                writer_task: Some(writer_task),
                next_part: 0,
                op,
                path,
                staging_path,
                destination_lock,
            }) as Box<dyn MultipartUpload>)
        })
    }

    fn get_opts<'life0, 'life1, 'async_trait>(
        &'life0 self,
        location: &'life1 Path,
        options: GetOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<GetResult, ObjectStoreError>>
                + std::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        // ObjectStore::get_opts resolves one virtual object to a byte stream.
        // Callers may supply a byte range; an absent range means the whole
        // object. Keep the OpenDAL reader lazy and chunked here: materializing
        // the range into one Buffer would force even schema-inference reads,
        // which request only a few KB before dropping the stream, to load an
        // entire multi-GB CSV/TSV file into memory.
        let op = self.dispatch_op(location);
        let path = self.dispatch_path(location);
        let object_lock = self.object_lock(&path);
        let mount_handle = self
            .mounts
            .as_ref()
            .and_then(|mounts| mounts.handle_for(location));
        Box::pin(async move {
            let _guard = object_lock.read().await;
            let meta = op
                .stat(&path)
                .await
                .map_err(opendal_to_object_store_error)?;
            let mut object_meta = opendal_meta_to_object_meta(&path, &meta);
            if let Some(handle) = mount_handle.as_ref() {
                object_meta.location =
                    Self::remap_to_virtual(&Path::parse(&path).unwrap_or_default(), handle);
            }
            let size = meta.content_length();

            let range = match options.range {
                Some(r) => r,
                None => GetRange::Bounded(0..size),
            };

            let byte_range = match range {
                GetRange::Bounded(r) => r,
                GetRange::Offset(start) => start..size,
                GetRange::Suffix(suffix) => size.saturating_sub(suffix)..size,
            };

            let reader = op
                .reader(&path)
                .await
                .map_err(opendal_to_object_store_error)?;
            let stream = reader
                .into_stream(byte_range.clone())
                .await
                .map_err(opendal_to_object_store_error)?
                .map_ok(|buffer| buffer.to_bytes())
                .map_err(opendal_to_object_store_error)
                .boxed();

            Ok(GetResult {
                payload: GetResultPayload::Stream(Box::pin(stream)),
                meta: object_meta,
                range: byte_range,
                attributes: Attributes::default(),
            })
        })
    }

    fn list(
        &self,
        prefix: Option<&Path>,
    ) -> BoxStream<'static, Result<ObjectMeta, ObjectStoreError>> {
        let effective = prefix.cloned().unwrap_or(Path::ROOT);

        // Snapshot mount metadata we need inside the stream.
        let dispatch_handle = self.mounts.as_ref().and_then(|m| m.handle_for(&effective));
        let child_mounts: Vec<(String, MountHandle)> = self
            .mounts
            .as_ref()
            .map(|m| {
                m.mount_paths()
                    .into_iter()
                    .filter_map(|p| {
                        let ds = Path::parse(&p).ok()?;
                        let parent = ds.parent().unwrap_or(Path::ROOT);
                        // Keep only mounts that are descendants of
                        // the scan root (not the scan root itself).
                        if !effective.prefix_matches(&parent) || parent == Path::ROOT {
                            return None;
                        }
                        m.handle_for(&ds).map(|h| (p, h))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Default-fs operator + scan path are always needed.
        let op = self.op.clone();
        let scan_path = effective.to_string();

        let stream = async_stream::stream! {
            // Case A: dispatch entirely into a single mount.
            if let Some(handle) = dispatch_handle {
                let op = (*handle.backend_op).clone();
                let key = mount_key(&handle, &effective);
                let scan = if key.is_empty() {
                    "/".to_string()
                } else {
                    key.clone()
                };
                if let Ok(meta) = op.stat(&key).await {
                    if meta.is_file() {
                        let mut object_meta = opendal_meta_to_object_meta(&key, &meta);
                        object_meta.location =
                            Self::remap_to_virtual(&Path::parse(&key).unwrap_or_default(), &handle);
                        yield Ok(object_meta);
                        return;
                    }
                }
                let mut lister = match op.lister_with(&scan).recursive(true).await {
                    Ok(l) => l,
                    Err(e) => {
                        yield Err(opendal_to_object_store_error(e));
                        return;
                    }
                };
                while let Some(entry) = lister.next().await {
                    match entry {
                        Ok(e) => {
                            if let Some(mut meta) = entry_to_meta(&e) {
                                meta.location = Self::remap_to_virtual(
                                    &Path::parse(e.path()).unwrap_or(Path::ROOT),
                                    &handle,
                                );
                                yield Ok(meta);
                            }
                        }
                        Err(e) => yield Err(opendal_to_object_store_error(e)),
                    }
                }
                return;
            }

            // Case B: composite listing — default fs + each child
            // mount merged.
            let lister = match op.lister_with(&scan_path).recursive(true).await {
                Ok(l) => l,
                Err(e) => {
                    yield Err(opendal_to_object_store_error(e));
                    return;
                }
            };
            let mut entries = lister;
            while let Some(entry) = entries.next().await {
                match entry {
                    Ok(e) => {
                        let entry_path = e.path().to_string();
                        let trimmed = entry_path.trim_end_matches('/');
                        if child_mounts
                            .iter()
                            .any(|(mp, _)| mp.trim_end_matches('/') == trimmed)
                        {
                            continue;
                        }
                        if e.metadata().is_file() {
                            if let Some(meta) = entry_to_meta(&e) {
                                yield Ok(meta);
                            }
                        }
                    }
                    Err(e) => yield Err(opendal_to_object_store_error(e)),
                }
            }
            for (vp, handle) in child_mounts {
                let key = mount_key(
                    &handle,
                    &Path::parse(&vp).unwrap_or(Path::ROOT),
                );
                let scan = if key.is_empty() {
                    "/".to_string()
                } else {
                    key
                };
                let op = (*handle.backend_op).clone();
                let mut lister = match op.lister_with(&scan).recursive(true).await {
                    Ok(l) => l,
                    Err(e) => {
                        yield Err(opendal_to_object_store_error(e));
                        continue;
                    }
                };
                while let Some(entry) = lister.next().await {
                    match entry {
                        Ok(e) => {
                            if let Some(mut meta) = entry_to_meta(&e) {
                                meta.location = Self::remap_to_virtual(
                                    &Path::parse(e.path()).unwrap_or(Path::ROOT),
                                    &handle,
                                );
                                yield Ok(meta);
                            }
                        }
                        Err(e) => yield Err(opendal_to_object_store_error(e)),
                    }
                }
            }
        };
        Box::pin(stream.boxed())
    }

    fn list_with_delimiter<'life0, 'life1, 'async_trait>(
        &'life0 self,
        prefix: Option<&'life1 Path>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ListResult, ObjectStoreError>>
                + std::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let effective = prefix.cloned().unwrap_or(Path::ROOT);

        // Case 1: path is STRICTLY under a mount (or IS a non-root
        // mount's root) → list inside that mount's backend. Only the
        // root mount itself (`/`) falls through to Case 2 so sibling
        // mounts at the top level are merged.
        if let Some(mounts) = &self.mounts {
            if let Some(handle) = mounts.handle_for(&effective) {
                let is_root_mount = handle.definition.path == "/" && effective == Path::ROOT;
                if !is_root_mount {
                    let op = (*handle.backend_op).clone();
                    let remote = self.dispatch_path(&effective);
                    return Box::pin(async move {
                        if let Ok(meta) = op.stat(&remote).await {
                            if meta.is_file() {
                                let mut object = opendal_meta_to_object_meta(&remote, &meta);
                                object.location = Self::remap_to_virtual(
                                    &Path::parse(&remote).unwrap_or_default(),
                                    &handle,
                                );
                                return Ok(ListResult {
                                    common_prefixes: Vec::new(),
                                    objects: vec![object],
                                });
                            }
                        }
                        let scan = if remote.is_empty() {
                            "/".to_string()
                        } else if remote.ends_with('/') {
                            remote
                        } else {
                            format!("{remote}/")
                        };
                        let mut lister = op
                            .lister_with(&scan)
                            .recursive(false)
                            .await
                            .map_err(opendal_to_object_store_error)?;
                        let mut objects = Vec::new();
                        let mut common_prefixes = Vec::new();
                        while let Some(entry) = lister.next().await {
                            let entry = entry.map_err(opendal_to_object_store_error)?;
                            let entry_path = entry.path().to_string();
                            let entry_ds = Path::parse(&entry_path).unwrap_or(Path::ROOT);
                            if entry.metadata().is_file() {
                                let mut meta = entry_to_meta(&entry).unwrap_or_else(|| {
                                    opendal_meta_to_object_meta(&entry_path, entry.metadata())
                                });
                                meta.location = Self::remap_to_virtual(&entry_ds, &handle);
                                objects.push(meta);
                            } else if entry.metadata().is_dir() {
                                common_prefixes.push(Self::remap_to_virtual(&entry_ds, &handle));
                            }
                        }
                        Ok(ListResult {
                            common_prefixes,
                            objects,
                        })
                    });
                }
            }
        }

        // Case 2: composite listing — covering root mount (or default
        // fs) + direct child mounts merged into one response.
        let root_mount = self.mounts.as_ref().and_then(|m| {
            let handle = m.handle_for(&Path::ROOT)?;
            (handle.definition.path == "/").then_some(handle)
        });
        let op = root_mount
            .as_ref()
            .map(|h| (*h.backend_op).clone())
            .unwrap_or_else(|| self.op.clone());
        let base_key = root_mount
            .as_ref()
            .map(|h| mount_key(h, &effective))
            .unwrap_or_else(|| effective.to_string());
        let scan_path = if base_key.is_empty() || base_key.ends_with('/') {
            base_key.clone()
        } else {
            format!("{base_key}/")
        };
        let mounts_snapshot: Vec<(String, std::sync::Arc<opendal::Operator>)> = self
            .mounts
            .as_ref()
            .map(|m| {
                m.mount_paths()
                    .into_iter()
                    .filter_map(|p| {
                        let ds = Path::parse(&p).ok()?;
                        // Strict direct-child mount only.
                        let parent = ds.parent().unwrap_or(Path::ROOT);
                        if parent != effective {
                            return None;
                        }
                        m.handle_for(&ds).map(|h| (p, h.backend_op))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let stream_root_mount = root_mount.clone();
        Box::pin(async move {
            // A root-mounted virtual path can itself be a file. OpenDAL's
            // directory lister returns an empty stream for `file/`, which
            // DataFusion otherwise treats as a valid zero-column table.
            if let Some(handle) = stream_root_mount.as_ref()
                && effective != Path::ROOT
                && let Ok(meta) = op.stat(&base_key).await
                && meta.is_file()
            {
                let mut object = opendal_meta_to_object_meta(&base_key, &meta);
                object.location =
                    Self::remap_to_virtual(&Path::parse(&base_key).unwrap_or_default(), handle);
                return Ok(ListResult {
                    common_prefixes: Vec::new(),
                    objects: vec![object],
                });
            }

            let mut lister = op
                .lister_with(&scan_path)
                .recursive(false)
                .await
                .map_err(opendal_to_object_store_error)?;

            let mut objects = Vec::new();
            let mut common_prefixes = Vec::new();

            // Track which base-fs prefixes are shadowed by a mount so
            // we don't emit them twice. Paths are compared in virtual
            // form because a root mount may have a backend source prefix.
            let shadowed: std::collections::HashSet<String> = mounts_snapshot
                .iter()
                .map(|(mp, _)| mp.trim_end_matches('/').to_string())
                .collect();

            while let Some(entry) = lister.next().await {
                let entry = entry.map_err(opendal_to_object_store_error)?;
                let entry_path = entry.path().to_string();
                let entry_ds = Path::parse(&entry_path).unwrap_or(Path::ROOT);
                let virtual_entry = stream_root_mount
                    .as_ref()
                    .map(|h| Self::remap_to_virtual(&entry_ds, h))
                    .unwrap_or(entry_ds);
                let trimmed = virtual_entry.as_ref().trim_end_matches('/').to_string();
                if entry.metadata().is_file() {
                    if let Some(mut meta) = entry_to_meta(&entry) {
                        meta.location = virtual_entry;
                        objects.push(meta);
                    }
                } else if entry.metadata().is_dir() {
                    if shadowed.contains(&trimmed) {
                        continue;
                    }
                    common_prefixes.push(virtual_entry);
                }
            }

            // Inject each direct-child mount as a synthetic directory
            // entry.
            for (vp, _) in &mounts_snapshot {
                if let Ok(p) = Path::parse(vp) {
                    common_prefixes.push(p);
                }
            }

            Ok(ListResult {
                common_prefixes,
                objects,
            })
        })
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path, ObjectStoreError>>,
    ) -> BoxStream<'static, Result<Path, ObjectStoreError>> {
        let this_op = self.op.clone();
        let this_mounts = self.mounts.clone();
        let path_locks = self.path_locks.clone();
        locations
            .map(move |location| {
                let op = this_op.clone();
                let mounts = this_mounts.clone();
                let path_locks = path_locks.clone();
                async move {
                    let location = location?;
                    if let Some(mounts) = mounts {
                        if let Some(handle) = mounts.handle_for(&location) {
                            if handle.read_only {
                                return Err(ObjectStoreError::NotSupported {
                                    source: format!(
                                        "mount '{}' is read-only",
                                        handle.definition.path
                                    )
                                    .into(),
                                });
                            }
                            let final_path = mount_key(&handle, &location);
                            let object_lock = lock_object(&path_locks, &final_path);
                            let _guard = object_lock.write().await;
                            let op = handle.backend_op.as_ref().clone();
                            op.delete(&final_path)
                                .await
                                .map_err(opendal_to_object_store_error)?;
                            return Ok(location);
                        }
                    }
                    let path = location.to_string();
                    let object_lock = lock_object(&path_locks, &path);
                    let _guard = object_lock.write().await;
                    op.delete(&path)
                        .await
                        .map_err(opendal_to_object_store_error)?;
                    Ok(location)
                }
            })
            .buffered(10)
            .boxed()
    }

    fn copy_opts<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        from: &'life1 Path,
        to: &'life2 Path,
        options: CopyOptions,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<(), ObjectStoreError>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        if let Err(e) = self.check_writable_path(to) {
            return Box::pin(async move { Err(e) });
        }
        let from_op = self.dispatch_op(from);
        let from_path = self.dispatch_path(from);
        let to_op = self.dispatch_op(to);
        let to_path = self.dispatch_path(to);
        let staging_path = staging_key(&to_path);
        let destination_lock = self.object_lock(&to_path);
        Box::pin(async move {
            // OpenDAL's `copy` is backend-internal — it cannot span
            // two different Operators. Detect that case and fall back
            // to read+write.
            let same_backend = Arc::ptr_eq(&Arc::new(from_op.clone()), &Arc::new(to_op.clone()));
            let staged = if !same_backend {
                let buf = from_op
                    .read(&from_path)
                    .await
                    .map_err(opendal_to_object_store_error);
                match buf {
                    Ok(buf) => to_op
                        .write(&staging_path, buf)
                        .await
                        .map(|_| ())
                        .map_err(opendal_to_object_store_error),
                    Err(err) => Err(err),
                }
            } else {
                match options.mode {
                    CopyMode::Overwrite => {
                        from_op
                            .copy(&from_path, &staging_path)
                            .await
                            .map_err(opendal_to_object_store_error)?;
                        Ok(())
                    }
                    CopyMode::Create => {
                        from_op
                            .copy_with(&from_path, &staging_path)
                            .if_not_exists(true)
                            .await
                            .map_err(opendal_to_object_store_error)?;
                        Ok(())
                    }
                }
            };

            if let Err(err) = staged {
                let _ = to_op.delete(&staging_path).await;
                return Err(err);
            }

            let _guard = destination_lock.write().await;
            if matches!(options.mode, CopyMode::Create) && to_op.stat(&to_path).await.is_ok() {
                let _ = to_op.delete(&staging_path).await;
                return Err(ObjectStoreError::AlreadyExists {
                    path: to_path.clone(),
                    source: "destination already exists".into(),
                });
            }

            if let Err(err) = to_op.rename(&staging_path, &to_path).await {
                let _ = to_op.delete(&staging_path).await;
                return Err(opendal_to_object_store_error(err));
            }
            Ok(())
        })
    }
}

/// Adapter that bridges `object_store::MultipartUpload` onto OpenDAL's
/// chunked `Writer` API.
///
/// DataFusion's single-file sink always writes through `WriteMultipart`,
/// which calls `put_part` for each 5 MiB chunk and then `complete`. OpenDAL's
/// `Writer` expects sequential `write` calls and a final `close`.
///
/// `put_part` returns a `'static` future and callers may poll those futures
/// concurrently. A mutex alone would make writes mutually exclusive but would
/// not guarantee their order. `put_part` assigns an index synchronously in API
/// invocation order; its future queues that indexed buffer through a bounded
/// channel. A single writer task buffers out-of-order arrivals in a `BTreeMap`
/// and flushes only the next expected index, making the final byte stream
/// deterministic even when callers poll part futures concurrently.
struct OpendalMultipartUpload {
    part_tx: Option<tokio::sync::mpsc::Sender<(u64, Vec<u8>)>>,
    next_part: u64,
    writer_task: Option<tokio::task::JoinHandle<Result<(), opendal::Error>>>,
    op: Operator,
    path: String,
    staging_path: String,
    destination_lock: Arc<tokio::sync::RwLock<()>>,
}

impl std::fmt::Debug for OpendalMultipartUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpendalMultipartUpload")
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl MultipartUpload for OpendalMultipartUpload {
    fn put_part(&mut self, data: PutPayload) -> UploadPart {
        let Some(part_tx) = self.part_tx.clone() else {
            return Box::pin(async move {
                Err(ObjectStoreError::Generic {
                    store: "opendal",
                    source: "multipart upload already completed or aborted".into(),
                })
            });
        };
        let part_index = self.next_part;
        self.next_part += 1;
        Box::pin(async move {
            let mut buf = Vec::with_capacity(data.content_length());
            for chunk in data.iter() {
                buf.extend_from_slice(chunk);
            }
            part_tx
                .send((part_index, buf))
                .await
                .map_err(|_| ObjectStoreError::Generic {
                    store: "opendal",
                    source: "ordered multipart writer exited before all parts were queued".into(),
                })?;
            Ok(())
        })
    }

    async fn complete(&mut self) -> Result<PutResult, ObjectStoreError> {
        drop(self.part_tx.take());
        let writer_task = self
            .writer_task
            .take()
            .expect("multipart upload completed twice");
        let write_result = writer_task.await?;
        write_result.map_err(opendal_to_object_store_error)?;

        let _guard = self.destination_lock.write().await;
        if let Err(err) = self.op.rename(&self.staging_path, &self.path).await {
            let _ = self.op.delete(&self.staging_path).await;
            return Err(opendal_to_object_store_error(err));
        }

        let e_tag = self
            .op
            .stat(&self.path)
            .await
            .ok()
            .and_then(|meta| meta.etag().map(String::from));
        Ok(PutResult {
            e_tag,
            version: None,
        })
    }

    async fn abort(&mut self) -> Result<(), ObjectStoreError> {
        drop(self.part_tx.take());
        if let Some(writer_task) = self.writer_task.take() {
            writer_task.abort();
            let _ = writer_task.await;
        }
        let _ = self.op.delete(&self.staging_path).await;
        Ok(())
    }
}

/// Build a unique sibling key. Renames must occur within one backend, so the
/// staging object must share the destination's backend-local parent directory.
fn staging_key(path: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = STAGING_OBJECT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let unique = format!("{}-{}-{}", std::process::id(), nanos, sequence);

    let file_name = path.rsplit('/').next().filter(|name| !name.is_empty());
    let Some(file_name) = file_name else {
        return format!(".root-object-{unique}.tmp");
    };
    match path.rfind('/') {
        Some(index) if index > 0 => format!("{}.{file_name}{unique}.tmp", &path[..=index]),
        _ => format!(".{file_name}{unique}.tmp"),
    }
}

/// Compute the backend-local key for a virtual `path` within a mount:
/// the mount's precomputed backend-relative `source_prefix` followed by
/// the virtual suffix. Mirrors [`MountedObjectStore::resolve`] (the
/// DataFusion path) so callers that dispatch to `handle.backend_op`
/// (the agent-facing vfs ops) and callers that go through the
/// `ObjectStore` trait see identical locations.
fn mount_key(handle: &MountHandle, path: &Path) -> String {
    let vp = Path::parse(&handle.definition.path).unwrap_or(Path::ROOT);
    let mut remote = String::new();
    if let Some(suffix_iter) = path.prefix_match(&vp) {
        for part in suffix_iter {
            if !remote.is_empty() {
                remote.push('/');
            }
            remote.push_str(part.as_ref());
        }
    }
    let source_key = handle.source_prefix.as_ref().trim_matches('/');
    match (source_key.is_empty(), remote.is_empty()) {
        (true, true) => String::new(),
        (true, false) => remote,
        (false, true) => source_key.to_string(),
        (false, false) => format!("{source_key}/{remote}"),
    }
}

/// Convert an opendal error into an object_store error.
fn opendal_to_object_store_error(err: opendal::Error) -> ObjectStoreError {
    let msg = err.message().to_string();
    match err.kind() {
        opendal::ErrorKind::NotFound => ObjectStoreError::NotFound {
            path: msg,
            source: err.into(),
        },
        opendal::ErrorKind::AlreadyExists => ObjectStoreError::AlreadyExists {
            path: msg,
            source: err.into(),
        },
        opendal::ErrorKind::PermissionDenied => {
            ObjectStoreError::NotSupported { source: err.into() }
        }
        _ => ObjectStoreError::Generic {
            store: "opendal",
            source: err.into(),
        },
    }
}

fn opendal_meta_to_object_meta(path: &str, meta: &opendal::Metadata) -> ObjectMeta {
    let last_modified = meta
        .last_modified()
        .map(|ts| {
            let sys_time: SystemTime = ts.into();
            Utc.timestamp_opt(
                sys_time
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0) as i64,
                0,
            )
            .single()
            .unwrap_or_else(Utc::now)
        })
        .unwrap_or_else(Utc::now);
    ObjectMeta {
        location: Path::parse(path).unwrap_or_else(|_| Path::from(path)),
        size: meta.content_length(),
        last_modified,
        e_tag: meta.etag().map(String::from),
        version: None,
    }
}

fn entry_to_meta(entry: &opendal::Entry) -> Option<ObjectMeta> {
    let meta = entry.metadata();
    if !meta.is_file() {
        return None;
    }
    opendal_meta_to_object_meta(entry.path(), meta).into()
}

#[cfg(test)]
mod tests {
    use super::OpendalFileStorage;

    #[test]
    fn normalize_absolute_path() {
        assert_eq!(OpendalFileStorage::normalize_path("/foo"), "/foo");
    }

    #[test]
    fn normalize_relative_path() {
        assert_eq!(OpendalFileStorage::normalize_path("foo"), "/foo");
    }

    #[test]
    fn normalize_dot_slash_path() {
        assert_eq!(OpendalFileStorage::normalize_path("./foo"), "/foo");
        assert_eq!(OpendalFileStorage::normalize_path("./foo/bar"), "/foo/bar");
    }

    #[test]
    fn normalize_root() {
        assert_eq!(OpendalFileStorage::normalize_path("/"), "/");
        assert_eq!(OpendalFileStorage::normalize_path(""), "/");
        assert_eq!(OpendalFileStorage::normalize_path("."), "/");
        assert_eq!(OpendalFileStorage::normalize_path("./"), "/");
    }

    #[test]
    fn normalize_slashes_only() {
        assert_eq!(OpendalFileStorage::normalize_path("///"), "/");
    }

    #[test]
    fn normalize_leading_slash_dot_slash() {
        assert_eq!(OpendalFileStorage::normalize_path("/./foo"), "/foo");
    }

    // ── `..` lexical resolution with root clamping ──

    #[test]
    fn normalize_dotdot_simple() {
        assert_eq!(OpendalFileStorage::normalize_path("/foo/../bar"), "/bar");
    }

    #[test]
    fn normalize_dotdot_escape_clamped() {
        // `..` from root is ignored — path stays within virtual root
        assert_eq!(
            OpendalFileStorage::normalize_path("/../etc/passwd"),
            "/etc/passwd"
        );
    }

    #[test]
    fn normalize_dotdot_multi_escape_clamped() {
        assert_eq!(OpendalFileStorage::normalize_path("a/../../../b"), "/b");
    }

    #[test]
    fn normalize_dotdot_mixed() {
        assert_eq!(
            OpendalFileStorage::normalize_path("/foo/./bar/../baz"),
            "/foo/baz"
        );
    }

    #[test]
    fn normalize_dotdot_only() {
        assert_eq!(OpendalFileStorage::normalize_path(".."), "/");
        assert_eq!(OpendalFileStorage::normalize_path("../.."), "/");
        assert_eq!(OpendalFileStorage::normalize_path("/../../.."), "/");
    }

    // ── multipart upload ──

    #[tokio::test]
    async fn test_multipart_upload_roundtrip() {
        use datafusion::object_store::{ObjectStoreExt, PutPayload};

        let storage = OpendalFileStorage::new_temp();
        let path = datafusion::object_store::path::Path::from("big_file.bin");

        // Write 6 MiB — larger than the 5 MiB default chunk size, forcing
        // multiple `put_part` calls if used via WriteMultipart.
        let part_data = vec![0xABu8; 6 * 1024 * 1024];
        let payload = PutPayload::from_bytes(part_data.clone().into());

        storage.put(&path, payload).await.unwrap();

        let got = storage.get(&path).await.unwrap().bytes().await.unwrap();
        assert_eq!(got.len(), 6 * 1024 * 1024);
        assert!(got.iter().all(|&b| b == 0xAB));
    }

    #[tokio::test]
    async fn get_returns_a_chunked_stream() {
        use datafusion::object_store::{ObjectStoreExt, PutPayload};
        use futures::TryStreamExt;

        let storage = OpendalFileStorage::new_temp();
        let path = datafusion::object_store::path::Path::from("chunked_read.bin");
        let len = 6 * 1024 * 1024;
        storage
            .put(&path, PutPayload::from_bytes(vec![0xCDu8; len].into()))
            .await
            .unwrap();

        let chunks = storage
            .get(&path)
            .await
            .unwrap()
            .into_stream()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert!(chunks.len() > 1, "expected chunked object-store reads");
        assert_eq!(chunks.iter().map(|chunk| chunk.len()).sum::<usize>(), len);
        assert!(chunks.iter().all(|chunk| chunk.iter().all(|&b| b == 0xCD)));
    }

    #[tokio::test]
    async fn test_multipart_upload_multiple_parts() {
        use datafusion::object_store::{ObjectStoreExt, WriteMultipart};

        let storage = OpendalFileStorage::new_temp();
        let path = datafusion::object_store::path::Path::from("multipart_test.bin");

        // Use WriteMultipart exactly like DataFusion's single-file sink does.
        let upload = storage.put_multipart(&path).await.unwrap();
        let mut writer = WriteMultipart::new_with_chunk_size(upload, 1024);

        // Write 50 KiB in a pattern we can verify → ~50 parts of ~1 KiB each.
        let data: Vec<u8> = (0..50 * 1024).map(|i| (i % 256) as u8).collect();
        writer.write(&data);
        writer.finish().await.unwrap();

        let got = storage.get(&path).await.unwrap().bytes().await.unwrap();
        assert_eq!(got.as_ref(), data.as_slice());
    }

    #[tokio::test]
    async fn test_multipart_upload_large_object_roundtrip() {
        use datafusion::object_store::{ObjectStoreExt, WriteMultipart};

        let storage = OpendalFileStorage::new_temp();
        let path = datafusion::object_store::path::Path::from("large_multipart.bin");
        let upload = storage.put_multipart(&path).await.unwrap();
        let mut writer = WriteMultipart::new_with_chunk_size(upload, 8 * 1024 * 1024);

        let chunk_len = 1024 * 1024;
        let mut expected_len = 0usize;
        let mut expected_sum = 0u64;
        for part in 0..64u8 {
            let chunk = vec![part; chunk_len];
            writer.write(&chunk);
            expected_sum = expected_sum.wrapping_add(chunk.iter().map(|b| *b as u64).sum());
            expected_len += chunk.len();
        }
        writer.finish().await.unwrap();

        let got = storage.get(&path).await.unwrap().bytes().await.unwrap();
        assert_eq!(got.len(), expected_len);
        for (part, chunk) in got.chunks(chunk_len).enumerate() {
            assert!(
                chunk.iter().all(|byte| *byte == part as u8),
                "multipart part {part} was written out of order"
            );
        }
        let actual_checksum = got.iter().map(|b| *b as u64).sum::<u64>();
        assert_eq!(actual_checksum, expected_sum);
    }

    #[tokio::test]
    async fn concurrent_multipart_replacement_never_exposes_partial_reads() {
        use datafusion::object_store::{ObjectStoreExt, PutPayload, WriteMultipart};
        use std::sync::Arc;

        let storage = Arc::new(OpendalFileStorage::new_temp());
        let path = datafusion::object_store::path::Path::from("shared.csv");
        let initial = vec![0u8; 64 * 1024];
        storage
            .put(&path, PutPayload::from_bytes(initial.into()))
            .await
            .unwrap();

        let writer = {
            let storage = storage.clone();
            let path = path.clone();
            async move {
                let mut jobs = Vec::new();
                for marker in 1u8..=8 {
                    let storage = storage.clone();
                    let path = path.clone();
                    jobs.push(async move {
                        let upload = storage.put_multipart(&path).await.unwrap();
                        let mut writer = WriteMultipart::new_with_chunk_size(upload, 64 * 1024);
                        for _ in 0..4 {
                            writer.write(&vec![marker; 128 * 1024]);
                        }
                        writer.finish().await.unwrap();
                    });
                }
                futures::future::join_all(jobs).await;
            }
        };

        let reader = {
            let storage = storage.clone();
            let path = path.clone();
            async move {
                for _ in 0..128 {
                    let result = storage.get(&path).await.unwrap();
                    let bytes = result.bytes().await.unwrap();
                    assert!(
                        bytes.len() == 64 * 1024 || bytes.len() == 512 * 1024,
                        "reader observed non-atomic object size {}",
                        bytes.len()
                    );
                    let first = bytes[0];
                    assert!(
                        bytes.iter().all(|byte| *byte == first),
                        "reader observed mixed contents from concurrent writers"
                    );
                }
            }
        };

        tokio::join!(writer, reader).1;
    }

    #[tokio::test]
    async fn multipart_reorders_concurrently_polled_parts_into_api_order() {
        use datafusion::object_store::{MultipartUpload, ObjectStoreExt};

        let storage = OpendalFileStorage::new_temp();
        let path = datafusion::object_store::path::Path::from("ordered.bin");
        let mut upload = storage.put_multipart(&path).await.unwrap();

        let part1 = upload.put_part(vec![1u8; 1024].into());
        let part2 = upload.put_part(vec![2u8; 1024].into());
        let part3 = upload.put_part(vec![3u8; 1024].into());
        part3.await.unwrap();
        part1.await.unwrap();
        part2.await.unwrap();
        upload.complete().await.unwrap();

        let got = storage.get(&path).await.unwrap().bytes().await.unwrap();
        assert_eq!(got.len(), 3 * 1024);
        assert!(got[..1024].iter().all(|byte| *byte == 1));
        assert!(got[1024..2048].iter().all(|byte| *byte == 2));
        assert!(got[2048..].iter().all(|byte| *byte == 3));
    }

    // ── VFS mount routing ──────────────────────────────────────────

    use crate::{BackendConfig, BackendDefinition, MountDefinition, VfsManifest};
    use datafusion::object_store::ObjectStore;
    use std::sync::Arc;

    /// Helper: build a storage backed by a temp data_dir + a manifest
    /// with a single read-only mount at `/data` pointing at a temp
    /// source directory containing pre-populated files. The TempDir
    /// guards must be kept alive by the caller (we return them in the
    /// tuple) — drop order matters: drop the guards last.
    struct MountedStorageHarness {
        storage: Arc<OpendalFileStorage>,
        data_dir: tempfile::TempDir,
        source_dir: tempfile::TempDir,
    }

    fn make_mounted_storage(files: &[(&str, &[u8])]) -> MountedStorageHarness {
        let data_dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let full = source_dir.path().join(rel.trim_start_matches('/'));
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, content).unwrap();
        }
        let manifest = VfsManifest {
            backend: vec![
                BackendDefinition {
                    id: "default".into(),
                    config: BackendConfig::local(data_dir.path().to_string_lossy().to_string()),
                },
                BackendDefinition {
                    id: "source".into(),
                    config: BackendConfig::local(source_dir.path().to_string_lossy().to_string()),
                },
            ],
            mount: vec![
                MountDefinition {
                    path: "/".into(),
                    backend: "default".into(),
                    source: "/".into(),
                    read_only: false,
                },
                MountDefinition {
                    path: "/data".into(),
                    backend: "source".into(),
                    source: "/".into(),
                    read_only: true,
                },
            ],
        };
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        // The default backend is rooted at `data_dir` so that the
        // unwrapped fallback (and downloads_op) land on the data_dir.
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            data_dir.path(),
            vfs.clone(),
        ));
        MountedStorageHarness {
            storage,
            data_dir,
            source_dir,
        }
    }

    #[tokio::test]
    async fn mount_list_returns_every_mount_definition() {
        let MountedStorageHarness {
            storage,
            data_dir: _data_dir,
            source_dir: _src,
        } = make_mounted_storage(&[]);
        let mounts = storage.mount_definitions();
        let paths: Vec<&str> = mounts.iter().map(|m| m.path.as_str()).collect();
        assert!(paths.contains(&"/"), "root mount should be present");
        assert!(paths.contains(&"/data"), "/data mount should be present");
    }

    #[tokio::test]
    async fn read_through_mount_routes_to_source_backend() {
        let MountedStorageHarness {
            storage,
            data_dir: _data_dir,
            source_dir: _src,
        } = make_mounted_storage(&[("panel.parquet", b"ldsc-data")]);
        let op = storage.resolve("/data/panel.parquet");
        let remote = storage.resolve_path("/data/panel.parquet");
        let buf = op.read(&remote).await.unwrap();
        assert_eq!(buf.to_vec(), b"ldsc-data");
    }

    #[tokio::test]
    async fn read_outside_mount_uses_default_backend() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("scratch.txt"), b"hello").unwrap();
        let manifest = VfsManifest::local_root(data.path().to_string_lossy().to_string());
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(data.path(), vfs.clone()));
        let remote = storage.resolve_path("/scratch.txt");
        let buf = storage.resolve("/scratch.txt").read(&remote).await.unwrap();
        assert_eq!(buf.to_vec(), b"hello");
    }

    #[tokio::test]
    async fn write_to_read_only_mount_is_rejected() {
        let MountedStorageHarness {
            storage,
            data_dir: _data_dir,
            source_dir: _src,
        } = make_mounted_storage(&[("panel.parquet", b"ldsc-data")]);
        let err = storage.check_writable("/data/panel.parquet").unwrap_err();
        assert!(matches!(err.kind(), opendal::ErrorKind::PermissionDenied));
    }

    #[tokio::test]
    async fn write_to_default_backend_is_allowed() {
        let data = tempfile::tempdir().unwrap();
        let manifest = VfsManifest::local_root(data.path().to_string_lossy().to_string());
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(data.path(), vfs.clone()));
        let remote = storage.resolve_path("/scratch.txt");
        storage
            .resolve("/scratch.txt")
            .write(&remote, b"hello".to_vec())
            .await
            .unwrap();
        let buf = storage.resolve("/scratch.txt").read(&remote).await.unwrap();
        assert_eq!(buf.to_vec(), b"hello");
    }

    #[tokio::test]
    async fn same_backend_detects_shared_mount() {
        let MountedStorageHarness {
            storage,
            data_dir: _data_dir,
            source_dir: _src,
        } = make_mounted_storage(&[]);
        assert!(storage.same_backend("/data/foo", "/data/bar"));
        assert!(!storage.same_backend("/data/foo", "/scratch.txt"));
        assert!(storage.same_backend("/scratch.txt", "/other"));
    }

    #[tokio::test]
    async fn list_with_delimiter_merges_mount_points_into_default_fs() {
        let MountedStorageHarness {
            storage,
            data_dir,
            source_dir: _src,
        } = make_mounted_storage(&[("panel.parquet", b"x")]);
        // Create a file in the default fs that should appear alongside
        // the mount's synthetic entry.
        std::fs::write(data_dir.path().join("scratch.txt"), b"hi").unwrap();
        let result = (*storage)
            .list_with_delimiter(Some(&datafusion::object_store::path::Path::ROOT))
            .await
            .unwrap();
        // /data should be present as a common_prefix (synthetic mount).
        let prefixes: Vec<&str> = result.common_prefixes.iter().map(|p| p.as_ref()).collect();
        assert!(
            prefixes.iter().any(|p| p.trim_start_matches('/') == "data"),
            "expected `/data` as a synthetic prefix, got: {prefixes:?}"
        );
    }

    #[tokio::test]
    async fn downloads_op_bypasses_mounts() {
        let MountedStorageHarness {
            storage,
            data_dir,
            source_dir: src,
        } = make_mounted_storage(&[]);
        let remote = storage.resolve_path("/scratch.txt");
        storage
            .downloads_op()
            .write(&remote, b"downloads".to_vec())
            .await
            .unwrap();
        let on_disk = std::fs::read(data_dir.path().join("scratch.txt")).unwrap();
        assert_eq!(on_disk, b"downloads");
        assert!(std::fs::read_dir(src).unwrap().next().is_none());
    }

    // ── mount with a non-empty `source` (mirrors real vfs.toml) ──

    /// Build a storage whose root mount maps `/` to a subdirectory
    /// (`<backend_root>/ws`) of the default backend, exactly like a
    /// vfs.toml with `backend.root = "/"` + `mount.source = "/mnt/..."`.
    /// The returned tuple keeps the temp dirs alive.
    struct SourcedMountHarness {
        storage: Arc<OpendalFileStorage>,
        backend_root: tempfile::TempDir,
        source_dir: std::path::PathBuf,
    }

    fn make_sourced_mount() -> SourcedMountHarness {
        let backend_root = tempfile::tempdir().unwrap();
        let source_dir = backend_root.path().join("ws");
        std::fs::create_dir_all(&source_dir).unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: BackendConfig::local(backend_root.path().to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/".into(),
                backend: "default".into(),
                source: source_dir.to_string_lossy().to_string(),
                read_only: false,
            }],
        };
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            vfs.clone(),
        ));
        SourcedMountHarness {
            storage,
            backend_root,
            source_dir,
        }
    }

    #[tokio::test]
    async fn resolve_path_with_nonempty_source_uses_backend_relative_key() {
        let SourcedMountHarness {
            storage,
            source_dir: _source_dir,
            backend_root: _backend_root,
        } = make_sourced_mount();
        assert_eq!(storage.resolve_path("/hello.txt"), "ws/hello.txt");
        assert_eq!(storage.resolve_path("/a/b/c.txt"), "ws/a/b/c.txt");
    }

    #[tokio::test]
    async fn write_through_mount_with_nonempty_source_lands_in_source_dir() {
        let SourcedMountHarness {
            storage,
            source_dir,
            backend_root: _backend_root,
        } = make_sourced_mount();
        let remote = storage.resolve_path("/hello.txt");
        storage
            .resolve("/hello.txt")
            .write(&remote, b"hi".to_vec())
            .await
            .unwrap();
        let on_disk = std::fs::read(source_dir.join("hello.txt")).unwrap();
        assert_eq!(on_disk, b"hi");
    }

    #[tokio::test]
    async fn remap_entry_strips_source_prefix_and_reattaches_virtual() {
        let SourcedMountHarness {
            storage,
            source_dir,
            backend_root: _backend_root,
        } = make_sourced_mount();
        std::fs::write(source_dir.join("panel.parquet"), b"x").unwrap();
        std::fs::create_dir_all(source_dir.join("sub")).unwrap();
        std::fs::write(source_dir.join("sub/nested.txt"), b"y").unwrap();

        assert_eq!(
            storage.remap_entry_to_virtual("/", "ws/panel.parquet"),
            "/panel.parquet"
        );
        assert_eq!(
            storage.remap_entry_to_virtual("/", "ws/sub/nested.txt"),
            "/sub/nested.txt"
        );
    }

    #[tokio::test]
    async fn resolve_path_matches_real_vfs_toml_layout() {
        // Mirrors the operator's actual ~/.autonomics/vfs.toml: local
        // backends rooted at `/` with absolute `source` paths. The
        // backend key must be the source path relative to `/`, so the
        // Fs operator (rooted at `/`) resolves it to the exact source.
        let manifest = VfsManifest::from_toml(
            r#"
            [[backend]]
            id = "default"
            type = "local"
            root = "/"

            [[mount]]
            path = "/"
            backend = "default"
            source = "/mnt/disk3/test"
            read_only = false

            [[backend]]
            id = "ldsc-local"
            type = "local"
            root = "/"

            [[mount]]
            path = "/data/ldsc"
            backend = "ldsc-local"
            source = "/mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet"
            read_only = true
            "#,
        )
        .unwrap();
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            vfs,
        ));

        assert_eq!(
            storage.resolve_path("/hello.txt"),
            "mnt/disk3/test/hello.txt"
        );
        assert_eq!(
            storage.resolve_path("/data/ldsc/1000g_eur.parquet"),
            "mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet/1000g_eur.parquet"
        );
        // Read-only mount is rejected for writes.
        assert!(matches!(
            storage
                .check_writable("/data/ldsc/1000g_eur.parquet")
                .unwrap_err()
                .kind(),
            opendal::ErrorKind::PermissionDenied
        ));
    }

    #[tokio::test]
    async fn list_root_mount_uses_mount_source_not_default_data_dir() {
        let SourcedMountHarness {
            storage,
            source_dir,
            backend_root: _backend_root,
        } = make_sourced_mount();
        std::fs::write(source_dir.join("hello.txt"), b"mounted").unwrap();

        let result = (*storage)
            .list_with_delimiter(Some(&datafusion::object_store::path::Path::ROOT))
            .await
            .unwrap();
        let objects: Vec<&str> = result.objects.iter().map(|m| m.location.as_ref()).collect();
        assert!(
            objects
                .iter()
                .any(|p| p.trim_start_matches('/') == "hello.txt"),
            "expected /hello.txt from mount source, got: {objects:?}"
        );
    }

    #[tokio::test]
    async fn list_non_root_mount_root_returns_its_objects() {
        // `/data` mount (source "/", backend rooted at source_dir) must
        // list its own contents — not fall through to the default fs.
        let MountedStorageHarness {
            storage,
            data_dir: _d,
            source_dir: _s,
        } = make_mounted_storage(&[("panel.parquet", b"ldsc-data")]);
        let result = (*storage)
            .list_with_delimiter(Some(
                &datafusion::object_store::path::Path::parse("/data").unwrap(),
            ))
            .await
            .unwrap();
        let objects: Vec<&str> = result.objects.iter().map(|m| m.location.as_ref()).collect();
        assert!(
            objects
                .iter()
                .any(|p| p.trim_start_matches('/') == "data/panel.parquet"),
            "expected /data/panel.parquet, got: {objects:?}"
        );
    }
}
