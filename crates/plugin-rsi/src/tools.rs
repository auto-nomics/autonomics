//! Agent tools for path-addressed plugin development.
//!
//! Tools operate on `/plugins/dev/<plugin-name>` through the mounted VFS. No
//! agent/plugin lease exists: an agent identity only determines the VFS
//! principal, while the path selects the workspace.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::{manifest::PluginManifest, node_definition::NodeDefinition};
use container_runtime::{
    ContainerNetwork, ContainerRunRequest, DEFAULT_CONTAINER_WORKDIR, GpuRequest, PodmanConnection,
    PullPolicy, trusted_workspace_ref, unique_container_name,
};
use serde::Deserialize;
use serde_json::{Value, json};
use vfs::{OpendalFileStorage, permission::VfsPrincipal};

use crate::{
    EnvironmentRegistry, Error, PluginWorkspace, Result as RsiResult, plugin::is_editable,
};

const MANIFEST_FILE: &str = "manifest.toml";
pub(crate) const MAX_AGENT_ID_BYTES: usize = 256;
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 900;

#[derive(Default)]
struct RegistryState {
    runtime: Option<Arc<dyn PodmanConnection>>,
    vfs: Option<OpendalFileStorage>,
    environments: Option<EnvironmentRegistry>,
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

    fn resolve_target(
        &self,
        principal: &VfsPrincipal,
        plugin_path: &str,
    ) -> RsiResult<PluginTarget> {
        self.lock(|state| {
            let base_vfs = state.vfs.clone().ok_or_else(|| {
                Error::Validation("plugin development VFS is not configured".into())
            })?;
            let root = state.development_root.clone().ok_or_else(|| {
                Error::Validation("plugin development root is not configured".into())
            })?;
            let normalized = OpendalFileStorage::normalize_path(plugin_path);
            let suffix = normalized
                .strip_prefix(crate::PLUGIN_DEVELOPMENT_VFS_ROOT)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .unwrap_or_default();
            let parts = suffix
                .split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            if parts.len() != 1 {
                return Err(Error::Validation(format!(
                    "plugin path must address one workspace beneath {}",
                    crate::PLUGIN_DEVELOPMENT_VFS_ROOT
                )));
            }
            let plugin_name = parts[0];
            crate::validate_plugin_name(plugin_name)?;
            let virtual_path = format!("{}/{}", crate::PLUGIN_DEVELOPMENT_VFS_ROOT, plugin_name);
            let vfs = base_vfs.with_principal(principal.clone());
            if !vfs.is_mounted(&virtual_path) {
                return Err(Error::Validation(format!(
                    "plugin path `{virtual_path}` is outside the development mount"
                )));
            }
            Ok(PluginTarget {
                plugin_name: plugin_name.to_string(),
                virtual_path,
                vfs,
                workspace: PluginWorkspace::new(root.join(plugin_name)),
            })
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

    fn manifest_lock(&self, plugin_name: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.lock(|state| {
            Ok(state
                .manifest_locks
                .entry(plugin_name.to_string())
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
        ToolRegistration::from(PluginEnvironmentsListTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginNodeCreateTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginNodeUpdateDocTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginContainerRunTool { state }),
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

fn tool_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: error.to_string().into(),
    }
}

fn vfs_error(error: impl std::fmt::Display) -> Error {
    Error::Validation(format!("VFS operation failed: {error}"))
}

fn manifest_path(target: &PluginTarget) -> String {
    format!(
        "{}/{}",
        target.virtual_path.trim_end_matches('/'),
        MANIFEST_FILE
    )
}

async fn resolve_target(state: &PluginToolState, plugin_path: &str) -> RsiResult<PluginTarget> {
    state.registry.resolve_target(&state.principal, plugin_path)
}

async fn read_virtual_text(vfs: &OpendalFileStorage, path: &str) -> RsiResult<String> {
    let length = vfs.content_length(path).await.map_err(vfs_error)?;
    let bytes = vfs
        .read_range(path, 0..length)
        .await
        .map_err(vfs_error)?
        .to_vec();
    String::from_utf8(bytes)
        .map_err(|error| Error::Validation(format!("VFS file `{path}` is not UTF-8: {error}")))
}

async fn load_manifest(target: &PluginTarget) -> RsiResult<PluginManifest> {
    let text = read_virtual_text(&target.vfs, &manifest_path(target)).await?;
    let manifest: PluginManifest = toml::from_str(&text)
        .map_err(|error| Error::Validation(format!("invalid {MANIFEST_FILE}: {error}")))?;
    if manifest.plugin_name != target.plugin_name {
        return Err(Error::Validation(format!(
            "workspace plugin_name `{}` does not match path plugin `{}`",
            manifest.plugin_name, target.plugin_name
        )));
    }
    Ok(manifest)
}

async fn editable_manifest(target: &PluginTarget) -> RsiResult<PluginManifest> {
    let manifest = load_manifest(target).await?;
    if !is_editable(manifest.status) {
        return Err(Error::Validation(format!(
            "plugin `{}` cannot be edited from manifest status {:?}",
            target.plugin_name, manifest.status
        )));
    }
    Ok(manifest)
}

fn save_manifest(workspace: &PluginWorkspace, manifest: &PluginManifest) -> RsiResult<()> {
    toml::to_string_pretty(manifest)
        .map_err(|error| Error::Validation(format!("cannot encode manifest: {error}")))
        .and_then(|text| workspace.write_text(MANIFEST_FILE, &text))
}

fn selected_node(manifest: &PluginManifest, node_kind: &str) -> RsiResult<NodeDefinition> {
    manifest
        .nodes
        .iter()
        .find(|node| node.kind == node_kind)
        .cloned()
        .ok_or_else(|| {
            Error::Validation(format!(
                "selected node `{}` is missing from the plugin manifest",
                node_kind
            ))
        })
}

#[tool(
    name = "plugin_environments_list",
    description = "List host-approved digest-pinned base images available to plugin development."
)]
struct PluginEnvironmentsListInput {}

struct PluginEnvironmentsListTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginEnvironmentsListTool {
    type Input = PluginEnvironmentsListInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let environments = self.state.registry.environments().map_err(tool_error)?;
        let environments = environments
            .list()
            .into_iter()
            .map(|(id, environment)| {
                json!({
                    "id": id,
                    "reference": environment.reference,
                    "interpreters": environment.interpreters,
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::success_json(Value::Array(environments)))
    }
}

#[tool(
    name = "plugin_node_create",
    description = "Add one complete node definition to a plugin VFS workspace. Create its script separately before review."
)]
struct PluginNodeCreateInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Complete NodeDefinition value in container-plugin JSON form.
    node: Value,
}

struct PluginNodeCreateTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeCreateTool {
    type Input = PluginNodeCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let node: NodeDefinition =
            serde_json::from_value(input.node).map_err(|error| ToolError::ValidationFailed {
                message: format!("invalid node definition: {error}"),
            })?;
        container_plugin::node_definition::validate(&node).map_err(|error| {
            ToolError::ValidationFailed {
                message: format!("invalid node definition: {error}"),
            }
        })?;

        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&target.plugin_name);
        let _guard = manifest_lock.lock().await;
        let mut manifest = editable_manifest(&target).await.map_err(tool_error)?;
        if manifest
            .nodes
            .iter()
            .any(|existing| existing.kind == node.kind)
        {
            return Err(ToolError::ValidationFailed {
                message: format!("node kind `{}` already exists", node.kind),
            });
        }
        manifest.nodes.push(node);
        save_manifest(&target.workspace, &manifest).map_err(tool_error)?;
        Ok(ToolResult::success("created node"))
    }
}

#[tool(
    name = "plugin_node_update_doc",
    description = "Replace one plugin node's documentation without changing its structural spec."
)]
struct PluginNodeUpdateDocInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Kind of the node to update.
    node_kind: String,
    /// Complete replacement documentation. Markdown formatting is allowed.
    doc: String,
}

struct PluginNodeUpdateDocTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeUpdateDocTool {
    type Input = PluginNodeUpdateDocInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.doc.trim().is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "node documentation cannot be empty".into(),
            });
        }
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest_lock = self.state.registry.manifest_lock(&target.plugin_name);
        let _guard = manifest_lock.lock().await;
        let mut manifest = editable_manifest(&target).await.map_err(tool_error)?;
        let mut node = selected_node(&manifest, &input.node_kind).map_err(tool_error)?;
        node.doc = input.doc;
        let index = manifest
            .nodes
            .iter()
            .position(|existing| existing.kind == node.kind)
            .expect("selected node position");
        manifest.nodes[index] = node;
        save_manifest(&target.workspace, &manifest).map_err(tool_error)?;
        Ok(ToolResult::success("updated node documentation"))
    }
}

#[tool(
    name = "plugin_container_run",
    description = "Run one argv command in the plugin environment with the addressed VFS workspace mounted at /work."
)]
struct PluginContainerRunInput {
    /// Plugin workspace path, such as `/plugins/dev/hello-world`.
    plugin_path: String,
    /// Executable and arguments. No shell string is accepted.
    argv: Vec<String>,
    /// Timeout in seconds; defaults to 120 and is capped at 600.
    timeout_secs: Option<u64>,
}

struct PluginContainerRunTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginContainerRunTool {
    type Input = PluginContainerRunInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.argv.is_empty() || input.argv.iter().any(|arg| arg.contains('\0')) {
            return Err(ToolError::ValidationFailed {
                message: "argv must be a nonempty NUL-free array".into(),
            });
        }
        let timeout_secs = input.timeout_secs.unwrap_or(120).clamp(1, 600);
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        let manifest = load_manifest(&target).await.map_err(tool_error)?;
        let runtime = self.state.registry.runtime().map_err(tool_error)?;
        let workspace_ref =
            trusted_workspace_ref(target.workspace.path(), DEFAULT_CONTAINER_WORKDIR)
                .map_err(tool_error)?;
        let request = ContainerRunRequest {
            image: manifest.image.reference.as_str().to_string(),
            command: input.argv,
            workspace: workspace_ref,
            env: vec![
                ("AUTONOMICS_PLUGIN_NAME".into(), target.plugin_name.clone()),
                (
                    "AUTONOMICS_PLUGIN_VFS_PATH".into(),
                    target.virtual_path.clone(),
                ),
            ],
            panels: Vec::new(),
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: Some(2.0),
            memory: Some("1g".into()),
            pids_limit: Some(256),
            shm_size: Some("256m".into()),
            gpus: GpuRequest::None,
            user: None,
            timeout_secs,
            name: unique_container_name(),
        };
        let output = runtime.run(request).await.map_err(tool_error)?;
        Ok(ToolResult::success_json(json!({
            "exit_code": output.exit_code,
            "stdout": output.stdout,
            "stderr": output.stderr,
        })))
    }

    fn timeout_seconds(&self) -> u64 {
        DEFAULT_TOOL_TIMEOUT_SECS
    }
}
