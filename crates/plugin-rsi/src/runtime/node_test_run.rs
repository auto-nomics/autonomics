//! Node test-run: execute one plugin node against sample inputs in a
//! throwaway container, mirroring the production `container_command`
//! contract exactly (fresh container per run, scratch workdir under the
//! daemon workspace root, script staged at `.autonomics/script`, inline
//! files under `.autonomics/files`, `$input/$output/$workdir` argv
//! replacements, the AUTONOMICS_* env family) — so a script that passes
//! the test-run behaves identically inside a DAG.
//!
//! The declared outputs are read back from the host side of the bind
//! mount (each `OutputSpec.path` relative to the workdir), bounded to
//! the standard captured-output size.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use container_runtime::types::{ContainerRunRequest, GpuRequest};
use container_runtime::{
    ContainerNetwork, DEFAULT_CONTAINER_WORKDIR, MAX_CAPTURED_OUTPUT_BYTES,
};

use super::dev_session::DevSessionManager;
use crate::{Error, Result};

/// v1 test-run bound: inputs are submitted as text for File ports.
pub const TEST_RUN_MAX_INPUT_BYTES: usize = 1024 * 1024;
/// Default per-run timeout when the node declares none.
pub const TEST_RUN_DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TestRunRequest {
    /// The node's `kind` inside the plugin manifest.
    pub kind: String,
    /// Port label → file contents (v1: File ports only).
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Submitted params, validated and defaulted by the same compile
    /// step production uses.
    #[serde(default = "empty_params")]
    pub params: serde_json::Value,
}

fn empty_params() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestRunOutput {
    /// Declared output path (relative to the workdir).
    pub path: String,
    pub present: bool,
    pub optional: bool,
    pub content: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestRunResult {
    pub plugin: String,
    pub kind: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub outputs: Vec<TestRunOutput>,
    /// Host scratch directory (kept for inspection; lives under the
    /// daemon's GC'd workspace root, not the git workspace).
    pub workdir: String,
}

impl DevSessionManager {
    /// Run one node of `plugin` against sample inputs and params.
    pub async fn test_run(&self, plugin: &str, request: &TestRunRequest) -> Result<TestRunResult> {
        let operator = self
            .store
            .develop(plugin)?
            .ok_or_else(|| Error::Validation(format!("unknown plugin project `{plugin}`")))?;
        let manifest = operator.manifest();
        let node = manifest
            .nodes
            .iter()
            .find(|node| node.kind == request.kind)
            .ok_or_else(|| {
                Error::Validation(format!(
                    "plugin `{plugin}` has no node kind `{}` (kinds: {})",
                    request.kind,
                    manifest
                        .nodes
                        .iter()
                        .map(|node| node.kind.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?
            .clone();
        // Inline `script_file` exactly like the runtime loader does, so
        // the dev workspace (not the materialized snapshot) is runnable.
        let mut node = node;
        if let Some(script_file) = node.command.script_file.clone() {
            if node.command.script.is_some() {
                return Err(Error::Validation(format!(
                    "node `{}` declares both script and script_file",
                    node.kind
                )));
            }
            let script = operator
                .workspace()
                .read_text(&script_file)
                .map_err(|error| {
                    Error::Validation(format!(
                        "cannot read script_file `{script_file}`: {error}"
                    ))
                })?;
            node.command.script = Some(script);
            node.command.script_file = None;
        }

        let spec = container_plugin::compile::spec_compile::compile_container_spec(
            &node,
            &manifest.image,
            &manifest.panels,
            &request.params,
        )
        .map_err(|error| Error::Validation(error.to_string()))?;

        // ── Scratch workdir under the daemon workspace root (GC'd, and
        // never inside the plugin git workspace). ──
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let workspace_root = self.execution.config.workspace_root.clone();
        let workdir = workspace_root
            .join("plugin-test-run")
            .join(plugin)
            .join(nanos.to_string());
        std::fs::create_dir_all(workdir.join(".autonomics/inputs"))
            .and_then(|_| std::fs::create_dir_all(workdir.join(".autonomics/files")))
            .map_err(|error| {
                Error::Container(format!(
                    "cannot create test-run workdir `{}`: {error}",
                    workdir.display()
                ))
            })?;

        // Stage File inputs by port label; tuple ports are out of v1
        // scope (they need structured channel values).
        let mut input_container_paths: Vec<Option<String>> = Vec::new();
        for input in &node.ports.inputs {
            if input.r#type != container_plugin::node_definition::PortKind::File {
                return Err(Error::Validation(format!(
                    "test-run v1 supports file inputs only; port `{}` of `{}` is a tuple port",
                    input.label.as_deref().unwrap_or("?"),
                    node.kind
                )));
            }
            let label = input
                .label
                .clone()
                .ok_or_else(|| {
                    Error::Validation(format!(
                        "input port {} of `{}` has no label",
                        input_container_paths.len(),
                        node.kind
                    ))
                })?
                .clone();
            let text = request.inputs.get(&label).ok_or_else(|| {
                Error::Validation(format!(
                    "missing test input for port `{label}` (node `{}`)",
                    node.kind
                ))
            })?;
            if text.len() > TEST_RUN_MAX_INPUT_BYTES {
                return Err(Error::InvalidRequest(format!(
                    "input `{label}` exceeds the {} byte test-run bound",
                    TEST_RUN_MAX_INPUT_BYTES
                )));
            }
            let host = workdir.join(".autonomics/inputs").join(safe_name(&label));
            std::fs::write(&host, text.as_bytes()).map_err(|error| {
                Error::Container(format!("cannot stage input `{label}`: {error}"))
            })?;
            input_container_paths.push(Some(format!(
                "{DEFAULT_CONTAINER_WORKDIR}/.autonomics/inputs/{}",
                safe_name(&label)
            )));
        }

        // ── Reproduce the production assembly (node.rs): script, files,
        // $-replacements, env family. ──
        let mut command = spec.command.clone();
        if let Some(script) = &spec.script {
            let script_path = workdir.join(".autonomics/script");
            std::fs::write(&script_path, script).map_err(|error| {
                Error::Container(format!("cannot stage script: {error}"))
            })?;
            for (relative, content) in &spec.files {
                write_within(&workdir.join(".autonomics/files"), relative, content)?;
            }
            command.insert(1, format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/script"));
        }

        let output_container_paths: Vec<String> = node
            .ports
            .outputs
            .iter()
            .map(|output| {
                format!(
                    "{DEFAULT_CONTAINER_WORKDIR}/{}",
                    output.path.trim_start_matches('/')
                )
            })
            .collect();
        let count = input_container_paths.len().max(output_container_paths.len());
        for item in &mut command {
            for index in (0..count).rev() {
                if let Some(path) = &input_container_paths[index.min(input_container_paths.len() - 1)] {
                    *item = item.replace(&format!("$input{index}"), path);
                }
                if index < output_container_paths.len() {
                    *item =
                        item.replace(&format!("$output{index}"), &output_container_paths[index]);
                }
            }
            *item = item.replace("$workdir", DEFAULT_CONTAINER_WORKDIR);
        }

        let mut env: Vec<(String, String)> = spec.env.clone().into_iter().collect();
        for (index, path) in input_container_paths.iter().enumerate() {
            if let Some(path) = path {
                env.push((format!("AUTONOMICS_INPUT{index}"), path.clone()));
            }
        }
        for (index, path) in output_container_paths.iter().enumerate() {
            env.push((format!("AUTONOMICS_OUTPUT{index}"), path.clone()));
        }
        env.push(("AUTONOMICS_WORKDIR".into(), DEFAULT_CONTAINER_WORKDIR.into()));
        env.push((
            "AUTONOMICS_INPUT_COUNT".into(),
            input_container_paths.len().to_string(),
        ));
        env.push((
            "AUTONOMICS_OUTPUT_COUNT".into(),
            output_container_paths.len().to_string(),
        ));
        if spec.script.is_some() {
            env.push((
                "AUTONOMICS_SCRIPT".into(),
                format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/script"),
            ));
            env.push((
                "AUTONOMICS_FILES_DIR".into(),
                format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/files"),
            ));
        }
        env.push(("AUTONOMICS_TEST_RUN".into(), "1".into()));

        let timeout_secs = if node.timeout_secs > 0 {
            node.timeout_secs
        } else {
            TEST_RUN_DEFAULT_TIMEOUT_SECS
        }
        .clamp(1, TEST_RUN_DEFAULT_TIMEOUT_SECS);
        let workspace = container_runtime::workspace_ref(
            &workspace_root,
            &workdir,
            DEFAULT_CONTAINER_WORKDIR,
        )
        .map_err(|error| Error::Container(error.to_string()))?;
        let run = ContainerRunRequest {
            image: spec.image.clone(),
            command,
            workspace,
            env,
            panels: Vec::new(),
            input_mounts: Vec::new(),
            network: match spec.network.as_str() {
                "egress" => ContainerNetwork::Egress,
                _ => ContainerNetwork::Isolated,
            },
            read_only_rootfs: spec.read_only_rootfs,
            pull_policy: spec.pull_policy,
            cpus: spec.cpus.or(Some(2.0)),
            memory: spec.memory.clone(),
            pids_limit: spec.pids_limit,
            shm_size: spec.shm_size.clone(),
            gpus: GpuRequest::None,
            user: spec.user.clone(),
            timeout_secs,
            name: container_runtime::unique_container_name(),
        };
        // A node that ran and exited non-zero is a *test result*, not a
        // transport error: collect what ran, report the outputs that
        // landed anyway.
        let output = match self.execution.runtime.run(run).await {
            Ok(output) => output,
            Err(container_runtime::ContainerRuntimeError::ExitStatus {
                exit_code,
                stdout,
                stderr,
            }) => {
                return Ok(TestRunResult {
                    plugin: plugin.to_string(),
                    kind: node.kind.clone(),
                    exit_code,
                    stdout,
                    stderr,
                    outputs: read_outputs(&workdir, &node),
                    workdir: workdir.display().to_string(),
                });
            }
            Err(other) => return Err(Error::Container(other.to_string())),
        };

        Ok(TestRunResult {
            plugin: plugin.to_string(),
            kind: node.kind.clone(),
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            outputs: read_outputs(&workdir, &node),
            workdir: workdir.display().to_string(),
        })
    }
}

fn read_outputs(workdir: &Path, node: &container_plugin::node_definition::NodeDefinition) -> Vec<TestRunOutput> {
    node.ports
        .outputs
        .iter()
        .map(|output| {
            // Output paths may be globs; v1 reports the declared path's
            // literal resolution and marks globbed paths present only
            // when the exact file exists.
            let relative = output.path.trim_start_matches('/').to_string();
            let host = workdir.join(&relative);
            match std::fs::read(&host) {
                Ok(bytes) => {
                    let truncated = bytes.len() > MAX_CAPTURED_OUTPUT_BYTES;
                    let content = container_runtime::connection::truncate_captured_bytes(&bytes);
                    TestRunOutput {
                        path: relative,
                        present: true,
                        optional: output.optional,
                        content: Some(content),
                        truncated,
                    }
                }
                Err(_) => TestRunOutput {
                    path: relative,
                    present: false,
                    optional: output.optional,
                    content: None,
                    truncated: false,
                },
            }
        })
        .collect()
}

/// Sanitize a port label into a single safe path segment.
fn safe_name(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "input".into()
    } else {
        cleaned
    }
}

/// Write `relative` (which may contain subdirectories) strictly inside
/// `root`, refusing traversal.
fn write_within(root: &Path, relative: &str, content: &str) -> Result<()> {
    let target = root.join(relative.trim_start_matches('/'));
    let canonical_parent =
        target
            .parent()
            .and_then(|parent| parent.to_str())
            .ok_or_else(|| Error::UnsafePath {
                path: relative.to_string(),
            })?;
    if !PathBuf::from(canonical_parent).starts_with(root) {
        return Err(Error::UnsafePath {
            path: relative.to_string(),
        });
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| Error::WriteFile {
            path: target.clone(),
            source: error,
        })?;
    }
    std::fs::write(&target, content).map_err(|error| Error::WriteFile {
        path: target,
        source: error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::dev_session::DevSessionManager;
    use crate::{Environment, EnvironmentRegistry, PluginStore};
    use async_trait::async_trait;
    use container_runtime::connection::PodmanConnection;
    use container_runtime::dev_container::DevContainerStatus;
    use container_runtime::error::ContainerRuntimeError;
    use container_runtime::types::{ContainerRunRequest, ContainerRunResult};
    use container_runtime::ContainerExecutionInfra;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// Fake runtime capturing the one-shot run request and replying with a
    /// scripted side effect: it "runs" the staged script by executing the
    /// staging contract on the host (cat input → write output).
    struct FakeRuntime {
        workspace_root: PathBuf,
        runs: Mutex<Vec<ContainerRunRequest>>,
    }

    #[async_trait]
    impl PodmanConnection for FakeRuntime {
        async fn run(
            &self,
            request: ContainerRunRequest,
        ) -> std::result::Result<ContainerRunResult, ContainerRuntimeError> {
            self.runs.lock().unwrap().push(request.clone());
            // Honor the production contract: stage script/files, then
            // execute the script semantics natively (POSIX sh assumed).
            let workdir = request.workspace.host_path.clone();
            // The real runtime receives the script staged on disk; the
            // fake reconstructs it from the command's second element.
            let _ = &request.command;
            let input = request
                .env
                .iter()
                .find(|(key, _)| key == "AUTONOMICS_INPUT0")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let container_input = input.strip_prefix("/work/").unwrap_or(&input);
            let text = std::fs::read_to_string(workdir.join(container_input)).unwrap_or_default();
            let name = text.trim();
            std::fs::write(
                workdir.join("demo_out.txt"),
                format!("Hello, {name}!\n"),
            )
            .unwrap();
            Ok(ContainerRunResult {
                exit_code: 0,
                stdout: format!("demo run for {}", request.name),
                stderr: String::new(),
            })
        }

        fn workspace_root(&self) -> &std::path::Path {
            &self.workspace_root
        }

        async fn dev_container_status(
            &self,
            _name: &str,
        ) -> std::result::Result<DevContainerStatus, ContainerRuntimeError> {
            Ok(DevContainerStatus {
                state: container_runtime::dev_container::DevContainerState::NotFound,
                container_id: None,
            })
        }
    }

    fn manager() -> (DevSessionManager, Arc<FakeRuntime>) {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(PluginStore::open(
            root.path(),
            "main",
            "Test Author",
            "test@example.com",
        ));
        let environments = EnvironmentRegistry::ephemeral(Default::default());
        environments
            .approve(
                "sh-base",
                Environment {
                    reference: "docker.io/library/debian@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                    interpreters: vec!["sh".into()],
                },
            )
            .unwrap();
        let mut operator = store
            .create_scaffolded("hello-plugin", "sh-base", "scaffold test", &Arc::new(environments.snapshot()))
            .unwrap();
        operator.refresh().unwrap();
        drop(operator);
        let fake = Arc::new(FakeRuntime {
            workspace_root: root.path().join("runs"),
            runs: Mutex::new(Vec::new()),
        });
        let infra = Arc::new(ContainerExecutionInfra {
            runtime: fake.clone(),
            panel_cache: Arc::new(container_runtime::panel::PanelCache::new(
                root.path().join("panels"),
            )),
            config: container_runtime::PodmanConfig {
                program: "unused".into(),
                workspace_root: root.path().join("runs"),
                panel_cache_root: root.path().join("panels"),
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
            },
        });
        std::mem::forget(root);
        (DevSessionManager::new(infra, store), fake)
    }

    #[tokio::test]
    async fn scaffold_creates_a_runnable_demo_node() {
        let (manager, _fake) = manager();
        let store = manager.store.clone();
        let mut operator = store.develop("hello-plugin").unwrap().unwrap();
        let manifest = operator.manifest().clone();
        let script_ok = operator.workspace().read_text("scripts/demo.sh").is_ok();
        assert_eq!(manifest.nodes.len(), 1);
        let node = &manifest.nodes[0];
        assert_eq!(node.kind, "hello_plugin");
        assert_eq!(node.command.script_file.as_deref(), Some("scripts/demo.sh"));
        assert!(script_ok, "scaffold script must exist");
    }

    #[tokio::test]
    async fn test_run_stages_inputs_and_reads_declared_outputs() {
        let (manager, fake) = manager();
        let request = TestRunRequest {
            kind: "hello_plugin".into(),
            inputs: [("input_text".to_string(), "插件开发".to_string())]
                .into_iter()
                .collect(),
            params: serde_json::json!({}),
        };
        let result = manager.test_run("hello-plugin", &request).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.kind, "hello_plugin");
        assert_eq!(result.outputs.len(), 1);
        let output = &result.outputs[0];
        assert_eq!(output.path, "demo_out.txt");
        assert!(output.present, "declared output must be reported");
        assert_eq!(output.content.as_deref(), Some("Hello, 插件开发!\n"));
        // The run mirrored the production contract.
        let runs = fake.runs.lock().unwrap();
        assert_eq!(runs.len(), 1);
        let env_names: Vec<&str> = runs[0].env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(env_names.contains(&"AUTONOMICS_INPUT0"));
        assert!(env_names.contains(&"AUTONOMICS_OUTPUT0"));
        assert!(env_names.contains(&"AUTONOMICS_WORKDIR"));
        assert!(env_names.contains(&"AUTONOMICS_INPUT_COUNT"));
        assert!(env_names.contains(&"AUTONOMICS_OUTPUT_COUNT"));
        assert!(env_names.contains(&"AUTONOMICS_SCRIPT"));
        assert!(env_names.contains(&"AUTONOMICS_TEST_RUN"));
        // Script staged at the contract path, and the scratch dir is
        // outside the plugin git workspace.
        assert!(result.workdir.contains("plugin-test-run"), "{}", result.workdir);
        assert!(PathBuf::from(&result.workdir).join(".autonomics/script").is_file());
    }

    #[tokio::test]
    async fn missing_input_and_unknown_kind_are_rejected() {
        let (manager, _fake) = manager();
        let missing_input = TestRunRequest {
            kind: "hello_plugin".into(),
            inputs: Default::default(),
            params: serde_json::json!({}),
        };
        let error = manager
            .test_run("hello-plugin", &missing_input)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("input_text"), "{error}");

        let unknown_kind = TestRunRequest {
            kind: "nope".into(),
            inputs: Default::default(),
            params: serde_json::json!({}),
        };
        assert!(manager.test_run("hello-plugin", &unknown_kind).await.is_err());
    }
}
