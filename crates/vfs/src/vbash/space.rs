//! Read-only capacity snapshots. These are never quota or resource admission.
use std::path::{Component, Path};

use agentik_core::tools::ToolError;
use agentik_sdk::types::ToolResult;
use serde_json::{Value, json};

use crate::storage::OpendalFileStorage;

const SCOPE: &str = "FILESYSTEM_CAPACITY_SNAPSHOT_NOT_RESOURCE_ADMISSION";

fn unavailable(backend: &str, reason: &str) -> Value {
    json!({"status":"UNAVAILABLE","backend":backend,"reason":reason,
        "scope":SCOPE,"observed_at":chrono::Utc::now().to_rfc3339(),
        "total_bytes":null,"free_bytes":null,"available_to_user_bytes":null,
        "availability_identity":"RUNTIME_OS_USER_NOT_VIRTUAL_PRINCIPAL",
        "quota_remaining_bytes":null,"quota_status":"UNKNOWN",
        "btrfs_physical_unallocated_bytes":null,"btrfs_physical_unallocated_status":"UNKNOWN",
        "admission_granted":false})
}

fn mount_prefix(storage: &OpendalFileStorage, path: &str) -> String {
    let mut mounts = storage.mount_paths();
    mounts.sort_by_key(|p| std::cmp::Reverse(p.len()));
    mounts
        .into_iter()
        .find(|prefix| {
            prefix == "/"
                || path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
        .unwrap_or_else(|| "/".into())
}

fn readable(storage: &OpendalFileStorage, path: &str) -> bool {
    storage.check_readable(path).is_ok() && storage.check_listable(path).is_ok()
}

fn virtual_parent(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let parent = Path::new(path).parent()?.to_str()?;
    Some(if parent.is_empty() {
        "/".into()
    } else {
        parent.into()
    })
}

/// No remote requests or file contents are read. Missing target suffixes use
/// only an existing authorized ancestor inside the same selected mount.
pub async fn op_df(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<ToolResult, ToolError> {
    let raw = path
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::ValidationFailed {
            message: "missing or empty 'path' for df".into(),
        })?;
    let raw = raw.strip_prefix("vfs://").unwrap_or(raw);
    if raw.chars().any(char::is_control)
        || Path::new(raw)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Ok(ToolResult::error(
            "df: parent traversal or control characters refused",
        ));
    }
    let requested = OpendalFileStorage::normalize_path(raw);
    if !readable(storage, &requested) {
        return Ok(ToolResult::error("df: target not readable/listable"));
    }
    let backend = storage.resolve(&requested).info().scheme().to_string();
    let Some(boundary) = storage.local_host_path(&mount_prefix(storage, &requested)) else {
        let mut result = unavailable(&backend, "NONLOCAL_BACKEND");
        result["path"] = requested.into();
        return Ok(ToolResult::success_json(result));
    };
    let boundary = match std::fs::canonicalize(&boundary) {
        Ok(value) => value,
        Err(_) => {
            return Ok(ToolResult::success_json(unavailable(
                &backend,
                "LOCAL_MOUNT_ROOT_UNAVAILABLE",
            )));
        }
    };
    let prefix = mount_prefix(storage, &requested);
    let mut existing = requested.clone();
    let actual = loop {
        if mount_prefix(storage, &existing) != prefix || !readable(storage, &existing) {
            return Ok(ToolResult::error(
                "df: parent outside readable selected mount",
            ));
        }
        let Some(host) = storage.local_host_path(&existing) else {
            return Ok(ToolResult::error(
                "df: backend changed while resolving parent",
            ));
        };
        match std::fs::symlink_metadata(&host) {
            Ok(_) => match std::fs::canonicalize(&host) {
                Ok(value) => break value,
                Err(_) => {
                    return Ok(ToolResult::error(
                        "df: existing path cannot be resolved safely",
                    ));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && existing != prefix => {
                let Some(parent) = virtual_parent(&existing) else {
                    return Ok(ToolResult::error("df: existing parent unavailable"));
                };
                existing = parent;
            }
            Err(_) => {
                return Ok(ToolResult::success_json(unavailable(
                    &backend,
                    "LOCAL_PATH_UNAVAILABLE",
                )));
            }
        }
    };
    let Ok(relative) = actual.strip_prefix(&boundary) else {
        return Ok(ToolResult::error("df: symlink escapes selected mount"));
    };
    let alias = OpendalFileStorage::normalize_path(&format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        relative.to_string_lossy()
    ));
    if mount_prefix(storage, &alias) != prefix || !readable(storage, &alias) {
        return Ok(ToolResult::error(
            "df: resolved alias outside readable selected mount",
        ));
    }
    // Reject re-routing into another mount even if a physical alias looked local.
    if storage
        .local_host_path(&alias)
        .and_then(|p| std::fs::canonicalize(p).ok())
        .as_ref()
        != Some(&actual)
    {
        return Ok(ToolResult::error("df: resolved alias backend differs"));
    }
    let mut result = snapshot(&boundary, relative, &backend);
    result["path"] = requested.into();
    result["sampled_existing_vfs_path"] = existing.into();
    result["resolved_existing_vfs_path"] = alias.into();
    Ok(ToolResult::success_json(result))
}

/// The runtime, not the caller, chooses this one host directory. This does not
/// measure a container's private /tmp or promise a plugin uses this directory.
pub async fn op_temp_space(path: Option<&str>) -> Result<ToolResult, ToolError> {
    if path.is_some() {
        return Ok(ToolResult::error(
            "temp_space: path is fixed by the runtime; no path argument allowed",
        ));
    }
    let temp = std::env::temp_dir();
    let mut result = match std::fs::canonicalize(&temp) {
        Ok(actual) => snapshot(&actual, Path::new("."), "runtime-local-fs"),
        Err(_) => unavailable("runtime-local-fs", "RUNTIME_TEMP_DIRECTORY_UNAVAILABLE"),
    };
    result["runtime_temp_directory"] = temp.to_string_lossy().as_ref().into();
    result["temp_scope"] = "DAEMON_PROCESS_TEMP_NOT_CONTAINER_TMP_OR_ADMISSION".into();
    Ok(ToolResult::success_json(result))
}

fn checked_capacity(blocks: u64, free: u64, available: u64, unit: u64) -> Option<(u64, u64, u64)> {
    if unit == 0 {
        return None;
    }
    Some((
        blocks.checked_mul(unit)?,
        free.checked_mul(unit)?,
        available.checked_mul(unit)?,
    ))
}

#[cfg(target_os = "linux")]
fn snapshot(boundary: &Path, relative: &Path, backend: &str) -> Value {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    fn open_at(dirfd: i32, path: &Path, directory: bool, resolve: u64) -> std::io::Result<OwnedFd> {
        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let how = OpenHow {
            flags: (libc::O_PATH | libc::O_CLOEXEC | if directory { libc::O_DIRECTORY } else { 0 })
                as u64,
            mode: 0,
            resolve,
        };
        // SAFETY: CString and repr(C) OpenHow remain live for this syscall;
        // successful ownership is transferred exactly once to OwnedFd.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                dirfd,
                name.as_ptr(),
                &how as *const OpenHow,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }
    // The already-canonical boundary cannot change into symlinks while opened.
    let root = match open_at(libc::AT_FDCWD, boundary, true, 0x04 | 0x02) {
        Ok(fd) => fd,
        Err(_) => return unavailable(backend, "SAFE_LOCAL_BOUNDARY_OPEN_UNAVAILABLE"),
    };
    let relative = if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    };
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return unavailable(backend, "INVALID_RELATIVE_PATH");
    }
    // RESOLVE_BENEATH|NO_SYMLINKS|NO_MAGICLINKS prohibits traversal races.
    let target = match open_at(root.as_raw_fd(), relative, false, 0x08 | 0x04 | 0x02) {
        Ok(fd) => fd,
        Err(_) => return unavailable(backend, "SAFE_LOCAL_TARGET_OPEN_UNAVAILABLE"),
    };
    let mut raw = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: target is a live owned FD, and fstatvfs initializes raw on success.
    if unsafe { libc::fstatvfs(target.as_raw_fd(), raw.as_mut_ptr()) } != 0 {
        return unavailable(backend, "STATVFS_UNAVAILABLE");
    }
    let raw = unsafe { raw.assume_init() };
    let Some((total, free, available)) = checked_capacity(
        raw.f_blocks as u64,
        raw.f_bfree as u64,
        raw.f_bavail as u64,
        raw.f_frsize as u64,
    ) else {
        return unavailable(backend, "SPACE_COUNTER_OVERFLOW_OR_UNKNOWN_UNIT");
    };
    json!({"status":"AVAILABLE","backend":backend,"scope":SCOPE,
        "observed_at":chrono::Utc::now().to_rfc3339(),"total_bytes":total,
        "free_bytes":free,"available_to_user_bytes":available,
        "availability_identity":"RUNTIME_OS_USER_NOT_VIRTUAL_PRINCIPAL",
        "quota_remaining_bytes":null,"quota_status":"UNKNOWN",
        "btrfs_physical_unallocated_bytes":null,"btrfs_physical_unallocated_status":"UNKNOWN",
        "admission_granted":false})
}

#[cfg(not(target_os = "linux"))]
fn snapshot(_: &Path, _: &Path, backend: &str) -> Value {
    unavailable(backend, "SAFE_LOCAL_SPACE_QUERY_UNSUPPORTED_ON_PLATFORM")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolResultContent;
    use std::sync::Arc;
    fn value(result: ToolResult) -> Value {
        assert_ne!(result.is_error, Some(true));
        match result.content {
            ToolResultContent::Json(v) => v,
            other => panic!("expected JSON, got {other:?}"),
        }
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn df_local_existing_and_missing_descendant_measure_real_filesystem_without_creation() {
        let root = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(root.path());
        let actual = value(op_df(&storage, Some("/")).await.unwrap());
        assert_eq!(actual["status"], "AVAILABLE");
        assert!(actual["total_bytes"].as_u64().unwrap() > 0);
        assert!(actual["free_bytes"].as_u64().unwrap() <= actual["total_bytes"].as_u64().unwrap());
        assert!(
            actual["available_to_user_bytes"].as_u64().unwrap()
                <= actual["free_bytes"].as_u64().unwrap()
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(actual["observed_at"].as_str().unwrap()).is_ok()
        );
        let missing = value(op_df(&storage, Some("/not-created/child")).await.unwrap());
        assert_eq!(missing["status"], "AVAILABLE");
        assert_eq!(missing["total_bytes"], actual["total_bytes"]);
        assert_eq!(missing["sampled_existing_vfs_path"], "/");
        assert!(!root.path().join("not-created").exists());
    }

    #[test]
    fn unknown_capacity_is_null_and_true_zero_is_preserved() {
        let value = unavailable("memory", "NONLOCAL_BACKEND");
        for key in [
            "total_bytes",
            "free_bytes",
            "available_to_user_bytes",
            "quota_remaining_bytes",
            "btrfs_physical_unallocated_bytes",
        ] {
            assert!(value[key].is_null());
        }
        assert_eq!(value["admission_granted"], false);
        assert_eq!(checked_capacity(10, 0, 0, 4096), Some((40960, 0, 0)));
        assert_eq!(checked_capacity(u64::MAX, 0, 0, 4096), None);
        assert_eq!(checked_capacity(10, 0, 0, 0), None);
    }

    #[tokio::test]
    async fn df_nonlocal_is_unavailable_without_numeric_capacity_or_remote_request() {
        let op = opendal::Operator::new(opendal::services::Memory::default())
            .unwrap()
            .finish();
        let storage = OpendalFileStorage::from_operator(op);
        let actual = value(op_df(&storage, Some("/missing")).await.unwrap());
        assert_eq!(actual["status"], "UNAVAILABLE");
        assert_eq!(actual["reason"], "NONLOCAL_BACKEND");
        assert!(actual["available_to_user_bytes"].is_null());
    }

    #[tokio::test]
    async fn temp_space_has_no_caller_selected_host_path() {
        assert_eq!(
            op_temp_space(Some("/etc")).await.unwrap().is_error,
            Some(true)
        );
        assert_eq!(op_temp_space(Some("")).await.unwrap().is_error, Some(true));
        let actual = value(op_temp_space(None).await.unwrap());
        assert_eq!(
            actual["runtime_temp_directory"],
            std::env::temp_dir().to_string_lossy().as_ref()
        );
        assert_eq!(
            actual["temp_scope"],
            "DAEMON_PROCESS_TEMP_NOT_CONTAINER_TMP_OR_ADMISSION"
        );
        assert!(actual["quota_remaining_bytes"].is_null());
    }

    fn mounted(root: &Path, read_only: bool) -> OpendalFileStorage {
        use crate::permission::{MountPermissions, VfsAccess, VfsPathRule, VfsPrincipal};
        let manifest = crate::VfsManifest {
            backend: vec![crate::BackendDefinition {
                id: "local".into(),
                config: crate::BackendConfig::local(root.to_string_lossy().into_owned()),
            }],
            mount: vec![crate::MountDefinition {
                path: "/mounted".into(),
                backend: "local".into(),
                source: "/".into(),
                read_only,
                permissions: MountPermissions::default().with_rules(vec![
                    VfsPathRule::Deny {
                        path: "denied".into(),
                        access: vec![VfsAccess::Read, VfsAccess::Execute],
                    },
                    VfsPathRule::Deny {
                        path: "denied/**".into(),
                        access: vec![VfsAccess::Read, VfsAccess::Execute],
                    },
                ]),
            }],
        };
        let mounts = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        OpendalFileStorage::with_mounts(root, mounts)
            .with_principal(VfsPrincipal::plugin_developer(10000))
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn df_read_only_mount_is_observable_and_denied_paths_stay_denied() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("denied")).unwrap();
        let storage = mounted(root.path(), true);
        assert!(storage.check_writable("/mounted").is_err());
        assert_eq!(
            value(op_df(&storage, Some("/mounted")).await.unwrap())["status"],
            "AVAILABLE"
        );
        assert_eq!(
            op_df(&storage, Some("/mounted/denied/child"))
                .await
                .unwrap()
                .is_error,
            Some(true)
        );
        assert_eq!(
            op_df(&storage, Some("/mounted/../outside"))
                .await
                .unwrap()
                .is_error,
            Some(true)
        );
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn df_symlink_escape_and_in_mount_denied_alias_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("denied")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        std::os::unix::fs::symlink(root.path().join("denied"), root.path().join("alias")).unwrap();
        let storage = mounted(root.path(), false);
        for path in [
            "/mounted/escape",
            "/mounted/escape/new/child",
            "/mounted/alias",
        ] {
            assert_eq!(
                op_df(&storage, Some(path)).await.unwrap().is_error,
                Some(true),
                "{path}"
            );
        }
        std::fs::create_dir(root.path().join("allowed")).unwrap();
        std::os::unix::fs::symlink(root.path().join("allowed"), root.path().join("safe-alias"))
            .unwrap();
        let safe = value(op_df(&storage, Some("/mounted/safe-alias")).await.unwrap());
        assert_eq!(safe["status"], "AVAILABLE");
        assert_eq!(safe["resolved_existing_vfs_path"], "/mounted/allowed");
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn df_alias_cannot_cross_a_nested_virtual_mount() {
        let root = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("inner")).unwrap();
        std::os::unix::fs::symlink(root.path().join("inner"), root.path().join("alias")).unwrap();
        let manifest = crate::VfsManifest {
            backend: vec![
                crate::BackendDefinition {
                    id: "outer".into(),
                    config: crate::BackendConfig::local(root.path().to_string_lossy().into_owned()),
                },
                crate::BackendDefinition {
                    id: "inner".into(),
                    config: crate::BackendConfig::local(
                        inner.path().to_string_lossy().into_owned(),
                    ),
                },
            ],
            mount: vec![
                crate::MountDefinition {
                    path: "/mounted".into(),
                    backend: "outer".into(),
                    source: "/".into(),
                    read_only: true,
                    permissions: Default::default(),
                },
                crate::MountDefinition {
                    path: "/mounted/inner".into(),
                    backend: "inner".into(),
                    source: "/".into(),
                    read_only: true,
                    permissions: Default::default(),
                },
            ],
        };
        let storage = OpendalFileStorage::with_mounts(
            root.path(),
            Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap()),
        );
        assert_eq!(
            op_df(&storage, Some("/mounted/alias"))
                .await
                .unwrap()
                .is_error,
            Some(true)
        );
        assert_eq!(
            value(
                op_df(&storage, Some("/mounted/inner/future"))
                    .await
                    .unwrap()
            )["status"],
            "AVAILABLE"
        );
    }
}
