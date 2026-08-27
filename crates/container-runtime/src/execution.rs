use std::sync::Arc;

use crate::config::ContainerBackend;
use crate::k3s::K3sConfig;
use crate::k3s::K3sRuntime;
use crate::panel::PanelCache;
use crate::podman::{PodmanConfig, PodmanRuntime};
use crate::runtime::ContainerRuntime;

#[derive(Debug, Clone)]
pub enum ContainerExecutionConfig {
    K3s(K3sConfig),
    Podman(PodmanConfig),
}

impl ContainerExecutionConfig {
    pub fn backend(&self) -> ContainerBackend {
        match self {
            Self::K3s(_) => ContainerBackend::K3s,
            Self::Podman(_) => ContainerBackend::Podman,
        }
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        match self {
            Self::K3s(config) => &config.workspace_root,
            Self::Podman(config) => &config.workspace_root,
        }
    }

    pub fn panel_cache_root(&self) -> &std::path::Path {
        match self {
            Self::K3s(config) => &config.panel_cache_root,
            Self::Podman(config) => &config.panel_cache_root,
        }
    }
}

/// Process-wide container execution resources.
///
/// The runtime host owns one instance and injects it into the data engine so
/// every DAG session shares the selected container runtime and panel cache.
#[derive(Clone)]
pub struct ContainerExecutionInfra {
    pub runtime: Arc<dyn ContainerRuntime>,
    /// K3s-only persistent development workspaces. Podman currently supports
    /// ephemeral container_command execution only.
    pub k3s: Option<Arc<K3sRuntime>>,
    pub panel_cache: Arc<PanelCache>,
    pub config: ContainerExecutionConfig,
}

impl Default for ContainerExecutionInfra {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ContainerExecutionInfra {
    pub fn from_config(config: K3sConfig) -> Self {
        let k3s = Arc::new(K3sRuntime::new(config.clone()));
        let panel_cache = PanelCache::new(
            config.panel_cache_root.clone(),
            config.panel_pvc_prefix.clone(),
        );
        Self {
            runtime: Arc::clone(&k3s) as Arc<dyn ContainerRuntime>,
            k3s: Some(k3s),
            panel_cache: Arc::new(panel_cache),
            config: ContainerExecutionConfig::K3s(config),
        }
    }

    pub fn from_env() -> Self {
        Self::try_from_env()
            .unwrap_or_else(|error| panic!("invalid container execution configuration: {error}"))
    }

    pub fn try_from_env() -> Result<Self, String> {
        match ContainerBackend::from_env()? {
            ContainerBackend::K3s => Ok(Self::from_config(K3sConfig::from_env())),
            ContainerBackend::Podman => Ok(Self::from_podman_config(PodmanConfig::from_env())),
        }
    }

    pub fn from_podman_config(config: PodmanConfig) -> Self {
        let runtime = Arc::new(PodmanRuntime::new(config.clone()));
        let panel_cache = PanelCache::new(config.panel_cache_root.clone(), String::new());
        Self {
            runtime: runtime as Arc<dyn ContainerRuntime>,
            k3s: None,
            panel_cache: Arc::new(panel_cache),
            config: ContainerExecutionConfig::Podman(config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k3s::K3sConfig;
    use crate::podman::PodmanConfig;
    use std::path::Path;

    #[test]
    fn explicit_backend_config_selects_expected_runtime() {
        let k3s = ContainerExecutionInfra::from_config(K3sConfig {
            workspace_root: "/tmp/autonomics-k3s-workspace".into(),
            panel_cache_root: "/tmp/autonomics-k3s-panels".into(),
            ..Default::default()
        });
        assert_eq!(k3s.runtime.name(), "k3s");
        assert!(k3s.k3s.is_some());

        let podman = ContainerExecutionInfra::from_podman_config(PodmanConfig {
            program: "podman".into(),
            workspace_root: "/tmp/autonomics-podman-workspace".into(),
            panel_cache_root: "/tmp/autonomics-podman-panels".into(),
        });
        assert_eq!(podman.runtime.name(), "podman");
        assert!(podman.k3s.is_none());
        assert_eq!(
            podman.config.workspace_root(),
            Path::new("/tmp/autonomics-podman-workspace")
        );
    }
}
