//! The capability contract of the Podman connection layer.
//!
//! [`PodmanConnection`] declares every capability consumers may rely on: the
//! control process and Podman must resolve workspace and panel paths on the
//! same host, and one request maps to one ephemeral container. The CLI
//! implementation lives in [`crate::podman`]; tests substitute fakes that
//! implement the same trait.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::error::ContainerRuntimeError;
use crate::types::{ContainerRunRequest, ContainerRunResult, WorkspaceRef};

pub const DEFAULT_CONTAINER_WORKDIR: &str = "/work";
pub const DEFAULT_TIMEOUT_SECS: u64 = 3600;
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

/// One connection to a Podman runtime on the local host.
///
/// The trait fixes the full capability surface of the connection layer: every
/// consumer-visible operation goes through [`PodmanConnection::run`], and the
/// shared workspace root is the single piece of host state both sides must
/// agree on.
#[async_trait]
pub trait PodmanConnection: Send + Sync {
    /// Run one ephemeral container: create it, attach to its start, and
    /// force-remove it after success, failure, or timeout.
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError>;

    /// Root of the shared workspace directory tree as seen by this control
    /// process. Container workspaces are created below it and bind-mounted
    /// into each container.
    fn workspace_root(&self) -> &Path;

    /// Connection implementation name, used for diagnostics only.
    fn name(&self) -> &'static str {
        "podman"
    }
}

pub fn unique_container_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "autonomics-container-command-{}-{nanos}",
        std::process::id()
    )
}

pub(crate) fn validate_run_request(
    request: &ContainerRunRequest,
) -> Result<(), ContainerRuntimeError> {
    if request.image.trim().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`image` cannot be empty".into(),
        ));
    }
    if request.command.is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`command` cannot be empty".into(),
        ));
    }
    if request.timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    if !request.workspace.host_path.is_absolute()
        || !request.workspace.host_path.is_dir()
        || !Path::new(&request.workspace.container_workdir).is_absolute()
    {
        return Err(ContainerRuntimeError::Invalid(
            "workspace must be an existing directory mapped to an absolute container path".into(),
        ));
    }
    if let Some(cpus) = request.cpus
        && cpus <= 0.0
    {
        return Err(ContainerRuntimeError::Invalid(
            "`cpus` must be positive".into(),
        ));
    }
    if let Some(pids_limit) = request.pids_limit
        && pids_limit <= 0
    {
        return Err(ContainerRuntimeError::Invalid(
            "`pids_limit` must be positive".into(),
        ));
    }
    validate_user(&request.user)?;
    for panel in &request.panels {
        panel
            .host_path
            .file_name()
            .ok_or_else(|| ContainerRuntimeError::Invalid("invalid panel cache path".into()))?;
    }
    validate_input_mounts(request)?;
    Ok(())
}

/// Host locations that must never be bind-mounted for input staging. `/tmp`
/// is podman-managed tmpfs (`--tmpfs /tmp`), the rest are kernel or runtime
/// virtual filesystems. The filesystem root itself is rejected separately:
/// `Path::starts_with("/")` matches every absolute path, so it must not be
/// part of this prefix list.
const UNSAFE_INPUT_MOUNT_ROOTS: [&str; 5] = ["/tmp", "/dev", "/proc", "/sys", "/run"];

fn validate_input_mounts(request: &ContainerRunRequest) -> Result<(), ContainerRuntimeError> {
    let workspace_mount = Path::new(&request.workspace.container_workdir);
    let mut seen = std::collections::BTreeSet::new();
    for mount in &request.input_mounts {
        let dir = &mount.host_dir;
        if dir == Path::new("/")
            || UNSAFE_INPUT_MOUNT_ROOTS
                .iter()
                .any(|root| dir.starts_with(root))
        {
            return Err(ContainerRuntimeError::Invalid(format!(
                "input mount `{}` is the filesystem root or below an unsafe host location",
                dir.display()
            )));
        }
        if !dir.is_absolute() || !dir.is_dir() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "input mount `{}` must be an existing absolute directory",
                dir.display()
            )));
        }
        if !seen.insert(dir.clone()) {
            return Err(ContainerRuntimeError::Invalid(format!(
                "duplicate input mount `{}`",
                dir.display()
            )));
        }
        // Identity mapping: the container destination equals the host path,
        // so it must not overlap the workspace or any panel mount either way.
        if dir.starts_with(workspace_mount) || workspace_mount.starts_with(dir) {
            return Err(ContainerRuntimeError::Invalid(format!(
                "input mount `{}` overlaps the container workdir `{}`",
                dir.display(),
                workspace_mount.display()
            )));
        }
        for panel in &request.panels {
            let panel_mount = Path::new(&panel.mount_path);
            if dir.starts_with(panel_mount) || panel_mount.starts_with(dir) {
                return Err(ContainerRuntimeError::Invalid(format!(
                    "input mount `{}` overlaps panel `{}` mount path `{}`",
                    dir.display(),
                    panel.id,
                    panel.mount_path
                )));
            }
        }
    }
    Ok(())
}

fn validate_user(user: &Option<String>) -> Result<(), ContainerRuntimeError> {
    let Some(user) = user else {
        return Ok(());
    };
    let mut parts = user.split(':');
    let uid_valid = parts.next().is_some_and(|uid| uid.parse::<i64>().is_ok());
    let gid = parts.next();
    let gid_valid = match gid {
        Some(gid) => gid.parse::<i64>().is_ok(),
        None => true,
    };
    if !uid_valid || !gid_valid || parts.next().is_some() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "`user` must be `uid:gid` or `uid`, got `{user}`"
        )));
    }
    Ok(())
}

/// Bind a host directory below `workspace_root` to a container workdir.
///
/// Fails when the directory is outside the configured root or maps to the
/// volume root itself.
pub fn workspace_ref(
    workspace_root: &Path,
    host_path: &Path,
    container_workdir: &str,
) -> Result<WorkspaceRef, ContainerRuntimeError> {
    let relative = host_path.strip_prefix(workspace_root).map_err(|_| {
        ContainerRuntimeError::Invalid(format!(
            "workspace `{}` is outside configured workspace root `{}`",
            host_path.display(),
            workspace_root.display()
        ))
    })?;
    if relative.as_os_str().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "workspace cannot map to the volume root".into(),
        ));
    }
    Ok(WorkspaceRef {
        host_path: host_path.to_path_buf(),
        container_workdir: container_workdir.to_string(),
    })
}

/// Bind a daemon-owned plugin workspace without requiring it to live below
/// the generic scratch workspace root.
///
/// The caller must already hold host authority over `host_path`; this check
/// still validates that the path is an existing directory and canonicalizes
/// symlinks before Podman receives the mount.
pub fn trusted_workspace_ref(
    host_path: &Path,
    container_workdir: &str,
) -> Result<WorkspaceRef, ContainerRuntimeError> {
    let canonical = host_path.canonicalize().map_err(|source| {
        ContainerRuntimeError::Invalid(format!(
            "trusted workspace `{}` is unavailable: {source}",
            host_path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "trusted workspace `{}` is not a directory",
            canonical.display()
        )));
    }
    if !container_workdir.starts_with('/') || container_workdir == "/" {
        return Err(ContainerRuntimeError::Invalid(
            "trusted workspace container directory must be an absolute non-root path".into(),
        ));
    }
    Ok(WorkspaceRef {
        host_path: canonical,
        container_workdir: container_workdir.to_string(),
    })
}

pub(crate) fn request_user_ids(request: &ContainerRunRequest) -> (i64, i64) {
    #[cfg(unix)]
    let default = (unsafe { libc::getuid() as i64 }, unsafe {
        libc::getgid() as i64
    });
    #[cfg(not(unix))]
    let default = (1000, 1000);
    let default_gid = default.1;

    request
        .user
        .as_deref()
        .and_then(|user| {
            let mut parts = user.split(':');
            let uid = parts.next()?.parse::<i64>().ok()?;
            let gid = parts
                .next()
                .and_then(|gid| gid.parse::<i64>().ok())
                .unwrap_or(default_gid);
            Some((uid, gid))
        })
        .unwrap_or(default)
}

pub(crate) fn truncate_captured_bytes(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_CAPTURED_OUTPUT_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    format!(
        "{}\n[output truncated at {} bytes]",
        String::from_utf8_lossy(&bytes[..MAX_CAPTURED_OUTPUT_BYTES]),
        MAX_CAPTURED_OUTPUT_BYTES
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_must_be_inside_configured_root() {
        let ok = workspace_ref(
            Path::new("/workspace"),
            Path::new("/workspace/runs/x"),
            "/work",
        )
        .unwrap();
        assert_eq!(ok.host_path, Path::new("/workspace/runs/x"));
        assert_eq!(ok.container_workdir, "/work");
        assert!(workspace_ref(Path::new("/workspace"), Path::new("/tmp/x"), "/work").is_err());
        assert!(workspace_ref(Path::new("/workspace"), Path::new("/workspace"), "/work").is_err());
    }

    #[test]
    fn trusted_workspace_can_use_daemon_owned_plugin_directories() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("plugin");
        std::fs::create_dir_all(&workspace).unwrap();
        let reference = trusted_workspace_ref(&workspace, "/work").unwrap();
        assert_eq!(reference.host_path, workspace.canonicalize().unwrap());
        assert!(trusted_workspace_ref(&workspace, "/").is_err());
        assert!(trusted_workspace_ref(&root.path().join("missing"), "/work").is_err());
    }
}
