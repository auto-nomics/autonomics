use std::sync::Arc;

use crate::config::ensure_backend_env_removed;
use crate::connection::PodmanConnection;
use crate::panel::PanelCache;
use crate::podman::{PodmanConfig, PodmanRuntime};

/// Process-wide container execution resources.
///
/// The runtime host owns one instance and injects it into the data engine so
/// every DAG session shares the same Podman connection and panel cache.
#[derive(Clone)]
pub struct ContainerExecutionInfra {
    pub runtime: Arc<dyn PodmanConnection>,
    pub panel_cache: Arc<PanelCache>,
    pub config: PodmanConfig,
}

impl Default for ContainerExecutionInfra {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ContainerExecutionInfra {
    pub fn from_config(config: PodmanConfig) -> Self {
        let runtime = Arc::new(PodmanRuntime::new(config.clone()));
        let panel_cache = PanelCache::new(config.panel_cache_root.clone());
        Self {
            runtime: runtime as Arc<dyn PodmanConnection>,
            panel_cache: Arc::new(panel_cache),
            config,
        }
    }

    pub fn from_env() -> Self {
        Self::try_from_env()
            .unwrap_or_else(|error| panic!("invalid container execution configuration: {error}"))
    }

    pub fn try_from_env() -> Result<Self, String> {
        ensure_backend_env_removed()?;
        Ok(Self::from_config(PodmanConfig::from_env()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn config_selects_podman_runtime_and_panel_cache() {
        let infra = ContainerExecutionInfra::from_config(PodmanConfig {
            program: "podman".into(),
            workspace_root: "/tmp/autonomics-podman-workspace".into(),
            panel_cache_root: "/tmp/autonomics-podman-panels".into(),
        });
        assert_eq!(infra.runtime.name(), "podman");
        assert_eq!(
            infra.runtime.workspace_root(),
            Path::new("/tmp/autonomics-podman-workspace")
        );
        assert_eq!(
            infra.config.panel_cache_root,
            Path::new("/tmp/autonomics-podman-panels")
        );
    }

    #[test]
    fn removed_backend_env_is_rejected() {
        assert!(crate::config::parse_removed_backend_env("k3s").is_err());
        assert!(crate::config::parse_removed_backend_env("podman").is_ok());
    }
}
