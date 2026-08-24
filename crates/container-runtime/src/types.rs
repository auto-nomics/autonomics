use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;

/// A reference-data or engine-managed bind mount.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize)]
pub struct ContainerMount {
    /// Absolute path as seen by the process that invokes Podman.
    pub host_path: String,
    /// Absolute path inside the workload container.
    pub container_path: String,
    /// Writable mounts are opt-in; the default is read-only.
    #[serde(default)]
    pub writable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullPolicy {
    /// Pull only when the image is absent locally.
    #[default]
    Missing,
    /// Pull before every execution.
    Always,
    /// Pull only when the registry has a newer local image.
    Newer,
    /// Never pull; fail if the image is absent.
    Never,
}

impl PullPolicy {
    pub(crate) fn as_cli_value(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Always => "always",
            Self::Newer => "newer",
            Self::Never => "never",
        }
    }
}

/// One ephemeral container invocation.
#[derive(Debug, Clone)]
pub struct ContainerRunRequest {
    pub image: String,
    /// argv executed inside the image. No shell is inserted.
    pub command: Vec<String>,
    /// Scratch directory on the Podman client host.
    pub host_workdir: PathBuf,
    /// Fixed writable bind destination for `host_workdir`.
    pub container_workdir: String,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<ContainerMount>,
    pub network: String,
    pub read_only_rootfs: bool,
    pub pull_policy: PullPolicy,
    pub cpus: Option<f64>,
    pub memory: Option<String>,
    pub pids_limit: Option<i64>,
    pub shm_size: Option<String>,
    /// Overrides the runtime's default `uid:gid`; reserved for advanced images.
    pub user: Option<String>,
    pub timeout_secs: u64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ContainerRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}
