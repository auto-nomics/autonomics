use std::fmt::{Debug, Display};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use chrono::{TimeZone, Utc};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::object_store::Error as ObjectStoreError;
use datafusion::object_store::{
    Attributes, CopyMode, CopyOptions, GetOptions, GetRange, GetResult, GetResultPayload,
    ListResult, MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions,
    PutPayload, PutResult, path::Path,
};
use datafusion::prelude::SessionContext;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use opendal::Operator;
use opendal::services::Fs;
use tempfile::TempDir;

pub struct OpendalFileStorage {
    pub op: Operator,
    root: PathBuf,
    /// Keeps the temp directory alive until this storage is dropped.
    _temp_guard: Option<TempDir>,
}

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
                "" | "." => {}                // skip empty and current-dir
                ".." => {
                    stack.pop();              // lexical parent; clamps at root
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

    /// Create a new storage backed by the local filesystem at the given root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let op = Operator::new(Fs::default().root(root.to_str().unwrap_or("/tmp/opendal")))
            .unwrap()
            .finish();
        Self {
            op,
            root,
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
            root,
            _temp_guard: Some(tmp),
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

    /// Convert an ObjectStore path to a local filesystem path.
    fn to_local_path(&self, location: &Path) -> PathBuf {
        let path_str = location.to_string();
        let relative = path_str.strip_prefix('/').unwrap_or(&path_str);
        self.root.join(relative)
    }
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
        let path = location.to_string();
        let op = self.op.clone();
        Box::pin(async move {
            let total_len = payload.content_length();
            let mut buf = Vec::with_capacity(total_len);
            for chunk in payload.iter() {
                buf.extend_from_slice(chunk);
            }
            let buffer = opendal::Buffer::from(buf);
            op.write(&path, buffer)
                .await
                .map_err(opendal_to_object_store_error)?;
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
        _location: &'life1 Path,
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
        Box::pin(async {
            Err(ObjectStoreError::NotSupported {
                source: "multipart upload is not supported".into(),
            })
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
        let local_path = self.to_local_path(location);
        let op = self.op.clone();
        let path = location.to_string();
        Box::pin(async move {
            let meta = op
                .stat(&path)
                .await
                .map_err(opendal_to_object_store_error)?;
            let object_meta = opendal_meta_to_object_meta(&path, &meta);
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

            Ok(GetResult {
                payload: GetResultPayload::File(
                    std::fs::File::open(&local_path).map_err(|e| ObjectStoreError::NotFound {
                        path: local_path.to_string_lossy().into_owned(),
                        source: e.into(),
                    })?,
                    local_path,
                ),
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
        let scan_path = prefix.map(|p| p.to_string()).unwrap_or_default();
        let op = self.op.clone();

        let stream = async_stream::stream! {
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
                        if e.metadata().is_file() {
                            if let Some(meta) = entry_to_meta(&e) {
                                yield Ok(meta);
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(opendal_to_object_store_error(e));
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
        let scan_path = prefix.map(|p| p.to_string()).unwrap_or_default();
        // opendal Fs requires a trailing '/' to list children of a directory.
        let scan_path = if scan_path.is_empty() || scan_path.ends_with('/') {
            scan_path
        } else {
            format!("{scan_path}/")
        };
        let op = self.op.clone();
        Box::pin(async move {
            let mut lister = op
                .lister_with(&scan_path)
                .recursive(false)
                .await
                .map_err(opendal_to_object_store_error)?;

            let mut objects = Vec::new();
            let mut common_prefixes = Vec::new();

            while let Some(entry) = lister.next().await {
                let entry = entry.map_err(opendal_to_object_store_error)?;
                if entry.metadata().is_file() {
                    if let Some(meta) = entry_to_meta(&entry) {
                        objects.push(meta);
                    }
                } else if entry.metadata().is_dir() {
                    if let Ok(p) = Path::parse(entry.path()) {
                        common_prefixes.push(p);
                    }
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
        let op = self.op.clone();
        locations
            .map(move |location| {
                let op = op.clone();
                async move {
                    let location = location?;
                    let path = location.to_string();
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
        let from_path = from.to_string();
        let to_path = to.to_string();
        let op = self.op.clone();
        Box::pin(async move {
            match options.mode {
                CopyMode::Overwrite => {
                    op.copy(&from_path, &to_path)
                        .await
                        .map_err(opendal_to_object_store_error)?;
                }
                CopyMode::Create => {
                    op.copy_with(&from_path, &to_path)
                        .if_not_exists(true)
                        .await
                        .map_err(opendal_to_object_store_error)?;
                }
            }
            Ok(())
        })
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
        assert_eq!(OpendalFileStorage::normalize_path("/../etc/passwd"), "/etc/passwd");
    }

    #[test]
    fn normalize_dotdot_multi_escape_clamped() {
        assert_eq!(OpendalFileStorage::normalize_path("a/../../../b"), "/b");
    }

    #[test]
    fn normalize_dotdot_mixed() {
        assert_eq!(OpendalFileStorage::normalize_path("/foo/./bar/../baz"), "/foo/baz");
    }

    #[test]
    fn normalize_dotdot_only() {
        assert_eq!(OpendalFileStorage::normalize_path(".."), "/");
        assert_eq!(OpendalFileStorage::normalize_path("../.."), "/");
        assert_eq!(OpendalFileStorage::normalize_path("/../../.."), "/");
    }
}
