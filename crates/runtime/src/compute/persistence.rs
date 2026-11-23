//! Private, ownership-checked filesystem primitives for compute state.
//! Every registry write goes through the atomic publication helpers here.
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use super::built::ExecutionBinding;

pub(super) const MAX_CONFIGURATION_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn identifier(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(
            "identifier must contain 1-64 ASCII letters, digits, underscores or dashes".into(),
        );
    }
    Ok(())
}

pub(crate) fn private_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let created = match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.to_string()),
    };
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
        return Err("compute state directory must be owned, private and nonsymlink".into());
    }
    if created {
        fs::File::open(path.parent().ok_or("compute directory requires a parent")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn read_private(path: &Path) -> Result<Vec<u8>, String> {
    read_owned(path, true)
}

pub(super) fn read_owned(path: &Path, private: bool) -> Result<Vec<u8>, String> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & if private { 0o077 } else { 0o022 } != 0
        || m.nlink() != 1
        || m.len() > MAX_CONFIGURATION_BYTES
    {
        return Err("compute configuration must be a bounded private regular file".into());
    }
    let mut bytes = Vec::new();
    (&mut f)
        .take(MAX_CONFIGURATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = f.metadata().map_err(|e| e.to_string())?;
    let named = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let identity = |v: &fs::Metadata| {
        (
            v.dev(),
            v.ino(),
            v.mode(),
            v.uid(),
            v.gid(),
            v.nlink(),
            v.len(),
            v.mtime(),
            v.mtime_nsec(),
            v.ctime(),
            v.ctime_nsec(),
        )
    };
    if identity(&m) != identity(&after)
        || identity(&after) != identity(&named)
        || bytes.len() as u64 > MAX_CONFIGURATION_BYTES
    {
        return Err("compute configuration changed while reading".into());
    }
    Ok(bytes)
}

pub(crate) fn write_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    write_bytes_new(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}
pub(super) fn write_bytes_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())?;
    fs::File::open(path.parent().unwrap())
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
/// Atomically publish registry state so readers never observe a partial file.
pub(super) fn write_state(root: &Path, bytes: &[u8]) -> Result<(), String> {
    let pending = root.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    write_bytes_new(&pending, bytes)?;
    fs::rename(&pending, root.join("registry.json")).map_err(|e| e.to_string())?;
    fs::File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
/// Publish a complete binding without replacing historical evidence. Pending
/// bytes survive a failed write/publication, but are never read as run records.
pub(super) fn write_binding(path: &Path, binding: &ExecutionBinding) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let parent = path.parent().ok_or("binding path requires a parent")?;
    let pending = parent.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    write_new(&pending, binding)?;
    let from = std::ffi::CString::new(pending.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: both paths are live NUL-terminated strings. Linux's no-replace
    // rename atomically exposes the synced file and preserves an existing ID.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
