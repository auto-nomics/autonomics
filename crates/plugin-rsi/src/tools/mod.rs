//! Agent tools for path-addressed plugin development.
//!
//! Tools operate on `/plugins/dev/<plugin-name>` through the mounted VFS. No
//! agent/plugin lease exists: an agent identity only determines the VFS
//! principal, while the path selects the workspace.

mod container_run;
mod env;
mod environment_bind;
mod environments_list;
mod fork;
mod helpers;
mod install;
mod manifest;
mod node_create;
mod node_update;
mod node_update_doc;
mod uninstall;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use agentik_core::tools::ToolRegistration;
use container_runtime::PodmanConnection;
use vfs::{OpendalFileStorage, permission::VfsPrincipal};

use crate::{EnvironmentDevInfra, EnvironmentRegistry, Error, PluginWorkspace, Result as RsiResult, RsiInfra};

pub(crate) const MAX_AGENT_ID_BYTES: usize = 256;

pub use env::environment_development_tool_registrations;

#[derive(Default)]
struct RegistryState {
    runtime: Option<Arc<dyn PodmanConnection>>,
    vfs: Option<OpendalFileStorage>,
    environments: Option<EnvironmentRegistry>,
    infra: Option<Arc<RsiInfra>>,
    environment_dev: Option<Arc<EnvironmentDevInfra>>,
    development_root: Option<PathBuf>,
    manifest_locks: BTreeMap<String, Arc<tokio::sync::Mutex<()>>>,
}

/// Process-wide registry carrying trusted plugin-development infrastructure.
#[derive(Default)]
pub struct PluginDevelopmentToolsetRegistry {
    state: Mutex<RegistryState>,
}

static REGISTRY: OnceLock<Arc<PluginDevelopmentToolsetRegistry>> = OnceLock::new();

impl PluginDevelopmentToolsetRegistry {
    pub fn global() -> Arc<Self> {
        REGISTRY.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub fn configure_runtime(&self, runtime: Arc<dyn PodmanConnection>) -> RsiResult<()> {
        self.lock(|state| {
            state.runtime = Some(runtime);
            Ok(())
        })
    }

    /// Configure the mounted development namespace used by all plugin tools.
    pub fn configure_vfs(
        &self,
        vfs: OpendalFileStorage,
        development_root: impl Into<PathBuf>,
    ) -> RsiResult<()> {
        let development_root = development_root.into();
        self.lock(|state| {
            state.vfs = Some(vfs);
            state.development_root = Some(development_root);
            Ok(())
        })
    }

    /// Configure the read-only environment catalog exposed to plugin agents.
    pub fn configure_environments(&self, environments: EnvironmentRegistry) -> RsiResult<()> {
        self.lock(|state| {
            state.environments = Some(environments);
            Ok(())
        })
    }

    /// Configure trusted lifecycle orchestration exposed by plugin tools.
    pub fn configure_infra(&self, infra: Arc<RsiInfra>) -> RsiResult<()> {
        self.lock(|state| {
            state.infra = Some(infra);
            Ok(())
        })
    }

    /// Configure trusted environment-development orchestration.
    ///
    /// The infra's store root doubles as the `/environments/dev` backing
    /// directory, so environment tools resolve their VFS targets through it.
    pub fn configure_environment_dev(&self, infra: Arc<EnvironmentDevInfra>) -> RsiResult<()> {
        self.lock(|state| {
            state.environment_dev = Some(infra);
            Ok(())
        })
    }

    fn resolve_target(
        &self,
        principal: &VfsPrincipal,
        plugin_path: &str,
    ) -> RsiResult<PluginTarget> {
        let (root, base_vfs) = self.lock(|state| {
            let root = state
                .development_root
                .clone()
                .ok_or_else(|| Error::Validation("plugin development root is not configured".into()))?;
            let vfs = state
                .vfs
                .clone()
                .ok_or_else(|| Error::Validation("plugin development VFS is not configured".into()))?;
            Ok((root, vfs))
        })?;
        let (plugin_name, virtual_path, vfs) = resolve_mounted_workspace(
            principal,
            plugin_path,
            crate::PLUGIN_DEVELOPMENT_VFS_ROOT,
            base_vfs,
        )?;
        Ok(PluginTarget {
            workspace: PluginWorkspace::new(root.join(&plugin_name)),
            plugin_name,
            virtual_path,
            vfs,
        })
    }

    fn resolve_environment_target(
        &self,
        principal: &VfsPrincipal,
        environment_path: &str,
    ) -> RsiResult<crate::tools::env::EnvironmentTarget> {
        let (root, base_vfs) = self.lock(|state| {
            let infra = state.environment_dev.clone().ok_or_else(|| {
                Error::Validation(
                    "environment development infrastructure is not configured".into(),
                )
            })?;
            let vfs = state
                .vfs
                .clone()
                .ok_or_else(|| Error::Validation("plugin development VFS is not configured".into()))?;
            Ok((infra.store().root().to_path_buf(), vfs))
        })?;
        let (environment_id, virtual_path, _vfs) = resolve_mounted_workspace(
            principal,
            environment_path,
            crate::ENVIRONMENT_DEVELOPMENT_VFS_ROOT,
            base_vfs,
        )?;
        Ok(crate::tools::env::EnvironmentTarget {
            workspace: PluginWorkspace::new(root.join(&environment_id)),
            environment_id,
            virtual_path,
        })
    }

    fn runtime(&self) -> RsiResult<Arc<dyn PodmanConnection>> {
        self.lock(|state| {
            state.runtime.clone().ok_or_else(|| {
                Error::Validation("plugin development container runtime is not configured".into())
            })
        })
    }

    fn environments(&self) -> RsiResult<crate::EnvironmentCatalog> {
        self.lock(|state| {
            state
                .environments
                .as_ref()
                .map(|registry| registry.snapshot())
                .ok_or_else(|| Error::Validation("plugin environments are not configured".into()))
        })
    }

    fn infra(&self) -> RsiResult<Arc<RsiInfra>> {
        self.lock(|state| {
            state.infra.clone().ok_or_else(|| {
                Error::Validation("plugin lifecycle infrastructure is not configured".into())
            })
        })
    }

    fn environment_infra(&self) -> RsiResult<Arc<EnvironmentDevInfra>> {
        self.lock(|state| {
            state.environment_dev.clone().ok_or_else(|| {
                Error::Validation(
                    "environment development infrastructure is not configured".into(),
                )
            })
        })
    }

    fn manifest_lock(&self, workspace_key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.lock(|state| {
            Ok(state
                .manifest_locks
                .entry(workspace_key.to_string())
                .or_default()
                .clone())
        })
        .expect("plugin development registry lock poisoned")
    }

    fn lock<T>(&self, operate: impl FnOnce(&mut RegistryState) -> RsiResult<T>) -> RsiResult<T> {
        let mut state = self.state.lock().map_err(|_| {
            Error::Validation("plugin development toolset registry lock poisoned".into())
        })?;
        operate(&mut state)
    }
}

/// Resolve one `/<root>/<name>` VFS address against a mounted storage.
///
/// The name must be a single kebab-case segment addressing a workspace whose
/// mount is visible to this principal.
fn resolve_mounted_workspace(
    principal: &VfsPrincipal,
    path: &str,
    vfs_root: &str,
    base_vfs: OpendalFileStorage,
) -> RsiResult<(String, String, OpendalFileStorage)> {
    let normalized = OpendalFileStorage::normalize_path(path);
    let suffix = normalized
        .strip_prefix(vfs_root)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .unwrap_or_default();
    let parts = suffix
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 1 {
        return Err(Error::Validation(format!(
            "path must address one workspace beneath {vfs_root}"
        )));
    }
    let name = parts[0];
    crate::validate_plugin_name(name)?;
    let virtual_path = format!("{vfs_root}/{name}");
    let vfs = base_vfs.with_principal(principal.clone());
    if !vfs.is_mounted(&virtual_path) {
        return Err(Error::Validation(format!(
            "path `{virtual_path}` is outside the development mount"
        )));
    }
    Ok((name.to_string(), virtual_path, vfs))
}

#[derive(Clone)]
struct PluginToolState {
    registry: Arc<PluginDevelopmentToolsetRegistry>,
    principal: VfsPrincipal,
}

#[derive(Clone)]
struct PluginTarget {
    plugin_name: String,
    virtual_path: String,
    vfs: OpendalFileStorage,
    workspace: PluginWorkspace,
}

/// Build the specialized plugin development tools for one agent identity.
pub fn plugin_development_tool_registrations(agent_id: impl Into<String>) -> Vec<ToolRegistration> {
    let agent_id = agent_id.into();
    validate_agent_id(&agent_id).expect("valid plugin development agent id");
    let state = PluginToolState {
        registry: PluginDevelopmentToolsetRegistry::global(),
        principal: principal_for_agent(&agent_id),
    };
    vec![
        ToolRegistration::from(environments_list::PluginEnvironmentsListTool {
            state: state.clone(),
        }),
        ToolRegistration::from(environment_bind::PluginEnvironmentBindTool {
            state: state.clone(),
        }),
        ToolRegistration::from(fork::PluginForkTool {
            state: state.clone(),
        }),
        ToolRegistration::from(install::PluginInstallTool {
            state: state.clone(),
        }),
        ToolRegistration::from(uninstall::PluginUninstallTool {
            state: state.clone(),
        }),
        ToolRegistration::from(node_create::PluginNodeCreateTool {
            state: state.clone(),
        }),
        ToolRegistration::from(node_update::PluginNodeUpdateTool {
            state: state.clone(),
        }),
        ToolRegistration::from(node_update_doc::PluginNodeUpdateDocTool {
            state: state.clone(),
        }),
        ToolRegistration::from(container_run::PluginContainerRunTool { state }),
    ]
}

fn validate_agent_id(agent_id: &str) -> RsiResult<()> {
    if agent_id.is_empty()
        || agent_id.len() > MAX_AGENT_ID_BYTES
        || agent_id.chars().any(|character| character.is_control())
    {
        return Err(Error::Validation(
            "agent id must be nonempty, bounded, and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn principal_for_agent(agent_id: &str) -> VfsPrincipal {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in agent_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let uid = 10_000_u32 + u32::try_from(hash % 55_536).unwrap_or_default();
    VfsPrincipal::plugin_developer(uid)
}
