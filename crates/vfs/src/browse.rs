//! Typed, transport-neutral browse facade over [`OpendalFileStorage`].
//!
//! The agent-facing vbash tool encodes every failure as a *successful*
//! tool payload (`AgentToolResult::error`), which is the right contract
//! for agents but unusable for transports that need real error
//! classification (HTTP status codes in the gateway, structured errors in
//! the TUI). This module extracts the mount-aware listing and mutation
//! semantics into functions returning typed results, so the vbash
//! adapters (`vbash/ops`) and the gateway `/api/v1/vfs` endpoints share a
//! single implementation and cannot drift on mount behavior:
//!
//! - synthetic child-mount entries and shadowed default-fs directories,
//! - longest-prefix mount dispatch and virtual-path remapping,
//! - pagination with mount points emitted before backend entries,
//! - the `rm` safety contract (refuse `/`, refuse non-empty dirs unless
//!   recursive) and the cross-backend `cp`/`mv` fallback.
//!
//! Permission enforcement stays inside [`OpendalFileStorage`]: every
//! facade function runs `check_readable`/`check_listable`/`check_writable`
//! before touching an operator, so read-only mounts reject mutations
//! regardless of the caller.

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::mount::MountDefinition;
use crate::storage::OpendalFileStorage;

/// Default maximum entries returned by [`list`] when no explicit `limit`
/// is given. Kept modest so a single listing cannot flood a context
/// window; callers may raise it via `limit` or page with `offset`.
pub const DEFAULT_LIST_LIMIT: usize = 200;

/// One entry in a VFS listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VfsEntry {
    /// Full virtual path, for example `/data/ldsc/panel.parquet`.
    pub path: String,
    /// Final path segment, for example `panel.parquet`.
    pub name: String,
    pub is_dir: bool,
    /// Content length in bytes; directories report 0.
    pub size: u64,
    /// Backend-reported last modification time, when available.
    pub last_modified: Option<String>,
}

impl VfsEntry {
    /// Build an entry from a virtual display path plus metadata.
    fn from_metadata(
        display: String,
        is_dir: bool,
        size: u64,
        last_modified: Option<String>,
    ) -> Self {
        let name = display
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        Self { path: display, name, is_dir, size, last_modified }
    }

    /// Synthetic directory entry for a mount point; no backend metadata
    /// exists for these.
    fn synthetic_mount(path: String) -> Self {
        Self::from_metadata(path, true, 0, None)
    }
}

/// A paginated directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VfsListing {
    /// Normalized virtual path that was listed.
    pub path: String,
    pub entries: Vec<VfsEntry>,
    /// Number of entries in this page.
    pub returned: usize,
    /// Whether more entries remain past this page.
    pub truncated: bool,
    /// Continuation offset for the next page; `None` when complete.
    pub next_offset: Option<usize>,
}

/// A text file read, bounded and UTF-8 validated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRead {
    pub path: String,
    pub contents: String,
    /// Full object size in bytes (may exceed `contents.len()` when
    /// truncated at `max_bytes`).
    pub size: u64,
    pub truncated: bool,
}

/// Transport-neutral browse/mutation error.
///
/// Variants carry just the human-readable message; transports map the
/// variant to their own status codes ([`BrowseError::status`] gives the
/// HTTP one) while the vbash adapters embed the message in tool payloads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowseError {
    /// Target path does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The attached principal or mount policy denies the operation.
    #[error("{0}")]
    PermissionDenied(String),
    /// The operation conflicts with existing state (e.g. removing a
    /// non-empty directory without `recursive`).
    #[error("{0}")]
    Conflict(String),
    /// The request is malformed or refuses a dangerous target (e.g. the
    /// virtual root `/`).
    #[error("{0}")]
    Invalid(String),
    /// The backing object-store backend failed.
    #[error("{0}")]
    Backend(String),
}

impl BrowseError {
    /// Message carried by this error.
    pub fn message(&self) -> &str {
        match self {
            BrowseError::NotFound(m)
            | BrowseError::PermissionDenied(m)
            | BrowseError::Conflict(m)
            | BrowseError::Invalid(m)
            | BrowseError::Backend(m) => m,
        }
    }

    /// Suggested HTTP status code for this error class.
    pub fn status(&self) -> u16 {
        match self {
            BrowseError::NotFound(_) => 404,
            BrowseError::PermissionDenied(_) => 403,
            BrowseError::Conflict(_) => 409,
            BrowseError::Invalid(_) => 400,
            BrowseError::Backend(_) => 500,
        }
    }
}

impl From<opendal::Error> for BrowseError {
    fn from(error: opendal::Error) -> Self {
        match error.kind() {
            opendal::ErrorKind::NotFound => Self::NotFound(error.to_string()),
            opendal::ErrorKind::PermissionDenied => Self::PermissionDenied(error.to_string()),
            opendal::ErrorKind::AlreadyExists => Self::Conflict(error.to_string()),
            _ => Self::Backend(error.to_string()),
        }
    }
}

/// Directory authority check shared by listing code: directories need
/// list permission, files need read permission.
pub(crate) fn entry_is_readable(
    storage: &OpendalFileStorage,
    path: &str,
    is_directory: bool,
) -> bool {
    if is_directory {
        storage.check_listable(path).is_ok()
    } else {
        storage.check_readable(path).is_ok()
    }
}

/// Resolve an entry's full metadata, following up with a `stat` when the
/// listing did not carry one (some backends return sparse metadata).
pub(crate) async fn followed_entry_metadata(
    op: &opendal::Operator,
    entry: &opendal::Entry,
) -> Result<opendal::Metadata, String> {
    if entry.metadata().is_dir() || entry.metadata().is_file() {
        return Ok(entry.metadata().clone());
    }
    match op.stat(entry.path()).await {
        Ok(metadata) => Ok(metadata),
        Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(entry.metadata().clone()),
        Err(error) => Err(error.to_string()),
    }
}

/// Direct-child mount points of `base`, filtered to the ones the current
/// principal may list. Mount points are namespace boundaries; listings
/// emit them before backend entries so a truncated page still exposes
/// every mounted branch.
fn child_mount_points(storage: &OpendalFileStorage, base: &str) -> Vec<String> {
    let base = base.trim_end_matches('/');
    let child_mounts: Vec<String> = storage
        .mounts
        .as_ref()
        .map(|m| {
            m.mount_paths()
                .into_iter()
                .filter_map(|mount| {
                    let mount = mount.trim_end_matches('/');
                    if mount == base {
                        return None;
                    }
                    let suffix = if base == "/" {
                        mount.strip_prefix('/')?
                    } else {
                        mount.strip_prefix(base)?.strip_prefix('/')?
                    };
                    let first = suffix.split('/').next()?;
                    if first.is_empty() {
                        return None;
                    }
                    Some(if base == "/" {
                        format!("/{first}")
                    } else {
                        format!("{base}/{first}")
                    })
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    child_mounts
        .into_iter()
        .filter(|mount| storage.check_listable(mount).is_ok())
        .collect()
}

/// The mount covering `vpath`, if the mount table has one.
fn covering_handle(
    storage: &OpendalFileStorage,
    vpath: &str,
) -> Option<crate::mount::MountHandle> {
    use datafusion::object_store::path::Path as DsPath;
    let mounts = storage.mounts.as_ref()?;
    let ds = DsPath::parse(vpath).ok()?;
    mounts.handle_for(&ds)
}

/// List directory entries under `path`.
///
/// Non-recursive listings return the direct children (with synthetic
/// mount-point entries for mounted branches); recursive listings walk the
/// covering backend and then each child mount. Results are paginated
/// (`offset` skipped, up to `limit` collected — default
/// [`DEFAULT_LIST_LIMIT`]).
pub async fn list(
    storage: &OpendalFileStorage,
    path: &str,
    recursive: bool,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<VfsListing, BrowseError> {
    let vpath = OpendalFileStorage::normalize_path(path);
    storage.check_listable(&vpath)?;
    let max = limit.unwrap_or(DEFAULT_LIST_LIMIT).max(1);
    let skip = offset.unwrap_or(0);

    let mut items: Vec<VfsEntry> = Vec::with_capacity(max.min(1024));
    let mut truncated = false;

    let mount = covering_handle(storage, &vpath);
    let child_mounts = child_mount_points(storage, &vpath);

    if recursive {
        // ── Recursive: walk the resolved operator, then each child mount ──
        let op = storage.resolve(&vpath);
        let remote = storage.resolve_path(&vpath);
        let scan = if remote.is_empty() {
            "/".to_string()
        } else if remote.ends_with('/') {
            remote.clone()
        } else {
            format!("{remote}/")
        };
        let mut lister = op.lister_with(&scan).recursive(true).await?;
        let scan_root = remote.trim_end_matches('/').to_string();
        let mut idx = 0usize;
        while let Some(entry) = lister.next().await {
            let entry = entry?;
            let entry_path = entry.path().to_string();
            // Remap the backend-local entry back into the virtual
            // namespace for display and shadow checks.
            let display = storage.remap_entry_to_virtual(&vpath, &entry_path);
            if !entry_is_readable(storage, &display, entry.metadata().is_dir()) {
                continue;
            }
            // Skip the scan-root self-entry.
            if entry.metadata().is_dir() && entry_path.trim_end_matches('/') == scan_root {
                continue;
            }
            // Skip entries shadowed by a child mount.
            let trimmed = display.trim_end_matches('/');
            if child_mounts.iter().any(|m| m.trim_end_matches('/') == trimmed) {
                continue;
            }
            idx += 1;
            if idx <= skip {
                continue;
            }
            if items.len() >= max {
                truncated = true;
                break;
            }
            let meta = entry.metadata();
            let is_dir = meta.is_dir();
            let size = if is_dir { 0 } else { meta.content_length() };
            items.push(VfsEntry::from_metadata(
                display,
                is_dir,
                size,
                meta.last_modified().map(|t| t.to_string()),
            ));
        }

        // If we still have room, append each child mount's contents.
        if !truncated {
            for mp in &child_mounts {
                if let Some(mounts) = &storage.mounts {
                    use datafusion::object_store::path::Path as DsPath;
                    if let Ok(ds) = DsPath::parse(mp) {
                        if let Some(h) = mounts.handle_for(&ds) {
                            let child_op = (*h.backend_op).clone();
                            let child_remote = storage.resolve_path(mp);
                            let child_scan = if child_remote.is_empty() {
                                "/".to_string()
                            } else {
                                child_remote
                            };
                            let mut ml =
                                match child_op.lister_with(&child_scan).recursive(true).await {
                                    Ok(l) => l,
                                    Err(_) => continue,
                                };
                            while let Some(entry) = ml.next().await {
                                if let Ok(entry) = entry {
                                    let meta = entry.metadata();
                                    let entry_path = entry.path().to_string();
                                    let display =
                                        storage.remap_entry_to_virtual(mp, &entry_path);
                                    let is_dir = meta.is_dir();
                                    if !entry_is_readable(storage, &display, is_dir) {
                                        continue;
                                    }
                                    idx += 1;
                                    if idx <= skip {
                                        continue;
                                    }
                                    if items.len() >= max {
                                        truncated = true;
                                        break;
                                    }
                                    let size = if is_dir { 0 } else { meta.content_length() };
                                    items.push(VfsEntry::from_metadata(
                                        display,
                                        is_dir,
                                        size,
                                        meta.last_modified().map(|t| t.to_string()),
                                    ));
                                }
                            }
                            if truncated {
                                break;
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(handle) = mount {
        // ── Non-recursive, covered by a single mount: list inside it ──
        let op = (*handle.backend_op).clone();
        let scan_remote = storage.resolve_path(&vpath);
        let scan_remote_cmp = scan_remote.clone();
        let scan = if scan_remote.ends_with('/') || scan_remote.is_empty() {
            if scan_remote.is_empty() {
                "/".to_string()
            } else {
                scan_remote
            }
        } else {
            format!("{scan_remote}/")
        };
        let mut lister = op.lister_with(&scan).recursive(false).await?;
        let mut idx = 0usize;

        // Mount points are namespace boundaries, not merely entries that
        // happen to exist in the covering backend. Emit them first so a
        // truncated listing still exposes the mounted branch.
        for mp in &child_mounts {
            idx += 1;
            if idx <= skip {
                continue;
            }
            if items.len() >= max {
                truncated = true;
                break;
            }
            items.push(VfsEntry::synthetic_mount(mp.clone()));
        }

        while let Some(entry) = lister.next().await {
            let entry = entry?;
            let entry_path_raw = entry.path().to_string();
            let metadata = followed_entry_metadata(&op, &entry)
                .await
                .map_err(BrowseError::Backend)?;
            if metadata.is_dir()
                && entry_path_raw.trim_end_matches('/') == scan_remote_cmp.trim_end_matches('/')
            {
                continue;
            }
            idx += 1;
            if idx <= skip {
                continue;
            }
            if items.len() >= max {
                truncated = true;
                break;
            }
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.content_length() };
            // Remap to virtual form (strip the mount's backend source
            // prefix, re-attach the virtual prefix).
            let display = storage.remap_entry_to_virtual(&vpath, &entry_path_raw);
            if !entry_is_readable(storage, &display, metadata.is_dir()) {
                continue;
            }
            if metadata.is_dir()
                && child_mounts
                    .iter()
                    .any(|m| m.trim_end_matches('/') == display.trim_end_matches('/'))
            {
                continue;
            }
            items.push(VfsEntry::from_metadata(
                display,
                is_dir,
                size,
                metadata.last_modified().map(|t| t.to_string()),
            ));
        }
    } else {
        // ── Default-fs non-recursive + synthetic direct-child mounts ──
        let op = storage.resolve(&vpath);
        let scan = if vpath.ends_with('/') {
            vpath.clone()
        } else {
            format!("{vpath}/")
        };
        let mut lister = op.lister_with(&scan).recursive(false).await?;
        let scan_root = vpath.trim_end_matches('/').to_string();
        let mut idx = 0usize;

        for mp in &child_mounts {
            idx += 1;
            if idx <= skip {
                continue;
            }
            if items.len() >= max {
                truncated = true;
                break;
            }
            items.push(VfsEntry::synthetic_mount(mp.clone()));
        }

        while let Some(entry) = lister.next().await {
            let entry = entry?;
            let entry_path_raw = entry.path().to_string();
            let trimmed = entry_path_raw.trim_end_matches('/').to_string();
            let metadata = followed_entry_metadata(&op, &entry)
                .await
                .map_err(BrowseError::Backend)?;
            if !entry_is_readable(storage, &entry_path_raw, metadata.is_dir()) {
                continue;
            }
            // Skip default-fs directories that are shadowed by mounts.
            if metadata.is_dir()
                && child_mounts.iter().any(|m| m.trim_end_matches('/') == trimmed)
            {
                continue;
            }
            if metadata.is_dir() && trimmed == scan_root {
                continue;
            }
            idx += 1;
            if idx <= skip {
                continue;
            }
            if items.len() >= max {
                truncated = true;
                break;
            }
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.content_length() };
            items.push(VfsEntry::from_metadata(
                entry_path_raw,
                is_dir,
                size,
                metadata.last_modified().map(|t| t.to_string()),
            ));
        }
    }

    let returned = items.len();
    let next_offset = truncated.then_some(skip + returned);
    Ok(VfsListing {
        path: vpath,
        entries: items,
        returned,
        truncated,
        next_offset,
    })
}

/// File or directory metadata for a single virtual path.
pub async fn stat(storage: &OpendalFileStorage, path: &str) -> Result<VfsEntry, BrowseError> {
    let vpath = OpendalFileStorage::normalize_path(path);
    storage.check_readable(&vpath)?;
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);
    let meta = op
        .stat(&remote)
        .await
        .map_err(|e| match e.kind() {
            opendal::ErrorKind::NotFound => {
                BrowseError::NotFound(format!("File or directory does not exist: {vpath}"))
            }
            _ => BrowseError::from(e),
        })?;
    Ok(VfsEntry::from_metadata(
        vpath,
        meta.is_dir(),
        meta.content_length(),
        meta.last_modified().map(|t| t.to_string()),
    ))
}

/// Snapshot of every mount defined for this VFS.
pub fn mounts(storage: &OpendalFileStorage) -> Vec<MountDefinition> {
    storage.mount_definitions()
}

/// The mount covering `path` (longest prefix), if any. Exposes the
/// read-only flag and declarative policy for UI badges.
pub fn covering_mount(storage: &OpendalFileStorage, path: &str) -> Option<MountDefinition> {
    covering_handle(storage, &OpendalFileStorage::normalize_path(path))
        .map(|handle| handle.definition)
}

/// Read a text file, bounded to `max_bytes`.
///
/// Returns UTF-8 validated contents plus the full object size so callers
/// can surface truncation. Non-UTF-8 content is an
/// [`BrowseError::Invalid`]; use the gateway streaming endpoint for
/// binary files.
pub async fn read_text(
    storage: &OpendalFileStorage,
    path: &str,
    max_bytes: usize,
) -> Result<TextRead, BrowseError> {
    let vpath = OpendalFileStorage::normalize_path(path);
    storage.check_readable(&vpath)?;
    let size = storage.content_length(&vpath).await?;
    let cap = (max_bytes as u64).min(size);
    let buffer = storage.read_range(&vpath, 0..cap).await?;
    let truncated = size > cap;
    let contents = String::from_utf8(buffer.to_vec()).map_err(|_| {
        BrowseError::Invalid(format!("'{vpath}' is not valid UTF-8 text"))
    })?;
    Ok(TextRead { path: vpath, contents, size, truncated })
}

/// Create a directory.
pub async fn mkdir(storage: &OpendalFileStorage, path: &str) -> Result<String, BrowseError> {
    let vpath = OpendalFileStorage::normalize_path(path);
    storage.check_writable(&vpath)?;
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    // OpenDAL Fs backend requires a trailing '/' for directory creation.
    let dir_path = if remote.is_empty() {
        "/".to_string()
    } else if remote.ends_with('/') {
        remote
    } else {
        format!("{remote}/")
    };
    op.create_dir(&dir_path).await?;
    Ok(vpath)
}

/// Delete a file or directory.
///
/// Safety contract (inherited from the vbash `rm` op):
/// - the virtual root `/` is always refused;
/// - missing targets are errors, not silent successes;
/// - non-empty directories are refused unless `recursive` is set; empty
///   directories are removed without the flag.
pub async fn remove(
    storage: &OpendalFileStorage,
    path: &str,
    recursive: bool,
) -> Result<String, BrowseError> {
    let vpath = OpendalFileStorage::normalize_path(path);
    if vpath == "/" {
        return Err(BrowseError::Invalid(
            "Refusing to rm '/': that is the virtual filesystem root. \
             Use a sub-path like '/tmp' instead."
                .into(),
        ));
    }
    storage.check_writable(&vpath)?;
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = op.stat(&remote).await.map_err(|e| match e.kind() {
        opendal::ErrorKind::NotFound => BrowseError::NotFound(format!(
            "cannot remove '{vpath}': No such file or directory"
        )),
        _ => BrowseError::from(e),
    })?;

    if meta.is_dir() && !recursive {
        // Empty-directory removal: succeeds only if the directory has no
        // entries. Non-empty directories are explicitly refused here so
        // the caller sees a clear error instead of a silent wipe.
        return match op.delete(&remote).await {
            Ok(()) => Ok(vpath),
            Err(e) => Err(BrowseError::Conflict(format!(
                "cannot remove '{vpath}': {e}. \
                 If the directory is non-empty, pass recursive=true to remove it and its contents."
            ))),
        };
    }

    op.delete_with(&remote).recursive(recursive).await?;
    Ok(vpath)
}

/// Copy a file (or same-backend subtree). Cross-backend copies fall back
/// to read + write.
pub async fn copy(
    storage: &OpendalFileStorage,
    src: &str,
    dst: &str,
) -> Result<(String, String), BrowseError> {
    let vsrc = OpendalFileStorage::normalize_path(src);
    let vdst = OpendalFileStorage::normalize_path(dst);
    storage.check_readable(&vsrc)?;
    storage.check_writable(&vdst)?;
    let src_op = storage.resolve(&vsrc);
    let dst_op = storage.resolve(&vdst);
    let src_remote = storage.resolve_path(&vsrc);
    let dst_remote = storage.resolve_path(&vdst);
    // OpenDAL copy is backend-internal. If src and dst resolve to
    // different backends, fall back to read+write.
    if !storage.same_backend(&vsrc, &vdst) {
        let buffer = src_op.read(&src_remote).await?;
        dst_op.write(&dst_remote, buffer).await?;
    } else {
        src_op.copy(&src_remote, &dst_remote).await?;
    }
    Ok((vsrc, vdst))
}

/// Rename/move a file. Requires write access on **both** ends so nothing
/// can be smuggled out of (or into) a read-only mount; cross-backend
/// moves fall back to read + write + delete.
pub async fn move_entry(
    storage: &OpendalFileStorage,
    src: &str,
    dst: &str,
) -> Result<(String, String), BrowseError> {
    let vsrc = OpendalFileStorage::normalize_path(src);
    let vdst = OpendalFileStorage::normalize_path(dst);
    storage.check_writable(&vsrc)?;
    storage.check_writable(&vdst)?;
    let src_op = storage.resolve(&vsrc);
    let dst_op = storage.resolve(&vdst);
    let src_remote = storage.resolve_path(&vsrc);
    let dst_remote = storage.resolve_path(&vdst);
    if !storage.same_backend(&vsrc, &vdst) {
        let buffer = src_op.read(&src_remote).await?;
        dst_op.write(&dst_remote, buffer).await?;
        src_op.delete(&src_remote).await?;
    } else {
        src_op.rename(&src_remote, &dst_remote).await?;
    }
    Ok((vsrc, vdst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn mounted_storage(read_only_mount: bool) -> (OpendalFileStorage, tempfile::TempDir) {
        let backend_root = tempfile::tempdir().unwrap();
        let source = backend_root.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("panel.parquet"), b"panel").unwrap();
        std::fs::write(source.join("nested/data.csv"), b"data").unwrap();
        let manifest = crate::VfsManifest {
            backend: vec![crate::BackendDefinition {
                id: "default".into(),
                config: crate::BackendConfig::local(
                    backend_root.path().to_string_lossy().to_string(),
                ),
            }],
            mount: vec![
                crate::MountDefinition {
                    path: "/".into(),
                    backend: "default".into(),
                    source: "/".into(),
                    read_only: false,
                    permissions: Default::default(),
                },
                crate::MountDefinition {
                    path: "/data/ldsc".into(),
                    backend: "default".into(),
                    source: "source".into(),
                    read_only: read_only_mount,
                    permissions: Default::default(),
                },
            ],
        };
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            vfs,
        );
        (storage, backend_root)
    }

    fn names(listing: &VfsListing) -> Vec<&str> {
        listing.entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[tokio::test]
    async fn list_root_synthesizes_mount_ancestor() {
        let (storage, _source) = mounted_storage(false);
        let listing = list(&storage, "/", false, None, None).await.unwrap();
        assert_eq!(listing.path, "/");
        assert!(
            names(&listing).contains(&"/data"),
            "expected synthetic ancestor, got {:?}",
            names(&listing)
        );
    }

    #[tokio::test]
    async fn list_mount_point_lists_mount_contents() {
        let (storage, _source) = mounted_storage(false);
        let listing = list(&storage, "/data", false, None, None).await.unwrap();
        assert_eq!(names(&listing), vec!["/data/ldsc"]);
        let inner = list(&storage, "/data/ldsc", false, None, None).await.unwrap();
        assert!(
            names(&inner).contains(&"/data/ldsc/panel.parquet"),
            "got {:?}",
            names(&inner)
        );
    }

    #[tokio::test]
    async fn pagination_prioritizes_mount_points() {
        let (storage, _source) = mounted_storage(false);
        let listing = list(&storage, "/", false, Some(1), None).await.unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].path, "/data");
        assert!(listing.truncated);
        assert_eq!(listing.next_offset, Some(1));
    }

    #[tokio::test]
    async fn read_only_mount_rejects_writes_but_allows_reads() {
        let (storage, _source) = mounted_storage(true);
        let text = read_text(&storage, "/data/ldsc/panel.parquet", 1024)
            .await
            .unwrap();
        assert_eq!(text.contents, "panel");
        let err = mkdir(&storage, "/data/ldsc/new-dir").await.unwrap_err();
        assert!(matches!(err, BrowseError::PermissionDenied(_)), "{err:?}");
        let err = remove(&storage, "/data/ldsc/panel.parquet", false)
            .await
            .unwrap_err();
        assert!(matches!(err, BrowseError::PermissionDenied(_)), "{err:?}");
        let err = move_entry(&storage, "/data/ldsc/panel.parquet", "/tmp-out")
            .await
            .unwrap_err();
        assert!(matches!(err, BrowseError::PermissionDenied(_)), "{err:?}");
    }

    #[tokio::test]
    async fn remove_guards_root_and_nonempty_directories() {
        let (storage, _source) = mounted_storage(false);
        assert!(matches!(
            remove(&storage, "/", true).await.unwrap_err(),
            BrowseError::Invalid(_)
        ));
        mkdir(&storage, "/data/ldsc/sub").await.unwrap();
        // Directory exists but has entries only after we add one.
        crate::OpendalFileStorage::write_bytes(
            &storage,
            "/data/ldsc/sub/file.txt",
            b"data".to_vec(),
        )
        .await
        .unwrap();
        let err = remove(&storage, "/data/ldsc/sub", false).await.unwrap_err();
        assert!(matches!(err, BrowseError::Conflict(_)), "{err:?}");
        remove(&storage, "/data/ldsc/sub", true).await.unwrap();
        let err = remove(&storage, "/data/ldsc/missing", false).await.unwrap_err();
        assert!(matches!(err, BrowseError::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn stat_and_copy_roundtrip() {
        let (storage, _source) = mounted_storage(false);
        let entry = stat(&storage, "/data/ldsc/panel.parquet").await.unwrap();
        assert_eq!(entry.is_dir, false);
        assert_eq!(entry.size, 5);
        copy(&storage, "/data/ldsc/panel.parquet", "/copied.parquet")
            .await
            .unwrap();
        let copied = stat(&storage, "/copied.parquet").await.unwrap();
        assert_eq!(copied.size, 5);
        move_entry(&storage, "/copied.parquet", "/moved.parquet")
            .await
            .unwrap();
        assert!(
            stat(&storage, "/copied.parquet").await.is_err(),
            "source should be gone after move"
        );
        assert_eq!(stat(&storage, "/moved.parquet").await.unwrap().size, 5);
    }

    #[tokio::test]
    async fn covering_mount_reports_definition() {
        let (storage, _source) = mounted_storage(true);
        let mount = covering_mount(&storage, "/data/ldsc/panel.parquet").unwrap();
        assert_eq!(mount.path, "/data/ldsc");
        assert!(mount.read_only);
        assert!(!mounts(&storage).is_empty());
    }
}
