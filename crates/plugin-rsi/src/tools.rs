//! Agent tools for specialized plugin development.
//!
//! The registry is the process-wide trust boundary. The host binds one agent
//! identity to one proposal, plugin, and detached candidate run; tool instances
//! carry only that identity and resolve everything else through the registry.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use container_plugin::{manifest::PluginManifest, node_definition::NodeDefinition};
use container_runtime::{
    ContainerNetwork, ContainerRunRequest, ContainerRunResult, DEFAULT_CONTAINER_WORKDIR,
    GpuRequest, PodmanConnection, PullPolicy, unique_container_name, workspace_ref,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Error, PluginDevelopment, ProposalWorkspace, Result as RsiResult};

const MANIFEST_FILE: &str = "manifest.toml";
const MAX_AGENT_ID_BYTES: usize = 256;
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 900;

/// One agent's exclusive assignment to develop a plugin candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDevelopmentBinding {
    /// Persistent proposal identifier.
    pub proposal_id: String,
    /// Immutable plugin name recorded by the proposal.
    pub plugin_name: String,
    /// The detached candidate run created for this assignment.
    pub run_id: String,
    /// Approved environment image selected by the proposal.
    pub environment_reference: String,
    candidate_workspace: PathBuf,
}

impl PluginDevelopmentBinding {
    /// Return the detached workspace owned by this assignment.
    pub fn workspace(&self) -> ProposalWorkspace {
        ProposalWorkspace::new(self.candidate_workspace.clone())
    }
}

#[derive(Default)]
struct RegistryState {
    runtime: Option<Arc<dyn PodmanConnection>>,
    bindings: BTreeMap<String, PluginDevelopmentBinding>,
    reserved: BTreeSet<String>,
}

/// Process-wide registry binding development agents to plugin candidates.
///
/// An agent id can have at most one active binding. This is deliberately a
/// singleton because all copies of the toolset must observe the same lease and
/// container connection.
#[derive(Default)]
pub struct PluginDevelopmentToolsetRegistry {
    state: Mutex<RegistryState>,
}

static REGISTRY: OnceLock<Arc<PluginDevelopmentToolsetRegistry>> = OnceLock::new();

impl PluginDevelopmentToolsetRegistry {
    /// Return the process-wide registry used by plugin development tools.
    pub fn global() -> Arc<Self> {
        REGISTRY.get_or_init(|| Arc::new(Self::default())).clone()
    }

    /// Configure the trusted runtime used by bound development agents.
    pub fn configure_runtime(&self, runtime: Arc<dyn PodmanConnection>) -> RsiResult<()> {
        self.lock(|state| {
            state.runtime = Some(runtime);
            Ok(())
        })
    }

    /// Bind one agent exclusively to a plugin and create its candidate run.
    ///
    /// The assignment always operates on the detached candidate created here;
    /// adoption remains a host-side `PluginDevelopment` operation.
    pub fn bind_plugin_agent(
        &self,
        agent_id: &str,
        development: &mut PluginDevelopment<'_>,
        run_id: &str,
    ) -> RsiResult<PluginDevelopmentBinding> {
        validate_agent_id(agent_id)?;
        let Some(environment_reference) = development.proposal().environment_reference.clone()
        else {
            return Err(Error::Validation(
                "proposal must bind an environment before plugin development".into(),
            ));
        };
        self.reserve_agent(agent_id)?;
        let candidate = match development.prepare_development_candidate(run_id) {
            Ok(candidate) => candidate,
            Err(error) => {
                let _ = self.release_reservation(agent_id);
                return Err(error);
            }
        };
        let binding = PluginDevelopmentBinding {
            proposal_id: development.id().to_string(),
            plugin_name: development.proposal().plugin_name.clone(),
            run_id: run_id.to_string(),
            environment_reference,
            candidate_workspace: candidate.path().to_path_buf(),
        };
        let binding = match self.lock(|state| {
            if state.bindings.contains_key(agent_id) {
                return Err(Error::Validation(format!(
                    "agent `{agent_id}` is already bound to a plugin development task"
                )));
            }
            state.bindings.insert(agent_id.to_string(), binding.clone());
            state.reserved.remove(agent_id);
            Ok(binding)
        }) {
            Ok(binding) => binding,
            Err(error) => {
                let _ = self.release_reservation(agent_id);
                return Err(error);
            }
        };
        Ok(binding)
    }

    /// Return one agent's active assignment, if any.
    pub fn binding(&self, agent_id: &str) -> RsiResult<Option<PluginDevelopmentBinding>> {
        self.lock(|state| Ok(state.bindings.get(agent_id).cloned()))
    }

    /// Release an agent assignment without touching its candidate directory.
    pub fn unbind_agent(&self, agent_id: &str) -> RsiResult<()> {
        self.lock(|state| {
            state.bindings.remove(agent_id);
            Ok(())
        })
    }

    fn resolve(
        &self,
        agent_id: &str,
    ) -> RsiResult<(PluginDevelopmentBinding, Arc<dyn PodmanConnection>)> {
        self.lock(|state| {
            let binding = state.bindings.get(agent_id).cloned().ok_or_else(|| {
                Error::Validation(format!(
                    "agent `{agent_id}` has no plugin development binding"
                ))
            })?;
            let runtime = state.runtime.clone().ok_or_else(|| {
                Error::Validation("plugin development container runtime is not configured".into())
            })?;
            Ok((binding, runtime))
        })
    }

    fn lock<T>(&self, operate: impl FnOnce(&mut RegistryState) -> RsiResult<T>) -> RsiResult<T> {
        let mut state = self.state.lock().map_err(|_| {
            Error::Validation("plugin development toolset registry lock poisoned".into())
        })?;
        operate(&mut state)
    }

    fn reserve_agent(&self, agent_id: &str) -> RsiResult<()> {
        self.lock(|state| {
            if state.bindings.contains_key(agent_id) || state.reserved.contains(agent_id) {
                return Err(Error::Validation(format!(
                    "agent `{agent_id}` is already bound to a plugin development task"
                )));
            }
            state.reserved.insert(agent_id.to_string());
            Ok(())
        })
    }

    fn release_reservation(&self, agent_id: &str) -> RsiResult<()> {
        self.lock(|state| {
            state.reserved.remove(agent_id);
            Ok(())
        })
    }
}

#[derive(Clone)]
struct PluginToolState {
    registry: Arc<PluginDevelopmentToolsetRegistry>,
    agent_id: String,
}

/// Build the specialized plugin development tools for one bound agent.
///
/// Pass the returned registrations to `Agent::builder().with_tools(...)`. The
/// agent remains inert until the host creates its binding in the global
/// registry.
pub fn plugin_development_tool_registrations(agent_id: impl Into<String>) -> Vec<ToolRegistration> {
    let state = PluginToolState {
        registry: PluginDevelopmentToolsetRegistry::global(),
        agent_id: agent_id.into(),
    };
    vec![
        ToolRegistration::from(PluginStatusTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginNodeSpecTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginNodeUpdateDocTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginNodeReadScriptTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginNodeWriteScriptTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginWorkspaceListTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginWorkspaceReadTool {
            state: state.clone(),
        }),
        ToolRegistration::from(PluginWorkspaceWriteTool {
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

fn tool_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: error.to_string().into(),
    }
}

fn load_manifest(workspace: &ProposalWorkspace) -> RsiResult<PluginManifest> {
    toml::from_str(&workspace.read_text(MANIFEST_FILE)?)
        .map_err(|error| Error::Validation(format!("invalid {MANIFEST_FILE}: {error}")))
}

fn save_manifest(workspace: &ProposalWorkspace, manifest: &PluginManifest) -> RsiResult<()> {
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
                "selected node `{}` is missing from the candidate manifest",
                node_kind
            ))
        })
}

fn resolve_binding(state: &PluginToolState) -> RsiResult<PluginDevelopmentBinding> {
    state.registry.binding(&state.agent_id)?.ok_or_else(|| {
        Error::Validation(format!(
            "agent `{}` has no plugin development binding",
            state.agent_id
        ))
    })
}

#[tool(
    name = "plugin_development_status",
    description = "Show the proposal, plugin nodes, run, environment, and candidate files assigned to this agent."
)]
struct PluginStatusInput {}

struct PluginStatusTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginStatusTool {
    type Input = PluginStatusInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let manifest = load_manifest(&binding.workspace()).map_err(tool_error)?;
        let files = binding.workspace().list_files().map_err(tool_error)?;
        let node_kinds = manifest
            .nodes
            .into_iter()
            .map(|node| node.kind)
            .collect::<Vec<_>>();
        Ok(ToolResult::success_json(json!({
            "proposal_id": binding.proposal_id,
            "plugin_name": binding.plugin_name,
            "node_kinds": node_kinds,
            "run_id": binding.run_id,
            "environment_reference": binding.environment_reference,
            "files": files,
        })))
    }
}

#[tool(
    name = "plugin_node_spec",
    description = "Read the complete definition of one node in the assigned plugin."
)]
struct PluginNodeSpecInput {
    /// Kind of the node to read.
    node_kind: String,
}

struct PluginNodeSpecTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeSpecTool {
    type Input = PluginNodeSpecInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let manifest = load_manifest(&binding.workspace()).map_err(tool_error)?;
        let node = selected_node(&manifest, &input.node_kind).map_err(tool_error)?;
        Ok(ToolResult::success_json(
            serde_json::to_value(node).map_err(tool_error)?,
        ))
    }
}

#[tool(
    name = "plugin_node_update_doc",
    description = "Replace one plugin node's documentation without changing its structural spec."
)]
struct PluginNodeUpdateDocInput {
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
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let workspace = binding.workspace();
        let mut manifest = load_manifest(&workspace).map_err(tool_error)?;
        let mut node = selected_node(&manifest, &input.node_kind).map_err(tool_error)?;
        node.doc = input.doc;
        let index = manifest
            .nodes
            .iter()
            .position(|existing| existing.kind == node.kind)
            .expect("selected node position");
        manifest.nodes[index] = node;
        save_manifest(&workspace, &manifest).map_err(tool_error)?;
        Ok(ToolResult::success("updated node documentation"))
    }
}

#[tool(
    name = "plugin_node_read_script",
    description = "Read the script file referenced by one node in the assigned plugin."
)]
struct PluginNodeReadScriptInput {
    /// Kind of the node whose script should be read.
    node_kind: String,
}

struct PluginNodeReadScriptTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeReadScriptTool {
    type Input = PluginNodeReadScriptInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let manifest = load_manifest(&binding.workspace()).map_err(tool_error)?;
        let node = selected_node(&manifest, &input.node_kind).map_err(tool_error)?;
        let script = node
            .command
            .script_file
            .ok_or_else(|| tool_error("selected node does not declare a script_file"))?;
        let contents = binding.workspace().read_text(&script).map_err(tool_error)?;
        Ok(ToolResult::success_json(json!({
            "path": script,
            "contents": contents,
        })))
    }
}

#[tool(
    name = "plugin_node_write_script",
    description = "Replace the script file referenced by one node in the assigned plugin."
)]
struct PluginNodeWriteScriptInput {
    /// Kind of the node whose script should be replaced.
    node_kind: String,
    /// Complete replacement script contents.
    contents: String,
}

struct PluginNodeWriteScriptTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginNodeWriteScriptTool {
    type Input = PluginNodeWriteScriptInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let manifest = load_manifest(&binding.workspace()).map_err(tool_error)?;
        let node = selected_node(&manifest, &input.node_kind).map_err(tool_error)?;
        let script = node
            .command
            .script_file
            .ok_or_else(|| tool_error("selected node does not declare a script_file"))?;
        binding
            .workspace()
            .write_text(&script, &input.contents)
            .map_err(tool_error)?;
        Ok(ToolResult::success_json(json!({ "path": script })))
    }
}

#[tool(
    name = "plugin_workspace_list",
    description = "List safe text files in this agent's detached plugin candidate."
)]
struct PluginWorkspaceListInput {}

struct PluginWorkspaceListTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginWorkspaceListTool {
    type Input = PluginWorkspaceListInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let files = binding.workspace().list_files().map_err(tool_error)?;
        Ok(ToolResult::success_json(Value::Array(
            files.into_iter().map(Value::String).collect(),
        )))
    }
}

#[tool(
    name = "plugin_workspace_read",
    description = "Read one safe text file from this agent's detached plugin candidate."
)]
struct PluginWorkspaceReadInput {
    /// Repository-relative file path.
    path: String,
}

struct PluginWorkspaceReadTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginWorkspaceReadTool {
    type Input = PluginWorkspaceReadInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        let contents = binding
            .workspace()
            .read_text(&input.path)
            .map_err(tool_error)?;
        Ok(ToolResult::success(contents))
    }
}

#[tool(
    name = "plugin_workspace_write",
    description = "Write one safe text file in this agent's detached plugin candidate. Structured manifest and Dockerfile changes are rejected."
)]
struct PluginWorkspaceWriteInput {
    /// Repository-relative file path.
    path: String,
    /// Complete replacement text contents.
    contents: String,
}

struct PluginWorkspaceWriteTool {
    state: PluginToolState,
}

#[async_trait]
impl ToolFunction for PluginWorkspaceWriteTool {
    type Input = PluginWorkspaceWriteInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let forbidden = input.path == MANIFEST_FILE
            || std::path::Path::new(&input.path)
                .file_name()
                .is_some_and(|name| name == "Dockerfile");
        if forbidden {
            return Err(ToolError::ValidationFailed {
                message:
                    "use node-specific tools; direct manifest and environment edits are denied"
                        .into(),
            });
        }
        let binding = resolve_binding(&self.state).map_err(tool_error)?;
        binding
            .workspace()
            .write_text(&input.path, &input.contents)
            .map_err(tool_error)?;
        Ok(ToolResult::success_json(json!({ "path": input.path })))
    }
}

#[tool(
    name = "plugin_container_run",
    description = "Run one argv command in the approved environment with the detached candidate mounted at /work. The container is isolated, resource-limited, and ephemeral."
)]
struct PluginContainerRunInput {
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
        let (binding, runtime) = self
            .state
            .registry
            .resolve(&self.state.agent_id)
            .map_err(tool_error)?;
        let workspace = binding.workspace();
        let workspace_ref = workspace_ref(
            runtime.workspace_root(),
            workspace.path(),
            DEFAULT_CONTAINER_WORKDIR,
        )
        .map_err(tool_error)?;
        let request = ContainerRunRequest {
            image: binding.environment_reference.clone(),
            command: input.argv,
            workspace: workspace_ref,
            env: vec![
                ("AUTONOMICS_PLUGIN_NAME".into(), binding.plugin_name.clone()),
                ("AUTONOMICS_DEVELOPMENT_RUN_ID".into(), binding.run_id),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Environment, EnvironmentCatalog, ProposalStore, RequestIntent, RequestRecord,
        RequestSource, RequestStatus, RequestStore,
    };
    use agentik_sdk::types::ToolResultContent;
    use container_plugin::node_definition::{
        CommandSpec, OutputSpec, PortKind, PortLayout, PortSpec,
    };
    use std::collections::BTreeMap;

    const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";

    struct FakeRuntime {
        root: PathBuf,
        requests: Arc<Mutex<Vec<ContainerRunRequest>>>,
    }

    #[async_trait]
    impl PodmanConnection for FakeRuntime {
        async fn run(
            &self,
            request: ContainerRunRequest,
        ) -> Result<ContainerRunResult, container_runtime::ContainerRuntimeError> {
            self.requests.lock().unwrap().push(request);
            Ok(ContainerRunResult {
                exit_code: 0,
                stdout: "development command completed".into(),
                stderr: String::new(),
            })
        }

        fn workspace_root(&self) -> &std::path::Path {
            &self.root
        }
    }

    fn catalog() -> EnvironmentCatalog {
        let mut catalog = EnvironmentCatalog::default();
        catalog.insert(
            "demo",
            Environment {
                reference: ENVIRONMENT_REFERENCE.into(),
                interpreters: vec!["sh".into()],
            },
        );
        catalog
    }

    fn setup(state: &tempfile::TempDir) -> PluginDevelopment<'static> {
        let requests = RequestStore::open(state.path());
        let request = requests
            .record(RequestRecord {
                id: String::new(),
                created_at: 0,
                source: RequestSource::User,
                intent: RequestIntent::NewNode,
                summary: "Develop a plugin node".into(),
                body: "Use the specialized plugin development tools.".into(),
                plugin_name: Some("tools-plugin".into()),
                evidence_ids: Vec::new(),
                status: RequestStatus::Open,
            })
            .unwrap();
        let store = Box::leak(Box::new(ProposalStore::open(
            state.path(),
            "main",
            "Autonomics RSI",
            "rsi@example.com",
        )));
        let mut development = store
            .create(
                "tools-plugin",
                &[request.id],
                "Develop through the dedicated toolset.",
                &requests,
            )
            .unwrap();
        development.bind_environment("demo", &catalog()).unwrap();
        let mut node = development
            .create_node(NodeDefinition {
                kind: "tools_plugin".into(),
                desc: "Demo adapter".into(),
                doc: "Original documentation.".into(),
                deprecated: false,
                timeout_secs: 3600,
                artifact_prefix: None,
                ports: PortLayout {
                    inputs: vec![PortSpec {
                        r#type: PortKind::File,
                        label: Some("input".into()),
                        accepted_formats: Vec::new(),
                    }],
                    outputs: vec![OutputSpec {
                        path: "result.txt".into(),
                        format: Some("txt".into()),
                        label: None,
                    }],
                },
                params: BTreeMap::new(),
                command: CommandSpec {
                    interpreter: "sh".into(),
                    argv: Vec::new(),
                    script: None,
                    script_file: Some("scripts/adapter.sh".into()),
                    env: BTreeMap::new(),
                    files: BTreeMap::new(),
                },
                resources: Default::default(),
            })
            .unwrap();
        node.write_script("#!/bin/sh\n").unwrap();
        development
    }

    async fn execute(
        tools: &[ToolRegistration],
        name: &str,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        tools
            .iter()
            .find(|tool| tool.definition.name == name)
            .unwrap()
            .implementation
            .execute(input)
            .await
    }

    #[tokio::test]
    async fn toolset_is_bound_to_one_plugin_candidate() {
        let state = tempfile::tempdir().unwrap();
        let mut development = setup(&state);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runtime = Arc::new(FakeRuntime {
            root: state.path().to_path_buf(),
            requests: requests.clone(),
        });
        let registry = PluginDevelopmentToolsetRegistry::global();
        registry.configure_runtime(runtime).unwrap();

        let binding = registry
            .bind_plugin_agent("agent-a", &mut development, "attempt-1")
            .unwrap();
        let tools = plugin_development_tool_registrations("agent-a");
        assert_eq!(tools.len(), 9);

        let status = execute(&tools, "plugin_development_status", json!({}))
            .await
            .unwrap();
        match status.content {
            ToolResultContent::Json(value) => {
                assert_eq!(value["plugin_name"], "tools-plugin");
                assert_eq!(value["node_kinds"], json!(["tools_plugin"]));
                assert_eq!(value["run_id"], "attempt-1");
            }
            other => panic!("unexpected status result: {other:?}"),
        }

        execute(
            &tools,
            "plugin_node_write_script",
            json!({
                "node_kind": "tools_plugin",
                "contents": "#!/bin/sh\nupdated\n"
            }),
        )
        .await
        .unwrap();
        let script = execute(
            &tools,
            "plugin_node_read_script",
            json!({ "node_kind": "tools_plugin" }),
        )
        .await
        .unwrap();
        match script.content {
            ToolResultContent::Json(value) => {
                assert_eq!(value["contents"], "#!/bin/sh\nupdated\n");
            }
            other => panic!("unexpected script result: {other:?}"),
        }
        assert_ne!(
            development
                .workspace()
                .read_text("scripts/adapter.sh")
                .unwrap(),
            "#!/bin/sh\nupdated\n"
        );

        execute(
            &tools,
            "plugin_node_update_doc",
            json!({
                "node_kind": "tools_plugin",
                "doc": "Updated documentation."
            }),
        )
        .await
        .unwrap();
        let run = execute(
            &tools,
            "plugin_container_run",
            json!({ "argv": ["sh", "scripts/adapter.sh"], "timeout_secs": 17 }),
        )
        .await
        .unwrap();
        match run.content {
            ToolResultContent::Json(value) => {
                assert_eq!(value["exit_code"], 0);
            }
            other => panic!("unexpected run result: {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].image, ENVIRONMENT_REFERENCE);
        assert_eq!(requests[0].command, vec!["sh", "scripts/adapter.sh"]);
        assert_eq!(requests[0].timeout_secs, 17);
        assert_eq!(requests[0].network, ContainerNetwork::Isolated);

        assert!(
            registry
                .bind_plugin_agent("agent-a", &mut development, "attempt-2")
                .is_err()
        );
        assert!(
            !state
                .path()
                .join("proposals")
                .join(development.id())
                .join("development-candidates")
                .join("attempt-2")
                .exists()
        );
        let unbound = plugin_development_tool_registrations("agent-b");
        assert!(
            execute(&unbound, "plugin_development_status", json!({}))
                .await
                .is_err()
        );
        assert!(binding.workspace().path().is_dir());
        registry.unbind_agent("agent-a").unwrap();
    }
}
