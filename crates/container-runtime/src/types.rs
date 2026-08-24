use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An immutable reference-data bundle stored in the authoritative object VFS.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct PanelRef {
    pub id: String,
    pub digest: String,
    pub source: String,
    pub mount_path: String,
}

/// A panel materialized into the shared k3s panel cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPanel {
    pub id: String,
    pub digest: String,
    pub host_path: PathBuf,
    pub pvc_sub_path: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRef {
    pub host_path: PathBuf,
    pub pvc_sub_path: String,
    pub container_workdir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullPolicy {
    #[default]
    Missing,
    Always,
    Newer,
    Never,
}

impl PullPolicy {
    pub fn as_kubernetes_value(self) -> &'static str {
        match self {
            Self::Missing => "IfNotPresent",
            Self::Always | Self::Newer => "Always",
            Self::Never => "Never",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContainerRunRequest {
    pub image: String,
    pub command: Vec<String>,
    pub workspace: WorkspaceRef,
    pub env: Vec<(String, String)>,
    pub panels: Vec<CachedPanel>,
    pub network: String,
    pub read_only_rootfs: bool,
    pub pull_policy: PullPolicy,
    pub cpus: Option<f64>,
    pub memory: Option<String>,
    pub pids_limit: Option<i64>,
    pub shm_size: Option<String>,
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

impl PanelRef {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("panel `id` cannot be empty".into());
        }
        if !self.digest.starts_with("sha256:") || self.digest.len() != 71 {
            return Err(format!("panel `{}` has invalid digest", self.id));
        }
        if self.source.contains('\0') || self.source.trim() == "/" {
            return Err(format!("panel `{}` has invalid source", self.id));
        }
        if !Path::new(&self.mount_path).is_absolute() || self.mount_path == "/" {
            return Err(format!("panel `{}` has invalid mount path", self.id));
        }
        Ok(())
    }
}
