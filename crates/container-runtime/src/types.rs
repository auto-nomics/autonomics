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

/// A panel materialized into the shared panel cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPanel {
    pub id: String,
    pub digest: String,
    pub host_path: PathBuf,
    pub mount_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRef {
    pub host_path: PathBuf,
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

/// Network profile of an ephemeral container.
///
/// `Isolated` runs with no network devices at all (`podman --network none`).
/// `Egress` uses the host's default container networking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContainerNetwork {
    #[default]
    Isolated,
    Egress,
}

impl ContainerNetwork {
    /// Parse a spec-level network name. `none` is accepted as a legacy
    /// spelling of `isolated`; `cluster` was removed together with the k3s
    /// backend and is rejected explicitly.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "isolated" | "none" => Ok(Self::Isolated),
            "egress" => Ok(Self::Egress),
            "cluster" => Err(
                "network profile `cluster` was removed with the k3s backend; use `isolated` or `egress`"
                    .into(),
            ),
            other => Err(format!(
                "network must be `isolated` or `egress`; got `{other}`"
            )),
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
    pub network: ContainerNetwork,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_none_parses_as_isolated_and_cluster_is_rejected() {
        assert_eq!(
            ContainerNetwork::parse("isolated").unwrap(),
            ContainerNetwork::Isolated
        );
        assert_eq!(
            ContainerNetwork::parse(" none ").unwrap(),
            ContainerNetwork::Isolated
        );
        assert_eq!(
            ContainerNetwork::parse("egress").unwrap(),
            ContainerNetwork::Egress
        );
        let cluster = ContainerNetwork::parse("cluster").unwrap_err();
        assert!(cluster.contains("removed with the k3s backend"));
        assert!(ContainerNetwork::parse("host").is_err());
    }
}
