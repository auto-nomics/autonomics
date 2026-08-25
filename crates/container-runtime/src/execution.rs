use std::sync::Arc;

use crate::{K3sConfig, K3sRuntime, PanelCache};

/// Process-wide container execution resources.
///
/// The runtime host owns one instance and injects it into the data engine so
/// every DAG session shares the same Kubernetes client and panel cache.
#[derive(Clone)]
pub struct ContainerExecutionInfra {
    pub k3s: Arc<K3sRuntime>,
    pub panel_cache: Arc<PanelCache>,
    pub config: K3sConfig,
}

impl Default for ContainerExecutionInfra {
    fn default() -> Self {
        Self::from_config(K3sConfig::from_env())
    }
}

impl ContainerExecutionInfra {
    pub fn from_config(config: K3sConfig) -> Self {
        let panel_cache = PanelCache::new(
            config.panel_cache_root.clone(),
            config.panel_pvc_prefix.clone(),
        );
        Self {
            k3s: Arc::new(K3sRuntime::new(config.clone())),
            panel_cache: Arc::new(panel_cache),
            config,
        }
    }

    pub fn from_env() -> Self {
        Self::from_config(K3sConfig::from_env())
    }
}
