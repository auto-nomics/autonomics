use std::sync::Arc;

use crate::config::ensure_backend_env_removed;
use crate::connection::PodmanConnection;
use crate::panel::PanelCache;
use crate::podman::{PodmanConfig, PodmanRuntime, PrivateStorageDriver};

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
    /// Explicit cluster-worker backend. This never changes daemon or formal
    /// worker backend selection; formal Podman admission remains separate.
    pub fn for_apptainer_worker(
        state_dir: &std::path::Path,
        catalog_path: &std::path::Path,
        executable: impl Into<std::path::PathBuf>,
    ) -> Result<Self, String> {
        let mut config = PodmanConfig::for_state_dir(state_dir);
        let runtime = crate::ApptainerRuntime::from_catalog_file(
            config.workspace_root.clone(),
            executable,
            catalog_path,
        )
        .map_err(|error| error.to_string())?;
        if let Some(root) = runtime.panel_cache_root() {
            config.panel_cache_root = root.to_path_buf();
        }
        let panel_cache = Arc::new(PanelCache::content_required(
            config.panel_cache_root.clone(),
        ));
        Ok(Self {
            runtime: Arc::new(runtime),
            panel_cache,
            config,
        })
    }

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

    /// One operator-only backend selection shared by daemon, readiness and
    /// software preparation. Agent input cannot choose a host/default store.
    /// Resolution order: `AUTONOMICS_CONTAINER_STORAGE_DRIVER` env, then the
    /// operator's persisted marker `<state_dir>/container-storage-driver`
    /// (write it once when adopting overlay — launching without either
    /// falls back to vfs and would silently build a second store), then
    /// the legacy vfs default.
    pub fn for_operator_state_dir(state_dir: &std::path::Path) -> Result<Self, String> {
        let setting = match std::env::var_os("AUTONOMICS_CONTAINER_STORAGE_DRIVER") {
            Some(value) => Some(
                value
                    .into_string()
                    .map_err(|_| "AUTONOMICS_CONTAINER_STORAGE_DRIVER must be valid UTF-8")?,
            ),
            None => std::fs::read_to_string(state_dir.join("container-storage-driver"))
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty()),
        };
        let driver = PrivateStorageDriver::parse_operator_setting(setting.as_deref())?;
        Self::for_state_dir_with_storage_driver(state_dir, driver)
    }

    pub fn for_state_dir(state_dir: &std::path::Path) -> Result<Self, String> {
        Self::for_state_dir_with_storage_driver(state_dir, PrivateStorageDriver::Vfs)
    }

    pub fn for_state_dir_with_storage_driver(
        state_dir: &std::path::Path,
        driver: PrivateStorageDriver,
    ) -> Result<Self, String> {
        ensure_backend_env_removed()?;
        Ok(Self::from_config(
            PodmanConfig::for_state_dir_with_storage_driver(state_dir, driver),
        ))
    }

    /// Keep native work, conmon and OCI children below the launcher's one
    /// delegated resource cap. The launcher moves its initial process into
    /// worker-native, leaving the delegated parent free of internal tasks.
    pub fn for_formal_worker_state_dir(state_dir: &std::path::Path) -> Result<Self, String> {
        Self::for_formal_worker_state_dir_with_storage_driver(state_dir, PrivateStorageDriver::Vfs)
    }

    pub fn for_formal_worker_state_dir_with_storage_driver(
        state_dir: &std::path::Path,
        driver: PrivateStorageDriver,
    ) -> Result<Self, String> {
        ensure_backend_env_removed()?;
        let mut config = PodmanConfig::for_state_dir_with_storage_driver(state_dir, driver);
        let text = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
        let parent = text
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .and_then(|path| path.strip_suffix("/worker-native"))
            .filter(|path| {
                path.starts_with('/') && !path.split('/').any(|p| matches!(p, "." | ".."))
            })
            .ok_or("formal worker requires a delegated worker-native cgroup")?;
        config.cgroup_manager = Some("cgroupfs".into());
        config.cgroup_parent = Some(parent.into());
        // keep-id consults the subordinate host UID's /etc/subuid allocation
        // and cannot remap container ID0 here. The launcher has already
        // established and verified a protected user namespace; reuse it.
        config.inherit_user_namespace = true;
        // The launcher also caps aggregate swap at zero, but every OCI leaf
        // must explicitly retain that policy instead of Podman's RAM-sized
        // default swap allowance. No agent/environment can switch this off.
        config.disable_swap = true;
        let mut infra = Self::from_config(config);
        infra.panel_cache = Arc::new(PanelCache::content_required(
            infra.config.panel_cache_root.clone(),
        ));
        Ok(infra)
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
            storage_root: None,
            run_root: None,
            storage_driver: None,
            cgroup_manager: None,
            cgroup_parent: None,
            inherit_user_namespace: false,
            disable_swap: false,
            worker_runtime_dir: None,
            worker_resource_control: None,
            worker_user_namespace: None,
            worker_oci_runtime: None,
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

    #[test]
    fn operator_storage_choice_preserves_legacy_and_rejects_invalid_settings() {
        assert_eq!(
            PrivateStorageDriver::parse_operator_setting(None).unwrap(),
            PrivateStorageDriver::Vfs
        );
        for invalid in ["", "auto", "Overlay", " /tmp/shared", "btrfs"] {
            assert!(PrivateStorageDriver::parse_operator_setting(Some(invalid)).is_err());
        }
        let state = Path::new("/tmp/operator-runtime-state");
        let vfs = PodmanConfig::for_state_dir_with_storage_driver(
            state,
            PrivateStorageDriver::parse_operator_setting(Some("vfs")).unwrap(),
        );
        let overlay = PodmanConfig::for_state_dir_with_storage_driver(
            state,
            PrivateStorageDriver::parse_operator_setting(Some("overlay")).unwrap(),
        );
        assert_eq!(vfs.storage_root.unwrap(), state.join("containers/storage"));
        assert_eq!(
            overlay.storage_root.unwrap(),
            state.join("containers-overlay/storage")
        );
        assert_eq!(
            overlay.run_root.unwrap(),
            state.join("containers-overlay/run")
        );
        assert_eq!(
            overlay.workspace_root,
            state.join("containers-overlay/workspace")
        );
        // Panels stay on the shared catalog root — one local copy mounted
        // directly by containers, never duplicated per store layout.
        assert_eq!(
            overlay.panel_cache_root,
            crate::config::default_panel_cache_root()
        );
    }
}
