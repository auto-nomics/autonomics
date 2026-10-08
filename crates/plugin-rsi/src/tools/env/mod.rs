//! Agent tools for path-addressed environment (image) development.
//!
//! Tools operate on `/environments/dev/<environment-id>` through the mounted
//! VFS, mirroring the plugin tools. Agents never execute git, Podman, or
//! registry operations directly; every mutation flows through
//! [`crate::EnvironmentDevInfra`].

mod container_run;
mod create;
mod manifest_update;
mod uninstall;
mod validate;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use vfs::permission::VfsPrincipal;

use crate::{Error, PluginWorkspace, Result as RsiResult};

use super::PluginDevelopmentToolsetRegistry;

/// One resolved `/environments/dev/<id>` address.
///
/// The VFS instance is consumed by the mount check during resolution; env
/// tools read workspace content through the host-owned [`PluginWorkspace`].
#[derive(Clone)]
pub(super) struct EnvironmentTarget {
    pub(super) environment_id: String,
    pub(super) virtual_path: String,
    pub(super) workspace: PluginWorkspace,
}

#[derive(Clone)]
pub(super) struct EnvToolState {
    pub(super) registry: Arc<PluginDevelopmentToolsetRegistry>,
    pub(super) principal: VfsPrincipal,
}

/// Build the specialized environment development tools for one agent identity.
pub fn environment_development_tool_registrations(
    agent_id: impl Into<String>,
) -> Vec<ToolRegistration> {
    let agent_id = agent_id.into();
    super::validate_agent_id(&agent_id).expect("valid environment development agent id");
    let state = EnvToolState {
        registry: PluginDevelopmentToolsetRegistry::global(),
        principal: super::principal_for_agent(&agent_id),
    };
    vec![
        ToolRegistration::from(create::EnvironmentCreateTool {
            state: state.clone(),
        }),
        ToolRegistration::from(create::EnvironmentForkTool {
            state: state.clone(),
        }),
        ToolRegistration::from(manifest_update::EnvironmentManifestUpdateTool {
            state: state.clone(),
        }),
        ToolRegistration::from(validate::EnvironmentValidateTool {
            state: state.clone(),
        }),
        ToolRegistration::from(validate::EnvironmentBuildTool {
            state: state.clone(),
        }),
        ToolRegistration::from(validate::EnvironmentInstallTool {
            state: state.clone(),
        }),
        ToolRegistration::from(uninstall::EnvironmentUninstallTool {
            state: state.clone(),
        }),
        ToolRegistration::from(container_run::EnvironmentContainerRunTool { state }),
    ]
}

pub(super) fn tool_error(error: impl std::fmt::Display) -> agentik_core::tools::ToolError {
    agentik_core::tools::ToolError::ExecutionFailed {
        source: error.to_string().into(),
    }
}

pub(super) async fn resolve_target(
    state: &EnvToolState,
    environment_path: &str,
) -> RsiResult<EnvironmentTarget> {
    state
        .registry
        .resolve_environment_target(&state.principal, environment_path)
}

pub(super) fn environment_manifest_lock(
    registry: &PluginDevelopmentToolsetRegistry,
    environment_id: &str,
) -> Arc<tokio::sync::Mutex<()>> {
    registry.manifest_lock(&format!("environment/{environment_id}"))
}

pub(super) fn ensure_editable(manifest: &crate::EnvironmentManifest) -> RsiResult<()> {
    if crate::env_manifest::environment_is_editable(manifest.status) {
        Ok(())
    } else {
        Err(Error::Validation(format!(
            "environment `{}` cannot be edited from manifest status {:?}",
            manifest.environment_id, manifest.status
        )))
    }
}
