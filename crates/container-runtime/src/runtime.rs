use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::error::ContainerRuntimeError;
use crate::types::{ContainerRunRequest, ContainerRunResult, WorkspaceRef};

pub const DEFAULT_CONTAINER_WORKDIR: &str = "/work";
pub const DEFAULT_TIMEOUT_SECS: u64 = 3600;
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

/// Runtime-neutral execution of one ephemeral container.
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError>;

    fn name(&self) -> &'static str {
        "container"
    }

    /// Root of the shared workspace volume as seen by this control process.
    fn workspace_root(&self) -> &Path {
        Path::new("/")
    }
}

pub type SharedContainerRuntime = Arc<dyn ContainerRuntime>;

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
        || request.workspace.pvc_sub_path.contains("..")
        || request.workspace.pvc_sub_path.starts_with('/')
    {
        return Err(ContainerRuntimeError::Invalid(
            "workspace must be an existing directory mapped to a safe volume subPath".into(),
        ));
    }
    if !matches!(
        request.network.as_str(),
        "isolated" | "none" | "cluster" | "egress"
    ) {
        return Err(ContainerRuntimeError::Invalid(format!(
            "unsupported network profile `{}`",
            request.network
        )));
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
        if panel.pvc_sub_path.contains("..") || panel.pvc_sub_path.starts_with('/') {
            return Err(ContainerRuntimeError::Invalid(format!(
                "panel `{}` has an unsafe volume subPath",
                panel.id
            )));
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
    let volume_sub_path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if volume_sub_path.is_empty()
        || volume_sub_path.contains("..")
        || volume_sub_path.starts_with('/')
    {
        return Err(ContainerRuntimeError::Invalid(
            "workspace cannot map to the volume root".into(),
        ));
    }
    Ok(WorkspaceRef {
        host_path: host_path.to_path_buf(),
        pvc_sub_path: volume_sub_path,
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
